//! Artwork: download (HTTP + disk cache), decode, and keep decoded images.
//!
//! `get` is called from the view and must be cheap: it only touches in-memory bookkeeping. Misses
//! go to a small pool of download threads, which read the disk cache first, then fetch over HTTP,
//! decode the cover once (for its tint, and shrunk to the size it is drawn at, `memory_px`), and
//! hand the pixels to the UI thread (`Art::loaded`), which gives them to Rune's `ImageCache`.
//! Decoded covers are kept within `MEMORY_BUDGET`, least recently drawn first out, but a cover
//! the last view drew is never let go (`Lru::next_frame`): when a page shows more covers than the
//! budget holds, evicting drawn ones made them load again and push out others, a loop the user saw
//! as covers flickering back to their placeholder tiles (the New page on a 2× screen).
//!
//! The threads take the newest request first, so what is on screen now goes ahead of covers
//! scrolled past or left on another page. The library sync's covers (`warm`) only go to disk,
//! and wait until nothing on screen is waiting.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rune::{Image, ImageCache};

use crate::state::{Handle, State};

/// Download threads. A cover is small and most of its time is the round trip, so more threads
/// bring more covers a second (4 gave about 12 a second on a cold cache).
const WORKERS: usize = 8;
/// Bytes of decoded covers kept alive (ImageCache itself only holds them weakly), counting the
/// mip chain Rune builds (a third more). Covers the last view drew are kept beyond it if need be,
/// so the budget bounds the covers kept for later, not the ones on screen.
const MEMORY_BUDGET: usize = 64 << 20;
/// Cover tints kept.
const TINT_ENTRIES: usize = 400;
/// Disk cache budget; the oldest files go first at startup.
const DISK_BUDGET: u64 = 300 * 1024 * 1024;
/// Largest artwork body accepted.
const MAX_BODY: u64 = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

/// Pixel size of the full-screen Now Playing cover. State preloads this size for the current
/// and next songs, and the view must request exactly this size so it hits the same cache entry.
pub const NOW_PLAYING_PX: u32 = 1000;
/// Pixel size of small covers (player bar, Up Next, rows); Now Playing shows it scaled up
/// until the large one is ready.
pub const SMALL_PX: u32 = 120;
/// Pixel size of a song row's thumbnail (`Size::Fixed(36.0)` in the playlist, album and songs
/// lists asks for twice its points). The library sync downloads this size.
pub const ROW_PX: u32 = 72;
/// Pixel size of an album or playlist page's cover (`Size::Fixed(220.0)`).
pub const HEADER_PX: u32 = 440;

thread_local! {
    /// The display scale (device pixels per point) the view last drew at. Per thread: the view
    /// and the cover lookups run on the UI thread, and parallel headless tests at 1x and 2x
    /// each keep their own.
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// The view records the window's scale here every frame (`ui::view`).
pub fn set_scale(scale: f32) {
    if scale.is_finite() && scale > 0.0 {
        SCALE.set(scale);
    }
}

fn scale() -> f32 {
    SCALE.get()
}

/// The display scale of the last frame (`set_scale`), for views that pick a drawing by density.
pub fn display_scale() -> f32 {
    scale()
}

/// The longest side a cover downloaded at `px` is kept at in memory. Views ask Apple for twice
/// the points they draw at, so covers are sharp on 2× screens; on a 1× screen three fifths of
/// those pixels would sit in memory unseen. This keeps 1.25× the drawn size per screen pixel
/// (cards vary in width). The Now Playing cover (`NOW_PLAYING_PX`, drawn at most 560 pt) is
/// kept at 625 px on 1× screens; the cover background reads the file, not this.
fn memory_px(px: u32, scale: f32) -> u32 {
    let k = (scale.clamp(1.0, 2.0) * 0.625).min(1.0);
    ((px as f32 * k).ceil() as u32).clamp(32.min(px), px)
}

/// `(url, key)` for the `px` cover of `art`: the URL to download (and to name the disk file),
/// and the key of the decoded image in memory. A catalog template has `{w}x{h}`, so every size
/// is its own URL and the key is the URL. A template without it (a user-uploaded playlist
/// picture, a local file) gives one URL for every size, and the decode is shrunk to
/// `memory_px`: the key then carries that size, so a 45 px row thumbnail never stands in for the
/// header's 275 px cover. The suffix is for memory only; it never reaches the download or the disk.
fn cover_key(art: &crate::models::Artwork, px: u32, scale: f32) -> (String, String) {
    let url = art.url(px);
    let key = if art.template.contains("{w}") { url.clone() } else { format!("{}#{}", stable_url(&url), memory_px(px, scale)) };
    (url, key)
}

/// The part of `url` that names the picture. A user-uploaded playlist picture is a signed
/// `*.blobstore.apple.com` link: the path is the upload, the query only the signature, and every
/// fetch of the library list signs it anew (it dies after a day). Keyed by the full link, each
/// new signature missed memory and disk and the cover went back to its placeholder until it
/// downloaded again; keyed by the path, a re-signed link is the same picture. Other URLs are
/// kept whole (catalog links carry the size in the path, nothing in the query).
pub(crate) fn stable_url(url: &str) -> &str {
    let host = url.strip_prefix("https://").unwrap_or("").split('/').next().unwrap_or("");
    match url.find('?') {
        Some(q) if host.ends_with(".blobstore.apple.com") => &url[..q],
        _ => url,
    }
}

/// A cover decoded on a download thread: straight-alpha RGBA, at most `memory_px` square.
pub struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    tint: [u8; 3],
}

