//! On-disk cache of Apple Music library data: `$XDG_CACHE_HOME/uzumaki/library/` (else
//! `~/.cache/uzumaki/library/`), one JSON file per list and one per opened album/playlist.
//!
//! Every file is `{"version": N, "saved_at": unix_secs, "data": ...}`. A version mismatch or a
//! parse error reads as "no cache". Writes are atomic (temp file + rename) and run on a background
//! thread, so the UI thread never waits for disk. Headless runs never create a `Cache`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Value, json};

use crate::models::{Collection, Shelf, Song};

/// Bump when a stored model changes shape.
const VERSION: u32 = 1;
/// Most collection files kept; the oldest (by mtime) are pruned.
const MAX_COLLECTIONS: usize = 300;

pub const SONGS: &str = "songs.json";
pub const ALBUMS: &str = "albums.json";
pub const PLAYLISTS: &str = "playlists.json";
pub const SHELVES: &str = "shelves.json";
/// Whether the last session was signed in (so the library can show before Electron is up).
pub const SESSION: &str = "session.json";

#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

/// A payload read back with the time it was saved.
pub struct Stored<T> {
    pub saved_at: u64,
    pub data: T,
}

/// A cached collection's song count, read without its songs (`uzu playlists`).
pub struct CountOnDisk {
    pub saved_at: u64,
    pub modified: Option<String>,
    pub count: usize,
}

/// Everything read at startup (collections are read lazily, when opened).
#[derive(Default)]
pub struct Cached {
    pub authorized: bool,
    pub songs: Option<Vec<Song>>,
    pub albums: Option<Vec<Collection>>,
    pub playlists: Option<Vec<Collection>>,
    pub shelves: [Option<Shelf>; 2],
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Cache {
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn default_location() -> Self {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(std::env::temp_dir);
        Self::at(base.join("uzumaki").join("library"))
    }

    /// The directory everything lives in (Settings → clear caches).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// File name (under `collections/`) for a collection id: anything odd becomes `_`.
    pub fn collection_name(id: &str) -> String {
        let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
        format!("collections/{safe}.json")
    }

    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Option<Stored<T>> {
        let text = std::fs::read_to_string(self.dir.join(name)).ok()?;
        let mut v: Value = serde_json::from_str(&text).ok()?;
        if v["version"].as_u64() != Some(VERSION as u64) {
            return None;
        }
        Some(Stored { saved_at: v["saved_at"].as_u64()?, data: serde_json::from_value(v["data"].take()).ok()? })
    }

    pub fn write<T: Serialize>(&self, name: &str, data: &T) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&json!({"version": VERSION, "saved_at": now_secs(), "data": data}))?;
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let result = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            std::fs::rename(&tmp, &path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Serializes and writes on a background thread (failures are only logged).
    pub fn save<T: Serialize + Send + 'static>(&self, name: impl Into<String>, data: T) {
        let (cache, name) = (self.clone(), name.into());
        let spawned = std::thread::Builder::new().name("cache-write".into()).spawn(move || {
            if let Err(e) = cache.write(&name, &data) {
                eprintln!("uzumaki: library cache write {name}: {e}");
            } else if name.starts_with("collections/") {
                cache.prune_collections(MAX_COLLECTIONS);
            }
        });
        if let Err(e) = spawned {
            eprintln!("uzumaki: library cache thread: {e}");
        }
    }

    /// No library on disk for an account that may be signed in: the first start, or the first
    /// after the library cache was cleared (Settings → General). The intro then waits for the
    /// whole library (`ui::intro`). A last session that was signed out has nothing to sync.
    pub fn needs_sync(&self) -> bool {
        if self.read::<bool>(SESSION).is_some_and(|s| !s.data) {
            return false;
        }
        ![SONGS, ALBUMS, PLAYLISTS].iter().any(|n| self.dir.join(n).is_file())
    }

