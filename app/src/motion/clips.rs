//! Motion clips on disk: `$XDG_CACHE_HOME/uzumaki/motion`, beside the artwork cache, within
//! `DISK_BUDGET` (oldest out, as `art.rs` prunes covers). One worker thread downloads: the
//! master playlist, then the chosen variant's playlist, then its one `.mp4`. Nothing here runs
//! on the UI thread.
//!
//! A file's name carries what the players need without asking the network again:
//! `<hash of the master URL>-<w>x<h>-<millifps>.mp4`. A clip already on disk plays offline.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use super::hls;

/// Kept clips (Apple's 768 H.264 loops are 4 to 6 MB: about 25 albums).
const DISK_BUDGET: u64 = 120 * 1024 * 1024;
/// A variant bigger than this is refused (the 768 loops are far below; 1080 HEVC could pass it).
const MAX_CLIP: u64 = 24 * 1024 * 1024;
const MAX_PLAYLIST: u64 = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Apple's clips are 23.976 fps; used when the master doesn't say.
pub const DEFAULT_FPS: f64 = 24000.0 / 1001.0;

/// A clip ready on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

/// `$XDG_CACHE_HOME/uzumaki/motion` (the artwork cache's sibling).
pub fn dir() -> PathBuf {
    crate::art::cache_dir().with_file_name("motion")
}

fn hash(url: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    h.finish()
}

fn file_name(master: &str, width: u32, height: u32, fps: f64) -> String {
    format!("{:016x}-{width}x{height}-{}.mp4", hash(master), (fps * 1000.0).round() as u64)
}

/// (hash, width, height, fps) from a clip's file name.
fn parse_name(name: &str) -> Option<(u64, u32, u32, f64)> {
    let stem = name.strip_suffix(".mp4")?;
    let mut parts = stem.split('-');
    let h = u64::from_str_radix(parts.next()?, 16).ok()?;
    let (w, ht) = parts.next()?.split_once('x')?;
    let fps = parts.next()?.parse::<u64>().ok()? as f64 / 1000.0;
    if parts.next().is_some() || fps <= 0.0 {
        return None;
    }
    Some((h, w.parse().ok()?, ht.parse().ok()?, fps))
}

/// The clip of `master` already in `dir`, touched so pruning keeps it. Never downloads.
pub fn find(dir: &Path, master: &str) -> Option<Clip> {
    let want = hash(master);
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let name = e.file_name();
        let (h, width, height, fps) = parse_name(name.to_str()?)?;
        let path = e.path();
        let len = e.metadata().ok()?.len();
        (h == want && len > 0).then(|| {
            if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            Clip { path, width, height, fps }
        })
    })
}

/// Deletes the oldest files until `dir` fits `budget` bytes (stale partial downloads too).
pub fn prune(dir: &Path, budget: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= budget {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err(format!("not an https URL: {url}"));
    }
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.into_body().with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("empty body".into());
    }
    Ok(bytes)
}