impl Decoded {
    /// What Rune keeps for it: the pixels plus their mip chain.
    fn bytes(&self) -> usize {
        self.rgba.len() * 4 / 3
    }
}

/// Decodes `bytes` once: the tint from a 16 px thumbnail, the pixels shrunk to fit `max_side`.
fn decode(bytes: &[u8], max_side: u32) -> Option<Decoded> {
    let img = image::load_from_memory(bytes).ok()?;
    let tint = tint_of(&img.thumbnail(16, 16).to_rgb8());
    let img = if img.width().max(img.height()) > max_side {
        img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(Decoded { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw(), tint })
}

pub struct Art {
    /// Covers are asked for and kept (a window with a handle, or a test standing in for one).
    live: bool,
    cache: Option<ImageCache>,
    /// The download threads' work; None in headless runs.
    queue: Option<Arc<Queue>>,
    inner: RefCell<Inner>,
    /// Covers handed to `warm` (each URL once, by its hash), for the sync's progress.
    warm_total: usize,
    warm_seen: HashSet<u64>,
}

struct Inner {
    images: Lru<Image>,
    tints: Lru<[u8; 3]>,
    /// Covers being fetched (by memory key), with the view frame that last asked for each (0: a preload).
    in_flight: HashMap<String, u64>,
    /// URLs given up on (not memory keys: a failed download fails at every size).
    failed: HashSet<String>,
    /// Counts for `UZUMAKI_ART_LOG`: covers decoded and let go.
    loads: u64,
    evictions: u64,
    /// The picture each held thumbnail last showed, by owner (`Art::held`).
    held: HashMap<String, Held>,
}

struct Held {
    img: Image,
    /// When the owner first showed a picture: it fades in for `FADE_IN` from then.
    since: std::time::Instant,
}

/// How long a held thumbnail's first picture counts as fading in (`Art::held`).
pub const FADE_IN: Duration = Duration::from_millis(220);

impl Default for Inner {
    fn default() -> Self {
        Self {
            images: Lru::with_budget(MEMORY_BUDGET),
            tints: Lru::with_budget(TINT_ENTRIES),
            in_flight: HashMap::new(),
            failed: HashSet::new(),
            loads: 0,
            evictions: 0,
            held: HashMap::new(),
        }
    }
}

impl Art {
    /// `handle` is None in headless runs: nothing is ever downloaded.
    pub fn new(handle: Option<Handle>) -> Self {
        let queue = handle.as_ref().map(|h| {
            let dir = cache_dir();
            let prune_dir = dir.clone();
            std::thread::Builder::new()
                .name("art-prune".into())
                .spawn(move || prune(&prune_dir, DISK_BUDGET))
                .ok();
            spawn_workers(h.clone(), dir)
        });
        Self { live: handle.is_some(), cache: None, queue, inner: RefCell::new(Inner::default()), warm_total: 0, warm_seen: HashSet::new() }
    }

    /// The library sync: downloads these covers to the disk cache, behind everything the view
    /// asks for. Nothing is decoded or kept in memory; the view loads them from disk when shown.
    pub fn warm(&mut self, urls: impl IntoIterator<Item = String>) {
        let Some(q) = &self.queue else { return };
        let fresh: Vec<String> = urls
            .into_iter()
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .filter(|u| self.warm_seen.insert(url_hash(stable_url(u))))
            .collect();
        if fresh.is_empty() {
            return;
        }
        self.warm_total += fresh.len();
        q.push_warm(fresh);
    }

    /// The library sync's covers: (on disk or given up, handed over).
    pub fn warm_progress(&self) -> (usize, usize) {
        let done = self.queue.as_ref().map_or(0, |q| q.warmed.load(Ordering::Relaxed));
        (done.min(self.warm_total), self.warm_total)
    }

    /// Starts downloading the `px` cover now (if it isn't cached) so it's ready when shown.
    /// A preload is not drawn, so it never holds a cover in memory past the budget.
    pub fn prefetch(&self, art: &crate::models::Artwork, px: u32) {
        let _ = self.request(art, px, false);
    }

    /// The view calls this once per build, before drawing any cover: covers `get` returns from
    /// here on are the ones this frame draws, and stay in memory until a later view leaves them out.
    pub fn begin_frame(&self) {
        self.inner.borrow_mut().images.next_frame();
    }

    /// The image for `url` if it is in memory, without asking for it or holding it: for covers
    /// built off screen (a carousel's far cards), drawn if they happen to be here.
    pub fn peek(&self, art: &crate::models::Artwork, px: u32) -> Option<Image> {
        if !self.live {
            return None;
        }
        self.inner.borrow().images.peek(&cover_key(art, px, scale()).1)
    }

    /// Called once from `AppSpec::setup` with the app's image cache.
    pub fn attach(&mut self, cache: ImageCache) {
        self.cache = Some(cache);
    }

    /// The image for `url` if it has been downloaded (it may still be decoding: draw it anyway,
    /// Rune shows it when ready). Otherwise starts a download (once per URL) and returns None,
    /// and the view draws a placeholder. Callable from the view.
    /// `px` is the size to download (the view's points times two); memory keeps it smaller on
    /// a 1× screen (`memory_px`).
    /// Drawn covers are held for this frame (`begin_frame`).
    pub fn get(&self, art: &crate::models::Artwork, px: u32) -> Option<Image> {
        self.request(art, px, true)
    }

    fn request(&self, art: &crate::models::Artwork, px: u32, drawn: bool) -> Option<Image> {
        if !self.live {
            return None;
        }
        let (url, key) = cover_key(art, px, scale());
        let mut inner = self.inner.borrow_mut();
        let frame = if drawn { inner.images.frame() } else { 0 };
        if let Some(img) = inner.images.get(&key, drawn) {
            return Some(img);
        }
        if inner.failed.contains(&url) {
            return None;
        }
        if let Some(asked) = inner.in_flight.get_mut(&key) {
            *asked = (*asked).max(frame);
            return None;
        }
        if let Some(q) = &self.queue {
            q.push_shown(key.clone(), url, memory_px(px, scale()));
            inner.in_flight.insert(key, frame);
        }
        None
    }

    /// The picture a small thumbnail that stays on screen (a sidebar playlist or pin, `owner` its
    /// id) draws, and whether it is the owner's first picture, still fading in (`FADE_IN`).
    /// Like `get` (asks once, holds it this frame), but once a picture has shown for `owner` it
    /// stays until the next one is ready: a new link, the cover let go from memory, or a new
    /// picture still decoding never put the placeholder back. No artwork forgets the owner.
    /// The handle kept here keeps Rune's pixels too (one 120 px cover per owner).
    pub fn held(&self, owner: &str, art: Option<&crate::models::Artwork>, px: u32) -> Option<(Image, bool)> {
        let Some(art) = art else {
            self.inner.borrow_mut().held.remove(owner);
            return None;
        };
        let now = self.get(art, px);
        let mut inner = self.inner.borrow_mut();
        if let Some(h) = inner.held.get_mut(owner) {
            // A new picture takes over once it can paint (a pending one paints nothing).
            if let Some(img) = now
                && h.img.id() != img.id()
                && (img.is_ready() || !h.img.is_ready())
            {
                h.img = img;
            }
            return Some((h.img.clone(), h.since.elapsed() < FADE_IN));
        }
        let img = now?;
        inner.held.insert(owner.to_string(), Held { img: img.clone(), since: std::time::Instant::now() });
        Some((img, true))
    }

    /// Tests: covers are kept as in a window, with `cache` (no downloads: nothing arrives
    /// unless `test_arrive` brings it).
    #[cfg(test)]
    pub(crate) fn test_live(&mut self, cache: ImageCache) {
        self.live = true;
        self.cache = Some(cache);
    }

    /// Tests: the `px` cover of `art` arrives as a `side` square of `rgb`, as from a download thread.
    #[cfg(test)]
    /// The display scale is process-wide and tests run side by side, so it lands at 1× and 2×.
    pub(crate) fn test_arrive(&mut self, art: &crate::models::Artwork, px: u32, side: u32, rgb: [u8; 3]) {
        for scale in [1.0, 2.0] {
            let (url, key) = cover_key(art, px, scale);
            let rgba = (0..side * side).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect();
            self.loaded(key, url, Decoded { width: side, height: side, rgba, tint: rgb });
        }
    }

    /// Tests: every cover leaves memory, as under pressure from a page full of covers.
    #[cfg(test)]
    pub(crate) fn test_evict_all(&mut self) {
        let inner = self.inner.get_mut();
        inner.images = Lru::with_budget(MEMORY_BUDGET);
        inner.in_flight.clear();
    }

    /// Representative background tint of the cover at `url` (the plain URL, whatever the size), once it has been downloaded and
    /// analysed (None before that, or if it could not be decoded). Cheap; never downloads.
    pub fn tint(&self, url: &str) -> Option<[u8; 3]> {
        self.inner.borrow_mut().tints.get(stable_url(url), false)
    }

    /// UI thread: a cover arrived, decoded on the worker thread.
    /// `key` is the memory key, `url` the download URL (`cover_key`); tints are kept by URL.
    pub fn loaded(&mut self, key: String, url: String, cover: Decoded) {
        let inner = self.inner.get_mut();
        // Asked for by the frame on screen: held like the covers it drew, so a page of more
        // covers than the budget fills up once instead of pushing out its own.
        let drawn = inner.in_flight.remove(&key).is_some_and(|f| f != 0 && f == inner.images.frame());
        inner.tints.put(stable_url(&url).to_string(), cover.tint, 1, false);
        match &self.cache {
            Some(cache) => {
                let bytes = cover.bytes();
                let img = cache.rgba(cover.width, cover.height, cover.rgba);
                let evicted = inner.images.put(key, img, bytes, drawn);
                inner.loads += 1;
                inner.evictions += evicted as u64;
                if log_enabled() {
                    let (kept, held) = inner.images.bytes();
                    eprintln!(
                        "uzumaki: art: {} loaded, {} let go; {} covers, {:.1} MiB ({:.1} MiB drawn), budget {} MiB",
                        inner.loads,
                        inner.evictions,
                        inner.images.len(),
                        kept as f64 / 1048576.0,
                        held as f64 / 1048576.0,
                        MEMORY_BUDGET >> 20
                    );
                }
            }
            None => {} // not attached yet: a later get() asks again
        }
    }

    /// UI thread: the download failed; don't retry this session.
    pub fn failed(&mut self, key: String, url: String) {
        let inner = self.inner.get_mut();
        inner.in_flight.remove(&key);
        inner.failed.insert(url);
    }
}

impl Drop for Art {
    fn drop(&mut self) {
        if let Some(q) = &self.queue {
            q.close();
        }
    }
}

/// `UZUMAKI_ART_LOG=1`: one line per cover loaded (loads, evictions, bytes kept), on stderr.
fn log_enabled() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("UZUMAKI_ART_LOG").is_some_and(|v| !v.is_empty() && v != "0"));
    *ON
}