    /// Nothing on disk says Uzumaki ran here before: no session on record (signed in or out)
    /// and no library. Part of the first-run onboarding rule (`ui::onboarding::at_launch`); it
    /// also covers a deleted or broken settings file on a machine that was already set up.
    pub fn fresh(&self) -> bool {
        !self.dir.join(SESSION).is_file() && ![SONGS, ALBUMS, PLAYLISTS].iter().any(|n| self.dir.join(n).is_file())
    }

    /// Whether the last run saw the user signed in (one small file; fine on the main thread).
    pub fn signed_in(&self) -> bool {
        self.read::<bool>(SESSION).is_some_and(|s| s.data)
    }

    /// Reads the startup files. Run it off the UI thread.
    pub fn load_all(&self) -> Cached {
        Cached {
            authorized: self.read::<bool>(SESSION).is_some_and(|s| s.data),
            songs: self.read(SONGS).map(|s| s.data),
            albums: self.read(ALBUMS).map(|s| s.data),
            playlists: self.read(PLAYLISTS).map(|s| s.data),
            shelves: self.read::<[Option<Shelf>; 2]>(SHELVES).map(|s| s.data).unwrap_or_default(),
        }
    }

    /// The file a collection is cached in.
    pub fn collection_path(&self, id: &str) -> PathBuf {
        self.dir.join(Self::collection_name(id))
    }

    /// How many songs the cached copy of `id` has, with its date and `modified`, without
    /// parsing the songs themselves. Only if the file really holds `id`.
    pub fn collection_count(&self, id: &str) -> Option<CountOnDisk> {
        #[derive(serde::Deserialize)]
        struct Data {
            id: String,
            #[serde(default)]
            modified: Option<String>,
            #[serde(default)]
            tracks: Vec<serde::de::IgnoredAny>,
        }
        #[derive(serde::Deserialize)]
        struct File {
            version: u64,
            saved_at: u64,
            data: Data,
        }
        let bytes = std::fs::read(self.collection_path(id)).ok()?;
        let f: File = serde_json::from_slice(&bytes).ok()?;
        (f.version == u64::from(VERSION) && f.data.id == id).then(|| CountOnDisk { saved_at: f.saved_at, modified: f.data.modified, count: f.data.tracks.len() })
    }

    /// One collection, only if the file really holds `id`.
    pub fn read_collection(&self, id: &str) -> Option<Stored<Collection>> {
        self.read::<Collection>(&Self::collection_name(id)).filter(|s| s.data.id == id)
    }

    /// Drops one cached album/playlist page (it changed, e.g. songs were added to it).
    pub fn forget_collection(&self, id: &str) {
        let _ = std::fs::remove_file(self.dir.join(Self::collection_name(id)));
    }

    /// Signed out: removes everything tied to the account (background thread).
    pub fn forget_personal(&self) {
        let cache = self.clone();
        std::thread::spawn(move || {
            for name in [SONGS, ALBUMS, PLAYLISTS, SHELVES] {
                let _ = std::fs::remove_file(cache.dir.join(name));
            }
            // Library ids start with "l." (albums) or "p." (playlists); catalog ones never do.
            if let Ok(rd) = std::fs::read_dir(cache.dir.join("collections")) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.starts_with("l.") || n.starts_with("p.") {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
        });
    }

    fn prune_collections(&self, keep: usize) {
        prune_oldest(&self.dir.join("collections"), keep);
    }
}

/// Does the probe (the newest items, newest first) match the cached list's newest ids?
/// Together with an equal `meta.total` this means nothing was added or removed.
pub fn same_newest<'a>(cached_newest_first: impl Iterator<Item = &'a str>, probe: &[String]) -> bool {
    cached_newest_first.take(probe.len()).eq(probe.iter().map(String::as_str))
}

/// A cached library playlist is current when its `lastModifiedDate` equals the one in the
/// freshly fetched playlist list (a missing date never counts as current).
pub fn playlist_current(cached: Option<&str>, listed: Option<&str>) -> bool {
    cached.is_some() && cached == listed
}

/// Keeps the `keep` newest files in `dir` (by mtime); also clears stale temp files.
fn prune_oldest(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> =
        rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|f| f.0);
    let extra = files.len() - keep;
    for (_, path) in files.into_iter().take(extra) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