fn text(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    String::from_utf8(get(agent, url, MAX_PLAYLIST)?).map_err(|_| "playlist is not UTF-8".into())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let tmp = path.with_extension(format!("part{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The clip of `master`: from `dir`, else downloaded into it. Blocking (worker thread only).
pub fn fetch(agent: &ureq::Agent, dir: &Path, master: &str) -> Result<Clip, String> {
    if let Some(c) = find(dir, master) {
        return Ok(c);
    }
    let variants = hls::parse_master(&text(agent, master)?, master)?;
    let v = hls::choose(&variants).ok_or("no square variant")?;
    let file = hls::media_file(&text(agent, &v.uri)?, &v.uri)?;
    let bytes = get(agent, &file, MAX_CLIP)?;
    let fps = v.fps.unwrap_or(DEFAULT_FPS);
    let path = dir.join(file_name(master, v.width, v.height, fps));
    write_atomic(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    prune(dir, DISK_BUDGET);
    Ok(Clip { path, width: v.width, height: v.height, fps })
}

type Done = Box<dyn FnOnce(Result<Clip, String>) + Send>;

/// The download worker: started on the first request, then fed one master URL at a time.
#[derive(Default)]
pub struct Clips {
    jobs: Mutex<Option<Sender<(String, Done)>>>,
}

impl Clips {
    /// Finds or downloads the clip of `master` on the worker and hands the result to `done`
    /// (on the worker thread). The caller asks once per clip (`state::motion` remembers).
    pub fn request(&self, master: String, done: impl FnOnce(Result<Clip, String>) + Send + 'static) {
        let mut jobs = self.jobs.lock().unwrap();
        let tx = jobs.get_or_insert_with(|| {
            let (tx, rx) = channel::<(String, Done)>();
            std::thread::Builder::new()
                .name("motion-clips".into())
                .spawn(move || {
                    let dir = dir();
                    prune(&dir, DISK_BUDGET);
                    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).build().into();
                    for (master, done) in rx {
                        let result = fetch(&agent, &dir, &master);
                        if let Err(e) = &result {
                            eprintln!("uzumaki: animated artwork: {e}");
                        }
                        done(result);
                    }
                })
                .expect("spawn motion-clips");
            tx
        });
        let _ = tx.send((master, Box::new(done)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("uzumaki-motion-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_name_carries_size_and_rate() {
        let n = file_name("https://x/m.m3u8", 768, 768, DEFAULT_FPS);
        assert!(n.ends_with("-768x768-23976.mp4"), "{n}");
        let (h, w, ht, fps) = parse_name(&n).unwrap();
        assert_eq!((h, w, ht), (hash("https://x/m.m3u8"), 768, 768));
        assert!((fps - 23.976).abs() < 1e-9);
        assert!(parse_name("0123-768x768-0.mp4").is_none());
        assert!(parse_name("whatever.mp4").is_none());
        assert!(parse_name(&n.replace(".mp4", ".part1-0")).is_none(), "a partial download is never a clip");
    }

    #[test]
    fn find_returns_the_clip_of_its_master_only() {
        let d = scratch("find");
        assert!(find(&d, "https://x/a.m3u8").is_none());
        std::fs::write(d.join(file_name("https://x/a.m3u8", 768, 768, 24.0)), b"clip").unwrap();
        std::fs::write(d.join(file_name("https://x/b.m3u8", 256, 256, 30.0)), b"").unwrap();
        let c = find(&d, "https://x/a.m3u8").unwrap();
        assert_eq!((c.width, c.height, c.fps), (768, 768, 24.0));
        assert!(find(&d, "https://x/b.m3u8").is_none(), "an empty file is no clip");
        assert!(find(&d, "https://x/c.m3u8").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pruning_drops_the_oldest_until_the_budget_fits() {
        let d = scratch("prune");
        let t0 = std::time::SystemTime::now() - Duration::from_secs(1000);
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let p = d.join(name);
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(t0 + Duration::from_secs(100 * i as u64)).unwrap();
        }
        prune(&d, 300);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 3, "within budget: nothing goes");
        prune(&d, 150);
        assert!(!d.join("old").exists() && !d.join("mid").exists());
        assert!(d.join("new").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finding_a_clip_keeps_it_from_pruning() {
        let d = scratch("touch");
        let old = d.join(file_name("https://x/a.m3u8", 768, 768, 24.0));
        let other = d.join(file_name("https://x/b.m3u8", 768, 768, 24.0));
        for (p, age) in [(&old, 2000), (&other, 1000)] {
            std::fs::write(p, vec![0u8; 100]).unwrap();
            let f = std::fs::File::options().write(true).open(p).unwrap();
            f.set_modified(std::time::SystemTime::now() - Duration::from_secs(age)).unwrap();
        }
        assert!(find(&d, "https://x/a.m3u8").is_some());
        prune(&d, 150);
        assert!(old.exists(), "the clip just found is the newest now");
        assert!(!other.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_cache_sits_beside_the_artwork_cache() {
        assert_eq!(dir().parent(), crate::art::cache_dir().parent());
        assert_eq!(dir().file_name().unwrap(), "motion");
    }
}