/// What the download threads have to do.
#[derive(Default)]
struct Jobs {
    /// Covers the view asked for: (memory key, URL, side to keep it at); the newest is taken first.
    shown: Vec<(String, String, u32)>,
    /// The library sync's covers, for the disk only; taken in order, when `shown` is empty.
    warm: VecDeque<String>,
    closed: bool,
}

enum Job {
    Show(String, String, u32),
    Warm(String),
}

impl Jobs {
    fn next(&mut self) -> Option<Job> {
        if let Some((key, url, px)) = self.shown.pop() {
            return Some(Job::Show(key, url, px));
        }
        self.warm.pop_front().map(Job::Warm)
    }
}

struct Queue {
    jobs: Mutex<Jobs>,
    ready: Condvar,
    /// The library sync's covers done (on disk, or given up).
    warmed: AtomicUsize,
}

impl Queue {
    fn new() -> Self {
        Self { jobs: Mutex::new(Jobs::default()), ready: Condvar::new(), warmed: AtomicUsize::new(0) }
    }

    fn jobs(&self) -> std::sync::MutexGuard<'_, Jobs> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push_shown(&self, key: String, url: String, keep_px: u32) {
        self.jobs().shown.push((key, url, keep_px));
        self.ready.notify_one();
    }

    fn push_warm(&self, urls: Vec<String>) {
        self.jobs().warm.extend(urls);
        self.ready.notify_all();
    }

    fn close(&self) {
        self.jobs().closed = true;
        self.ready.notify_all();
    }

    /// Waits for the next job; None once the queue is closed.
    fn take(&self) -> Option<Job> {
        let mut jobs = self.jobs();
        loop {
            if jobs.closed {
                return None;
            }
            if let Some(job) = jobs.next() {
                return Some(job);
            }
            jobs = self.ready.wait(jobs).unwrap_or_else(|e| e.into_inner());
        }
    }
}

fn spawn_workers(handle: Handle, dir: PathBuf) -> Arc<Queue> {
    let queue = Arc::new(Queue::new());
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).build().into();
    for i in 0..WORKERS {
        let (queue, handle, dir, agent) = (queue.clone(), handle.clone(), dir.clone(), agent.clone());
        std::thread::Builder::new()
            .name(format!("art-{i}"))
            .spawn(move || {
                while let Some(job) = queue.take() {
                    match job {
                        Job::Show(key, url, keep_px) => show(&handle, &agent, &dir, key, url, keep_px),
                        Job::Warm(url) => {
                            // Already on disk: nothing to do. Failures are only logged; the
                            // view tries again when it shows the cover.
                            if !cache_path(&dir, &url).is_file()
                                && let Err(e) = fetch_retrying(&agent, &dir, &url)
                            {
                                eprintln!("uzumaki: artwork {url}: {e}");
                            }
                            let n = queue.warmed.fetch_add(1, Ordering::Relaxed) + 1;
                            // Moves the sync's progress now and then, and at the end.
                            if n % 25 == 0 || queue.jobs().warm.is_empty() {
                                handle.update_if(|s: &mut State, _tx| s.syncing);
                            }
                        }
                    }
                }
            })
            .ok();
    }
    queue
}

/// A cover the view asked for: fetched, decoded and handed to the UI thread.
fn show(handle: &Handle, agent: &ureq::Agent, dir: &Path, key: String, url: String, keep_px: u32) {
    let result = fetch_retrying(agent, dir, &url)
        .and_then(|bytes| decode(&bytes, keep_px).ok_or_else(|| "not an image we can decode".to_string()));
    match result {
        Ok(cover) => handle.update(move |s: &mut State, _tx| s.art.loaded(key, url, cover)),
        Err(e) => {
            eprintln!("uzumaki: artwork {url}: {e}");
            handle.update(move |s: &mut State, _tx| s.art.failed(key, url));
        }
    }
}

/// `fetch`, tried again after network hiccups (a reused pooled connection can be reset), not
/// after HTTP errors.
fn fetch_retrying(agent: &ureq::Agent, dir: &Path, url: &str) -> Result<Vec<u8>, String> {
    let mut result = fetch(agent, dir, url);
    for delay_ms in [300, 1500] {
        match &result {
            Err(e) if !e.starts_with("HTTP ") && !e.starts_with("not an") => {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                result = fetch(agent, dir, url);
            }
            _ => break,
        }
    }
    result
}

/// Average colour of `img`, ignoring near-black and near-white pixels (weighted toward saturated
/// ones), then moved to a mid luma (60..=140) so it works as a dark-UI background tint.
fn tint_of(img: &image::RgbImage) -> [u8; 3] {
    let luma = |p: [f64; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let (mut sum, mut wsum) = ([0f64; 3], 0f64);
    let (mut all, mut n) = ([0f64; 3], 0f64);
    for p in img.pixels() {
        let c = [p[0] as f64, p[1] as f64, p[2] as f64];
        for i in 0..3 {
            all[i] += c[i];
        }
        n += 1.0;
        let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
        if mx < 30.0 || mn > 225.0 {
            continue;
        }
        let w = 1.0 + 3.0 * (mx - mn) / mx;
        for i in 0..3 {
            sum[i] += c[i] * w;
        }
        wsum += w;
    }
    let avg = if wsum > 0.0 {
        [sum[0] / wsum, sum[1] / wsum, sum[2] / wsum]
    } else if n > 0.0 {
        [all[0] / n, all[1] / n, all[2] / n]
    } else {
        return [90, 90, 90];
    };
    let l = luma(avg);
    let target = l.clamp(60.0, 140.0);
    let out = if l < 1.0 {
        [target; 3]
    } else {
        let k = target / l;
        [avg[0] * k, avg[1] * k, avg[2] * k]
    };
    [out[0].min(255.0) as u8, out[1].min(255.0) as u8, out[2].min(255.0) as u8]
}

/// The bytes of `url`: the disk cache, else downloaded (and cached); `file://` reads the file.
/// Blocking: call it from worker threads only (`backdrop.rs`).
pub(crate) fn bytes_blocking(url: &str) -> Result<Vec<u8>, String> {
    static AGENT: std::sync::LazyLock<ureq::Agent> =
        std::sync::LazyLock::new(|| ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).build().into());
    fetch(&AGENT, &cache_dir(), url)
}

/// Disk cache first, then the network (and then the disk).
fn fetch(agent: &ureq::Agent, dir: &Path, url: &str) -> Result<Vec<u8>, String> {
    // Local files' covers: thumbnails the scanner already wrote to disk (not copied again).
    if let Some(path) = url.strip_prefix("file://") {
        return match std::fs::read(path) {
            Ok(bytes) if !bytes.is_empty() => Ok(bytes),
            Ok(_) => Err("not an http(s) URL: empty file".into()),
            Err(e) => Err(format!("not an http(s) URL: {e}")),
        };
    }
    let path = cache_path(dir, url);
    if let Ok(bytes) = std::fs::read(&path) {
        if !bytes.is_empty() {
            // Touch so pruning (oldest first) keeps what is in use.
            if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            return Ok(bytes);
        }
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("not an http(s) URL".into());
    }
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp
        .into_body()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("empty body".into());
    }
    if let Err(e) = write_atomic(&path, &bytes) {
        eprintln!("uzumaki: artwork cache write {}: {e}", path.display());
    }
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
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

/// `$XDG_CACHE_HOME/uzumaki/art`, else `~/.cache/uzumaki/art`.
pub(crate) fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("uzumaki").join("art")
}

fn url_hash(url: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    h.finish()
}

/// The cover `art` as a file already on disk, for `uzu` (`Status::cover`): the big Now Playing
/// size (the app preloads it for the current song), else the small one; a local song's cover is
/// its thumbnail file. None until one of them is there. Never downloads.
pub(crate) fn cached_file(art: &crate::models::Artwork) -> Option<PathBuf> {
    cached_file_in(&cache_dir(), art)
}

fn cached_file_in(dir: &Path, art: &crate::models::Artwork) -> Option<PathBuf> {
    [NOW_PLAYING_PX, SMALL_PX].into_iter().find_map(|px| {
        let url = art.url(px);
        let path = match url.strip_prefix("file://") {
            Some(local) => PathBuf::from(local),
            None => cache_path(dir, &url),
        };
        path.is_file().then_some(path)
    })
}

/// Stable file name per picture (`stable_url`: a re-signed link finds the file of the old one).
fn cache_path(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{:016x}", url_hash(stable_url(url))))
}

/// Deletes the oldest files until the directory fits `budget` bytes; also removes stale temp files.
fn prune(dir: &Path, budget: u64) {
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

/// Least-recently-returned map within a budget: each entry has a cost (bytes, or 1 to count
/// entries), and the oldest go until the total fits. The newest entry always stays, and so do the
/// entries held in the current frame (`get(.., true)` since `next_frame`, or `put(.., true)`): the
/// budget is for what is kept beyond those.
struct Lru<V> {
    /// Value, last use, cost, frame it was last held in.
    map: HashMap<String, (V, u64, usize, u64)>,
    tick: u64,
    /// The current frame; starts at 1 so 0 means "never held".
    frame: u64,
    budget: usize,
    total: usize,
}

impl<V> Lru<V> {
    fn with_budget(budget: usize) -> Self {
        Self { map: HashMap::new(), tick: 0, frame: 1, budget, total: 0 }
    }

    /// A new frame: what the last one held may go again, unless this one holds it too.
    fn next_frame(&mut self) {
        self.frame += 1;
    }

    fn frame(&self) -> u64 {
        self.frame
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    /// (all costs, the costs of entries held this frame).
    fn bytes(&self) -> (usize, usize) {
        let held = self.map.values().filter(|e| e.3 == self.frame).map(|e| e.2).sum();
        (self.total, held)
    }
}

impl<V: Clone> Lru<V> {
    /// The value, now the most recently used; `hold` keeps it for the rest of this frame.
    fn get(&mut self, key: &str, hold: bool) -> Option<V> {
        self.tick += 1;
        let (tick, frame) = (self.tick, self.frame);
        let (v, t, _, f) = self.map.get_mut(key)?;
        *t = tick;
        if hold {
            *f = frame;
        }
        Some(v.clone())
    }

    /// The value, leaving its age and hold alone.
    fn peek(&self, key: &str) -> Option<V> {
        self.map.get(key).map(|e| e.0.clone())
    }

    /// Adds `key` as the newest entry, then lets the oldest unheld ones go until the total fits
    /// (or only held ones and this one are left). Returns how many went.
    fn put(&mut self, key: String, value: V, cost: usize, hold: bool) -> usize {
        self.tick += 1;
        let f = if hold { self.frame } else { self.map.get(&key).map_or(0, |e| e.3) };
        if let Some((_, _, old, _)) = self.map.insert(key.clone(), (value, self.tick, cost, f)) {
            self.total -= old;
        }
        self.total += cost;
        let mut evicted = 0;
        while self.total > self.budget {
            let frame = self.frame;
            let oldest = self
                .map
                .iter()
                .filter(|(k, e)| e.3 != frame && **k != key)
                .min_by_key(|(_, e)| e.1)
                .map(|(k, _)| k.clone());
            match oldest.and_then(|k| self.map.remove(&k)) {
                Some((_, _, c, _)) => {
                    self.total -= c;
                    evicted += 1;
                }
                None => break,
            }
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_re_signed_upload_link_is_the_same_picture_in_memory_and_on_disk() {
        let old = "https://store-033.blobstore.apple.com/sq/e4/f7/uuid/image?X-Amz-Date=20261111T083315Z&X-Amz-Signature=aa";
        let new = "https://store-033.blobstore.apple.com/sq/e4/f7/uuid/image?X-Amz-Date=20261112T090000Z&X-Amz-Signature=bb";
        let art = |t: &str| crate::models::Artwork { template: t.into(), bg: None };
        assert_eq!(stable_url(old), "https://store-033.blobstore.apple.com/sq/e4/f7/uuid/image");
        assert_eq!(cover_key(&art(old), SMALL_PX, 2.0).1, cover_key(&art(new), SMALL_PX, 2.0).1, "one memory key");
        assert_ne!(cover_key(&art(old), SMALL_PX, 2.0).0, cover_key(&art(new), SMALL_PX, 2.0).0, "each downloads with its own signature");
        let dir = Path::new("/x");
        assert_eq!(cache_path(dir, old), cache_path(dir, new), "one disk file");
        let other = "https://store-033.blobstore.apple.com/sq/00/11/other/image?X-Amz-Date=20261112T090000Z";
        assert_ne!(cache_path(dir, old), cache_path(dir, other), "another upload is another picture");
        // Anything else keeps its whole URL, query included.
        let q = "https://example.com/a.jpg?v=2";
        assert_eq!(stable_url(q), q);
        assert_eq!(stable_url("file:///tmp/a.jpg"), "file:///tmp/a.jpg");
        let catalog = "https://is1-ssl.mzstatic.com/image/thumb/x/120x120bb.jpg";
        assert_eq!(stable_url(catalog), catalog);
    }

    #[test]
    fn on_screen_covers_go_newest_first_and_the_sync_waits() {
        let q = Queue::new();
        q.push_warm(vec!["w1".into(), "w2".into()]);
        q.push_shown("a".into(), "a".into(), 72);
        q.push_shown("b".into(), "b".into(), 72);
        let mut order = Vec::new();
        loop {
            let job = q.jobs().next(); // the lock goes before anything is pushed
            let Some(job) = job else { break };
            order.push(match job {
                Job::Show(u, _, _) => u,
                Job::Warm(u) => format!("warm {u}"),
            });
            if order.len() == 1 {
                q.push_shown("c".into(), "c".into(), 72); // scrolled on while "b" downloads
            }
        }
        assert_eq!(order, ["b", "c", "a", "warm w1", "warm w2"]);
        q.close();
        assert!(q.take().is_none());
    }

    #[test]
    fn tint_red_black_split_is_red() {
        let img = image::RgbImage::from_fn(16, 16, |x, _| {
            if x < 8 { image::Rgb([220, 20, 20]) } else { image::Rgb([0, 0, 0]) }
        });
        let [r, g, b] = tint_of(&img);
        assert!(r > 100 && r > g.saturating_mul(3) && r > b.saturating_mul(3), "{r},{g},{b}");
    }

    #[test]
    fn tint_white_and_black_are_sane_grays() {
        for v in [255u8, 0] {
            let img = image::RgbImage::from_pixel(8, 8, image::Rgb([v, v, v]));
            let [r, g, b] = tint_of(&img);
            assert!(r == g && g == b && (60..=140).contains(&r), "{r}");
        }
    }

    #[test]
    fn decode_tints_and_shrinks_once() {
        let img = image::RgbImage::from_pixel(320, 320, image::Rgb([30, 60, 200]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let d = decode(buf.get_ref(), 200).unwrap();
        let [r, _, b] = d.tint;
        assert!(b > r);
        assert_eq!((d.width, d.height, d.rgba.len()), (200, 200, 200 * 200 * 4));
        let whole = decode(buf.get_ref(), 1000).unwrap();
        assert_eq!((whole.width, whole.height), (320, 320), "never enlarged");
        assert!(decode(b"junk", 200).is_none());
    }

    #[test]
    fn covers_are_kept_at_the_size_they_are_drawn() {
        // A grid card asks for 320 px (twice ~160 pt): 200 px on a 1× screen, all of it at 2×.
        assert_eq!(memory_px(320, 1.0), 200);
        assert_eq!(memory_px(320, 1.5), 300);
        assert_eq!(memory_px(320, 2.0), 320);
        assert_eq!(memory_px(SMALL_PX, 1.0), 75);
        assert_eq!(memory_px(NOW_PLAYING_PX, 1.0), 625, "560 pt at most, with room");
        assert_eq!(memory_px(NOW_PLAYING_PX, 2.0), NOW_PLAYING_PX);
        assert_eq!(memory_px(40, 1.0), 32, "tiny ones keep a floor");
    }

    #[test]
    fn a_template_without_size_is_kept_per_memory_size() {
        // A signed upload link: the same URL at every size.
        let upload = crate::models::Artwork { template: "https://store-1.blobstore.apple.com/p.jpg?sig=1".into(), bg: None };
        let (small_url, small) = cover_key(&upload, ROW_PX, 1.0);
        let (big_url, big) = cover_key(&upload, HEADER_PX, 1.0);
        assert_eq!(small_url, upload.template, "one download");
        assert_eq!(big_url, small_url);
        assert_ne!(small, big, "a thumbnail never stands in for the header");
        assert_eq!(cover_key(&upload, ROW_PX, 1.0).1, small, "stable");
        assert_eq!(cache_path(Path::new("/c"), &small_url), cache_path(Path::new("/c"), &big_url), "one disk file");
        assert!(!small_url.contains('#') && !big_url.contains('#'), "no suffix on the wire or the disk name");
        // Sizes that are kept alike share the entry.
        assert_eq!(cover_key(&upload, HEADER_PX, 1.0).1, cover_key(&upload, HEADER_PX, 1.0).1);
        // Catalog covers: the key is the URL, as before.
        let catalog = crate::models::Artwork { template: "https://a/{w}x{h}.{f}".into(), bg: None };
        for px in [SMALL_PX, ROW_PX, HEADER_PX] {
            let (url, key) = cover_key(&catalog, px, 1.0);
            assert_eq!(url, catalog.url(px));
            assert_eq!(key, url);
        }
    }

    #[test]
    fn cache_path_is_stable_and_distinct() {
        let dir = Path::new("/tmp/x");
        let a = cache_path(dir, "https://a/1.jpg");
        assert_eq!(a, cache_path(dir, "https://a/1.jpg"));
        assert_ne!(a, cache_path(dir, "https://a/2.jpg"));
        assert_eq!(a.parent(), Some(dir));
        assert_eq!(a.file_name().unwrap().len(), 16);
    }

    #[test]
    fn cached_file_prefers_the_big_cover_and_never_guesses() {
        let dir = std::env::temp_dir().join(format!("uzumaki-art-cover-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let art = crate::models::Artwork { template: "https://a/{w}x{h}.{f}".into(), bg: None };
        assert_eq!(cached_file_in(&dir, &art), None, "nothing downloaded yet");
        let small = cache_path(&dir, &art.url(SMALL_PX));
        std::fs::write(&small, b"jpg").unwrap();
        assert_eq!(cached_file_in(&dir, &art), Some(small));
        let big = cache_path(&dir, &art.url(NOW_PLAYING_PX));
        std::fs::write(&big, b"jpg").unwrap();
        assert_eq!(cached_file_in(&dir, &art), Some(big));
        // A local song's cover is its own thumbnail file.
        let thumb = dir.join("thumb.jpg");
        let local = crate::models::Artwork { template: format!("file://{}", thumb.display()), bg: None };
        assert_eq!(cached_file_in(&dir, &local), None);
        std::fs::write(&thumb, b"jpg").unwrap();
        assert_eq!(cached_file_in(&dir, &local), Some(thumb));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lru_evicts_least_recently_returned() {
        let mut l: Lru<u32> = Lru::with_budget(2);
        l.put("a".into(), 1, 1, false);
        l.put("b".into(), 2, 1, false);
        assert_eq!(l.get("a", false), Some(1)); // a is now newer than b
        l.put("c".into(), 3, 1, false);
        assert_eq!(l.get("b", false), None);
        assert_eq!(l.get("a", false), Some(1));
        assert_eq!(l.get("c", false), Some(3));
        assert_eq!(l.map.len(), 2);
    }

    #[test]
    fn lru_keeps_within_its_byte_budget() {
        let mut l: Lru<u32> = Lru::with_budget(100);
        for i in 0..10 {
            l.put(format!("{i}"), i, 30, false);
        }
        assert_eq!((l.map.len(), l.total), (3, 90), "the three newest fit");
        l.put("9".into(), 9, 50, false); // replacing an entry counts its new cost only
        assert_eq!((l.map.len(), l.total), (2, 80));
        l.put("huge".into(), 0, 500, false);
        assert_eq!(l.map.len(), 1, "the newest stays even over budget");
    }

    #[test]
    fn lru_never_lets_go_of_what_this_frame_holds() {
        let mut l: Lru<u32> = Lru::with_budget(3);
        l.next_frame();
        for k in ["a", "b", "c"] {
            l.put(k.into(), 0, 1, true);
        }
        // A fourth cover drawn in the same frame goes over the budget rather than evicting.
        l.put("d".into(), 0, 1, true);
        assert_eq!((l.len(), l.bytes()), (4, (4, 4)));
        // A preload (not drawn) can't push out drawn covers either; it stays as the newest.
        assert_eq!(l.put("pre".into(), 0, 1, false), 0);
        assert_eq!(l.len(), 5);
        // Next frame draws only c and d: a, b and the preload may go, oldest first.
        l.next_frame();
        l.get("c", true);
        l.get("d", true);
        assert_eq!(l.put("e".into(), 0, 1, false), 3, "a, b and the preload went; e stays as the newest");
        assert!(l.peek("a").is_none() && l.peek("b").is_none() && l.peek("pre").is_none());
        assert!(l.peek("c").is_some() && l.peek("d").is_some() && l.peek("e").is_some());
        // peek reads without touching: f pushes out e (not held), not c or d.
        l.peek("e");
        l.put("f".into(), 0, 1, false);
        assert!(l.peek("e").is_none() && l.peek("f").is_some());
    }

    /// The New page at 2×: `shelves` carousels of 30 cards, all built by every view (320 px
    /// covers, 546 KB each with mips; 64 MiB holds 122). Each frame the view asks for every cover
    /// in page order; the misses land (decoded) before the next frame, the way `Art::loaded`
    /// puts them. Returns the covers loaded in each frame.
    fn simulate_new_page(shelves: usize, frames: usize, hold_drawn: bool) -> Vec<usize> {
        let cover = 320 * 320 * 4 * 4 / 3;
        let mut l: Lru<u32> = Lru::with_budget(MEMORY_BUDGET);
        let shown: Vec<String> = (0..shelves * 30).map(|i| format!("cover-{i}")).collect();
        (0..frames)
            .map(|_| {
                l.next_frame();
                let missed: Vec<&String> = shown.iter().filter(|u| l.get(u, hold_drawn).is_none()).collect();
                for u in &missed {
                    l.put((*u).clone(), 0, cover, hold_drawn);
                }
                missed.len()
            })
            .collect()
    }

    #[test]
    fn a_page_wider_than_the_budget_loads_once() {
        // The bug: drawn covers were let go like any other, so 180 covers on a page that holds
        // 122 reloaded 58 of them every frame, for ever (each landing redraws the page).
        assert_eq!(simulate_new_page(6, 6, false), [180, 58, 58, 58, 58, 58]);
        // Holding what the frame draws: every cover loads once, then nothing.
        assert_eq!(simulate_new_page(6, 6, true), [180, 0, 0, 0, 0, 0]);
        // Within the budget nothing changes.
        assert_eq!(simulate_new_page(4, 3, false), [120, 0, 0]);
    }

    #[test]
    fn prune_removes_oldest_first() {
        let dir = std::env::temp_dir().join(format!("uzumaki-art-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let p = dir.join(name);
            std::fs::write(&p, vec![0u8; 10]).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1000 + i as u64)).unwrap();
        }
        prune(&dir, 20);
        assert!(!dir.join("old").exists());
        assert!(dir.join("mid").exists() && dir.join("new").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
