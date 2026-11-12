//! Background pictures with their effect, ready to draw (Settings → Appearance).
//!
//! The view asks for a slot's picture each frame it shows it (`Backdrops::show`); rendering
//! (`uzumaki_fx::render`) happens off the UI thread and lands in a `rune::Pixels` the view draws.
//! Newest request wins; while a new one renders, `show` keeps returning the slot's last image.
//! Slots asking for the very same picture share it: Now Playing on "Same as app" draws the
//! content area's finished picture from its first frame, with no render and no second texture.
//!
//! CONTRACT (fixed by the main session): the public items below and their docs.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rune::Pixels;

use crate::appearance::Effect;
use crate::state::{Handle, State};

pub use uzumaki_fx::{Ground, Target};

/// Where a background is drawn. Each slot shows one picture at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    /// Behind the content area.
    App,
    /// Behind full-screen Now Playing.
    NowPlaying,
    /// The preview tile in Settings → Appearance.
    Preview,
}

/// Number of slots.
const SLOTS: usize = 3;

/// The largest screen, in points (each display's size over its scale), set once at startup.
static SCREEN: std::sync::OnceLock<[f32; 2]> = std::sync::OnceLock::new();

/// Records the screens (main.rs, from `rune::run_windows`). Backgrounds are rendered for the
/// largest one and drawn at a fixed scale, so resizing the window only shows more or less of
/// the same picture, never re-renders or zooms it.
pub fn set_screens(displays: &[rune::Display]) {
    let largest = displays.iter().fold([0.0f32, 0.0f32], |m, d| {
        let s = d.scale.max(1.0) as f32;
        [m[0].max(d.size.0 as f32 / s), m[1].max(d.size.1 as f32 / s)]
    });
    if largest[0] >= 1.0 && largest[1] >= 1.0 {
        set_screen(largest);
    }
}

/// Sets the largest screen in points (once; later calls are ignored). Scenes use it directly.
pub fn set_screen(points: [f32; 2]) {
    let _ = SCREEN.set(points);
}

/// The largest screen in points; None in headless runs (the window size stands in).
pub fn screen() -> Option<[f32; 2]> {
    SCREEN.get().copied()
}

impl Slot {
    fn index(self) -> usize {
        match self {
            Slot::App => 0,
            Slot::NowPlaying => 1,
            Slot::Preview => 2,
        }
    }
}

/// The picture a background starts from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// `uzumaki_fx::builtin`, drawn at the target size.
    Builtin,
    /// A PNG or JPEG file.
    File(PathBuf),
    /// Artwork by URL: read from the artwork disk cache, else downloaded (`art::bytes_blocking`).
    /// `file://` URLs (local files' covers) are read directly.
    Url(String),
}

/// One background: a source, an effect, the theme it sits on and the device-pixel size of the
/// element it fills. The ground is part of the key: a theme switch asks for a new picture, and a
/// dark Now Playing never shares the light content area's picture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Request {
    pub source: Source,
    pub effect: Effect,
    pub ground: Ground,
    pub target: Target,
}

/// What a slot draws now.
#[derive(Clone)]
pub struct Shown {
    /// Draw with `El::pixels_fit(&pixels, SurfaceFit::Fill)` on an element of the requested
    /// size (Blur images are smaller and get stretched, which is fine).
    pub pixels: Pixels,
    /// Changes whenever `pixels` holds a different picture (a new request finished): key the
    /// element by it to cross-fade between pictures.
    pub id: u64,
}

/// The background service, in `State::backdrops`.
///
/// One long-lived worker thread (spawned only when there is a handle), fed through a channel;
/// a slot's pending request is replaced by a newer one before it starts (newest wins, at most one
/// render in flight per slot: the worker does one job at a time). A request waits `DEBOUNCE` for
/// newer ones (a window resize asks every frame), except for a slot with nothing shown yet.
/// Decoded sources are cached (a small LRU) so changing only the effect or the size doesn't decode
/// again. Results go back with `handle.update(..)`; the slot then returns them from `show`.
/// Without a handle (headless scenes, tests) `show` renders synchronously on the calling thread
/// and never touches the network.
/// A failed request (unreadable file, download error) sets `error(slot)` and leaves the slot's
/// last image showing. Memory: a slot keeps the image it shows (the view holds the one
/// cross-fading out); `hide` drops it.
///
/// A request another slot already shows finished (same source, effect and size) takes that
/// picture at once, and a finished render goes to every slot asking for it; the slot's own
/// render, if one was on its way, is dropped. The shared `Pixels` are one atlas slot and one
/// upload in Rune.
pub struct Backdrops {
    handle: Option<Handle>,
    /// To the worker; None without a handle.
    jobs: Option<Sender<Msg>>,
    /// Per slot: bumped by `hide`, so the worker skips and `finished` drops older jobs.
    epochs: Arc<[AtomicU64; SLOTS]>,
    inner: RefCell<Inner>,
}

/// Wait for newer requests this long before rendering (a drag-resize asks every frame).
const DEBOUNCE: Duration = Duration::from_millis(120);
/// Decoded pictures are downscaled to at most this on the longer side.
const MAX_SIDE: u32 = 3200;
/// Decoded sources kept while Settings → Appearance is open (switching effects there re-renders
/// at once). Otherwise none: the finished picture is all that stays, and the next render (a new
/// cover, a bigger window) decodes the file again.
const CACHE_ENTRIES: usize = 2;

#[derive(Default)]
struct Inner {
    slots: [SlotState; SLOTS],
    next_id: u64,
    /// Decoded sources for the synchronous (headless) path.
    sync_cache: SourceCache,
}

#[derive(Default)]
struct SlotState {
    asked: Option<Request>,
    shown: Option<Shown>,
    /// The request `shown` answers (`asked` may be newer while it renders).
    shown_req: Option<Request>,
    error: Option<String>,
}

impl SlotState {
    /// Show `shown`, the finished picture for `req`.
    fn take(&mut self, req: &Request, shown: Shown) {
        self.shown = Some(shown);
        self.shown_req = Some(req.clone());
        self.error = None;
    }
}

/// To the worker.
enum Msg {
    Job(Job),
    /// Both slots are hidden: drop the decoded sources (a big photo is tens of MB).
    Forget,
    /// The Appearance preview opened (true) or closed: keep decoded sources while it is open.
    Editing(bool),
}

struct Job {
    slot: Slot,
    epoch: u64,
    req: Request,
    /// Not before this (arrival + `DEBOUNCE`, or arrival for a slot with nothing shown).
    ready_at: Instant,
}

/// What the worker should do next.
#[derive(Debug, PartialEq)]
enum Next {
    Run(Job),
    /// Nothing is ready: wait this long for newer requests.
    Wait(Duration),
    Idle,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Job({:?}, epoch {}, {:?})", self.slot, self.epoch, self.req.target)
    }
}
impl PartialEq for Job {
    fn eq(&self, o: &Job) -> bool {
        self.slot == o.slot && self.epoch == o.epoch && self.req == o.req && self.ready_at == o.ready_at
    }
}

/// Pending-request bookkeeping of the worker (pure, so it is testable): one job per slot,
/// a newer one replaces the older.
#[derive(Default)]
struct Scheduler {
    /// One per slot. (It once had room for two while there were three slots: the Appearance
    /// preview's first job indexed past the end and the worker thread died.)
    pending: [Option<Job>; SLOTS],
}

impl Scheduler {
    fn push(&mut self, job: Job) {
        let i = job.slot.index();
        self.pending[i] = Some(job);
    }

    /// The ready job that has waited longest, else how long until one is ready.
    fn next(&mut self, now: Instant) -> Next {
        let ready = (0..SLOTS).filter_map(|i| self.pending[i].as_ref().map(|j| (i, j.ready_at))).min_by_key(|&(_, at)| at);
        match ready {
            None => Next::Idle,
            Some((i, at)) if at <= now => Next::Run(self.pending[i].take().unwrap()),
            Some((_, at)) => Next::Wait(at - now),
        }
    }
}

/// Decoded sources, most recently used first.
#[derive(Default)]
struct SourceCache(Vec<(Source, Arc<image::RgbaImage>)>);

impl SourceCache {
    fn get(&mut self, source: &Source) -> Option<Arc<image::RgbaImage>> {
        let at = self.0.iter().position(|(s, _)| s == source)?;
        let hit = self.0.remove(at);
        let img = hit.1.clone();
        self.0.insert(0, hit);
        Some(img)
    }

    fn put(&mut self, source: Source, img: Arc<image::RgbaImage>) {
        self.0.retain(|(s, _)| *s != source);
        self.0.insert(0, (source, img));
        self.0.truncate(CACHE_ENTRIES);
    }
}

/// The picture for `req`, rendered (blocking).
fn render_request(cache: &mut SourceCache, req: &Request, online: bool) -> Result<image::RgbaImage, String> {
    if req.source == Source::Builtin {
        let src = uzumaki_fx::builtin_on(req.target.width, req.target.height, req.ground);
        return Ok(uzumaki_fx::render_on(&src, req.effect, req.target, req.ground));
    }
    let src = match cache.get(&req.source) {
        Some(src) => src,
        None => {
            let t0 = Instant::now();
            let src = Arc::new(load(&req.source, online)?);
            crate::framelog::event(|| {
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                format!("backdrop: decoded the source ({}x{}) in {ms:.0} ms", src.width(), src.height())
            });
            cache.put(req.source.clone(), src.clone());
            src
        }
    };
    Ok(uzumaki_fx::render_on(&src, req.effect, req.target, req.ground))
}

fn worker(rx: Receiver<Msg>, epochs: Arc<[AtomicU64; SLOTS]>, handle: Handle) {
    let mut sched = Scheduler::default();
    let mut cache = SourceCache::default();
    let mut editing = false;
    let take = |msg: Msg, sched: &mut Scheduler, cache: &mut SourceCache, editing: &mut bool| match msg {
        Msg::Job(job) => sched.push(job),
        Msg::Forget => *cache = SourceCache::default(),
        Msg::Editing(on) => *editing = on,
    };
    loop {
        while let Ok(msg) = rx.try_recv() {
            take(msg, &mut sched, &mut cache, &mut editing);
        }
        match sched.next(Instant::now()) {
            Next::Run(job) => {
                if epochs[job.slot.index()].load(Ordering::Acquire) != job.epoch {
                    continue; // hidden since
                }
                let (slot, epoch, req) = (job.slot, job.epoch, job.req);
                let t0 = Instant::now();
                let result = render_request(&mut cache, &req, true).map(|img| {
                    let (w, h) = img.dimensions();
                    let pixels = Pixels::new();
                    pixels.set(w, h, img.into_raw());
                    pixels
                });
                crate::framelog::event(|| {
                    let ms = t0.elapsed().as_secs_f64() * 1000.0;
                    format!("backdrop {slot:?}: rendered {}x{} in {ms:.0} ms", req.target.width, req.target.height)
                });
                handle.update(move |s: &mut State, _tx| s.backdrops.finished(slot, epoch, &req, result));
            }
            Next::Wait(d) => match rx.recv_timeout(d) {
                Ok(msg) => take(msg, &mut sched, &mut cache, &mut editing),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            },
            Next::Idle => {
                // Nothing left to render: keep only the finished pictures (the decoded source
                // of a big photo is tens of MB), unless Appearance is open.
                if !editing {
                    cache = SourceCache::default();
                }
                match rx.recv() {
                    Ok(msg) => take(msg, &mut sched, &mut cache, &mut editing),
                    Err(_) => return,
                }
            }
        }
    }
}

impl Backdrops {
    /// `handle` is None in headless runs and tests.
    pub fn new(handle: Option<Handle>) -> Self {
        let epochs = Arc::new(std::array::from_fn(|_| AtomicU64::new(0)));
        let jobs = handle.as_ref().and_then(|h| {
            let (tx, rx) = mpsc::channel();
            let (epochs, h) = (epochs.clone(), h.clone());
            std::thread::Builder::new().name("backdrop".into()).spawn(move || worker(rx, epochs, h)).ok().map(|_| tx)
        });
        Self { handle, jobs, epochs, inner: RefCell::new(Inner::default()) }
    }

    /// Call from the view while `slot` is on screen. Asks for `req` (asking again for the same
    /// request is free) and returns what to draw now: the latest finished picture for the slot,
    /// possibly an older request's while `req` renders, or None before the first one is ready.
    pub fn show(&self, slot: Slot, req: &Request) -> Option<Shown> {
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;
        let i = slot.index();
        // Another slot already shows this very picture: share it (no render, no new texture).
        let shared = (inner.slots[i].asked.as_ref() != Some(req))
            .then(|| inner.slots.iter().find(|o| o.shown_req.as_ref() == Some(req)).and_then(|o| o.shown.clone()))
            .flatten();
        let st = &mut inner.slots[i];
        if st.asked.as_ref() != Some(req) {
            let opening = slot == Slot::Preview && st.asked.is_none();
            st.asked = Some(req.clone());
            if opening && let Some(tx) = &self.jobs {
                let _ = tx.send(Msg::Editing(true));
            }
            if let Some(shown) = shared {
                st.take(req, shown);
                // A render of an older request for this slot may still be on its way: drop it.
                self.epochs[i].fetch_add(1, Ordering::AcqRel);
                crate::framelog::event(|| format!("backdrop {slot:?}: shares a finished picture"));
            } else if self.handle.is_some() {
                let now = Instant::now();
                // Nothing to show yet: start at once; otherwise wait for newer requests.
                let ready_at = if st.shown.is_none() { now } else { now + DEBOUNCE };
                let epoch = self.epochs[slot.index()].load(Ordering::Acquire);
                let sent = self.jobs.as_ref().is_some_and(|tx| tx.send(Msg::Job(Job { slot, epoch, req: req.clone(), ready_at })).is_ok());
                if !sent {
                    st.error = Some("the background renderer is not running".into());
                }
            } else {
                match render_request(&mut inner.sync_cache, req, false) {
                    Ok(img) => {
                        let (w, h) = img.dimensions();
                        let pixels = Pixels::new();
                        pixels.set(w, h, img.into_raw());
                        inner.next_id += 1;
                        st.take(req, Shown { pixels, id: inner.next_id });
                    }
                    Err(e) => st.error = Some(e),
                }
            }
        }
        st.shown.clone()
    }

    /// `slot` went off screen (its setting was turned off, Now Playing closed): free its
    /// images. The next `show` starts over.
    pub fn hide(&self, slot: Slot) {
        let mut inner = self.inner.borrow_mut();
        let st = &mut inner.slots[slot.index()];
        if st.asked.is_none() && st.shown.is_none() && st.error.is_none() {
            return; // called every frame while off: stay cheap
        }
        *st = SlotState::default();
        self.epochs[slot.index()].fetch_add(1, Ordering::AcqRel);
        if slot == Slot::Preview && let Some(tx) = &self.jobs {
            let _ = tx.send(Msg::Editing(false));
        }
        if inner.slots.iter().all(|st| st.asked.is_none()) {
            inner.sync_cache = SourceCache::default();
            if let Some(tx) = &self.jobs {
                let _ = tx.send(Msg::Forget);
            }
        }
    }

    /// The id of the picture `slot` shows now (tests: two slots sharing one picture).
    #[cfg(test)]
    pub fn shown_id(&self, slot: Slot) -> Option<u64> {
        self.inner.borrow().slots[slot.index()].shown.as_ref().map(|s| s.id)
    }

    /// Why the slot's latest request failed, if it did (for Settings to show).
    pub fn error(&self, slot: Slot) -> Option<String> {
        self.inner.borrow().slots[slot.index()].error.clone()
    }

    /// UI thread: the worker finished `req` for `slot`. Dropped if the slot was hidden since
    /// (`epoch` differs). Other slots waiting for the same request get it too, and their own
    /// render of it is dropped.
    pub(crate) fn finished(&mut self, slot: Slot, epoch: u64, req: &Request, result: Result<Pixels, String>) {
        if self.epochs[slot.index()].load(Ordering::Acquire) != epoch {
            crate::framelog::event(|| format!("backdrop {slot:?}: late result dropped"));
            return;
        }
        crate::framelog::event(|| format!("backdrop {slot:?}: landed"));
        let inner = self.inner.get_mut();
        match result {
            Ok(pixels) => {
                inner.next_id += 1;
                let shown = Shown { pixels, id: inner.next_id };
                inner.slots[slot.index()].take(req, shown.clone());
                for (k, st) in inner.slots.iter_mut().enumerate() {
                    if k != slot.index() && st.asked.as_ref() == Some(req) && st.shown_req.as_ref() != Some(req) {
                        st.take(req, shown.clone());
                        self.epochs[k].fetch_add(1, Ordering::AcqRel);
                    }
                }
            }
            Err(e) => inner.slots[slot.index()].error = Some(e),
        }
    }
}

/// Decodes a request's source (blocking), downscaled to at most `MAX_SIDE`. `online` is false in
/// headless runs and tests: they never download (only `file://` artwork is read).
fn load(source: &Source, online: bool) -> Result<image::RgbaImage, String> {
    let bytes = match source {
        Source::Builtin => return Err("no file".into()),
        Source::File(path) => std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), short(&e)))?,
        Source::Url(url) if !online && !url.starts_with("file://") => return Err("offline".into()),
        Source::Url(url) => crate::art::bytes_blocking(url)?,
    };
    let img = image::load_from_memory(&bytes).map_err(|e| format!("not a readable picture ({e})"))?;
    drop(bytes);
    let img = if img.width().max(img.height()) > MAX_SIDE {
        let k = MAX_SIDE as f32 / img.width().max(img.height()) as f32;
        let (w, h) = (((img.width() as f32 * k).round() as u32).max(1), ((img.height() as f32 * k).round() as u32).max(1));
        img.resize_exact(w, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(img.into_rgba8())
}

fn short(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "not found".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(source: Source, effect: Effect, w: u32) -> Request {
        Request { source, effect, ground: Ground::Dark, target: Target { width: w, height: w / 2, unit: 1 } }
    }
    fn job(slot: Slot, w: u32, ready_at: Instant) -> Job {
        Job { slot, epoch: 0, req: req(Source::Builtin, Effect::Plain, w), ready_at }
    }

    #[test]
    fn every_slot_can_queue_a_job() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        for slot in [Slot::App, Slot::NowPlaying, Slot::Preview] {
            s.push(job(slot, 100, t0));
        }
        let mut ran = Vec::new();
        while let Next::Run(j) = s.next(t0) {
            ran.push(j.slot.index());
        }
        ran.sort();
        assert_eq!(ran, [0, 1, 2]);
    }

    #[test]
    fn newest_request_wins_per_slot() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.push(job(Slot::App, 100, t0));
        s.push(job(Slot::App, 200, t0));
        s.push(job(Slot::NowPlaying, 300, t0));
        let Next::Run(a) = s.next(t0) else { panic!() };
        let Next::Run(b) = s.next(t0) else { panic!() };
        assert_eq!((a.slot, a.req.target.width, b.slot, b.req.target.width), (Slot::App, 200, Slot::NowPlaying, 300));
        assert_eq!(s.next(t0), Next::Idle);
    }

    #[test]
    fn debounce_waits_and_a_newer_request_restarts_the_wait() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.push(job(Slot::App, 100, t0 + DEBOUNCE));
        assert_eq!(s.next(t0), Next::Wait(DEBOUNCE));
        // A resize frame 50 ms later replaces it and the wait starts over.
        let t1 = t0 + Duration::from_millis(50);
        s.push(job(Slot::App, 110, t1 + DEBOUNCE));
        assert_eq!(s.next(t0 + DEBOUNCE), Next::Wait(Duration::from_millis(50)));
        let Next::Run(j) = s.next(t1 + DEBOUNCE) else { panic!() };
        assert_eq!(j.req.target.width, 110);
    }

    #[test]
    fn urgent_job_runs_before_a_debounced_one() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.push(job(Slot::App, 100, t0 + DEBOUNCE));
        s.push(job(Slot::NowPlaying, 200, t0));
        let Next::Run(j) = s.next(t0) else { panic!() };
        assert_eq!(j.slot, Slot::NowPlaying);
        assert!(matches!(s.next(t0), Next::Wait(_)));
    }

    #[test]
    fn source_cache_keeps_two_most_recent() {
        let mut c = SourceCache::default();
        let img = Arc::new(image::RgbaImage::new(1, 1));
        let f = |n: &str| Source::File(n.into());
        c.put(f("a"), img.clone());
        c.put(f("b"), img.clone());
        assert!(c.get(&f("a")).is_some()); // a is now the newest
        c.put(f("c"), img);
        assert!(c.get(&f("b")).is_none());
        assert!(c.get(&f("a")).is_some() && c.get(&f("c")).is_some());
    }

    #[test]
    fn headless_show_renders_synchronously() {
        let b = Backdrops::new(None);
        let r = req(Source::Builtin, Effect::Dither, 64);
        let first = b.show(Slot::App, &r).expect("rendered at once");
        assert_eq!(first.pixels.size(), (64, 32));
        assert_eq!(b.show(Slot::App, &r).unwrap().id, first.id, "same request: same picture");
        let second = b.show(Slot::App, &req(Source::Builtin, Effect::Plain, 64)).unwrap();
        assert_ne!(second.id, first.id);
        assert!(b.show(Slot::NowPlaying, &r).is_some());
        b.hide(Slot::App);
        assert!(b.show(Slot::App, &r).is_some_and(|s| s.id != second.id));
    }

    #[test]
    fn headless_never_downloads_and_errors_keep_the_last_image() {
        let b = Backdrops::new(None);
        let ok = req(Source::Builtin, Effect::Plain, 32);
        let shown = b.show(Slot::App, &ok).unwrap();
        let net = req(Source::Url("https://example.invalid/a.jpg".into()), Effect::Plain, 32);
        assert_eq!(b.show(Slot::App, &net).unwrap().id, shown.id);
        assert_eq!(b.error(Slot::App).as_deref(), Some("offline"));
        let missing = req(Source::File("/nonexistent/uzumaki.png".into()), Effect::Plain, 32);
        assert_eq!(b.show(Slot::App, &missing).unwrap().id, shown.id);
        assert!(b.error(Slot::App).unwrap().contains("not found"));
        assert!(b.show(Slot::App, &ok).is_some());
        assert_eq!(b.error(Slot::App), None);
    }

    #[test]
    fn headless_reads_file_urls_and_downscales_huge_pictures() {
        let dir = std::env::temp_dir().join(format!("uzumaki-backdrop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        image::RgbaImage::from_pixel(3300, 40, image::Rgba([200, 100, 50, 255])).save(&path).unwrap();
        let loaded = load(&Source::File(path.clone()), false).unwrap();
        assert_eq!(loaded.width(), MAX_SIDE);
        let b = Backdrops::new(None);
        let url = req(Source::Url(format!("file://{}", path.display())), Effect::Scanlines, 48);
        assert_eq!(b.show(Slot::App, &url).unwrap().pixels.size(), (48, 24));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn late_results_after_hide_are_dropped() {
        let mut b = Backdrops::new(None);
        let r = req(Source::Builtin, Effect::Plain, 32);
        b.show(Slot::App, &r);
        let epoch = b.epochs[0].load(Ordering::Acquire);
        b.hide(Slot::App);
        b.finished(Slot::App, epoch, &r, Ok(Pixels::new()));
        assert!(b.inner.get_mut().slots[0].shown.is_none());
        b.finished(Slot::App, epoch, &r, Err("late".into()));
        assert_eq!(b.error(Slot::App), None);
        // A result for the current generation lands.
        let epoch = b.epochs[0].load(Ordering::Acquire);
        b.finished(Slot::App, epoch, &r, Ok(Pixels::new()));
        let id = b.inner.get_mut().slots[0].shown.as_ref().unwrap().id;
        b.finished(Slot::App, epoch, &r, Ok(Pixels::new()));
        assert_ne!(b.inner.get_mut().slots[0].shown.as_ref().unwrap().id, id);
    }

    /// Now Playing on "Same as app" asks for the content area's exact picture: it takes the
    /// finished one at once (same id, so the same pixels and texture), with no render of its own.
    #[test]
    fn a_slot_takes_a_picture_another_slot_already_shows() {
        let b = Backdrops::new(None);
        let r = req(Source::Builtin, Effect::Halftone, 64);
        let app = b.show(Slot::App, &r).unwrap();
        let epoch = b.epochs[1].load(Ordering::Acquire);
        let np = b.show(Slot::NowPlaying, &r).expect("shown on its first ask");
        assert_eq!(np.id, app.id, "shared, not rendered again");
        assert_eq!(b.epochs[1].load(Ordering::Acquire), epoch + 1, "a render of an older request is dropped");
        // A different request still renders its own; asking for the shared one again shares again.
        let other = b.show(Slot::NowPlaying, &req(Source::Builtin, Effect::Plain, 64)).unwrap();
        assert_ne!(other.id, app.id);
        assert_eq!(b.show(Slot::NowPlaying, &r).unwrap().id, app.id);
        // Hiding one slot leaves the other's picture alone.
        b.hide(Slot::NowPlaying);
        assert_eq!(b.show(Slot::App, &r).unwrap().id, app.id);
    }

    /// Two slots waiting for the same request (a new cover with both on the cover): the first
    /// finished render goes to both, and the other slot's own render is dropped when it lands.
    #[test]
    fn a_finished_render_goes_to_every_slot_asking_for_it() {
        let mut b = Backdrops::new(None);
        let r = req(Source::Builtin, Effect::Plain, 32);
        for st in &mut b.inner.get_mut().slots[..2] {
            st.asked = Some(r.clone()); // both renders on their way
        }
        let np_epoch = b.epochs[1].load(Ordering::Acquire);
        b.finished(Slot::App, b.epochs[0].load(Ordering::Acquire), &r, Ok(Pixels::new()));
        let id = b.shown_id(Slot::App).unwrap();
        assert_eq!(b.shown_id(Slot::NowPlaying), Some(id));
        b.finished(Slot::NowPlaying, np_epoch, &r, Ok(Pixels::new()));
        assert_eq!(b.shown_id(Slot::NowPlaying), Some(id), "its own copy came too late and was dropped");
        assert_eq!(b.shown_id(Slot::Preview), None, "a slot asking for nothing gets nothing");
    }

    /// The theme is part of the request: switching it renders a new picture, and a dark slot
    /// (Now Playing) never takes the light content area's picture.
    #[test]
    fn the_ground_is_part_of_the_key() {
        let b = Backdrops::new(None);
        let dark = req(Source::Builtin, Effect::Dither, 64);
        let light = Request { ground: Ground::Light, ..dark.clone() };
        let app = b.show(Slot::App, &light).unwrap();
        let np = b.show(Slot::NowPlaying, &dark).unwrap();
        assert_ne!(np.id, app.id, "not shared across themes");
        let back = b.show(Slot::App, &dark).unwrap();
        assert_ne!(back.id, app.id, "a theme switch renders again");
        assert_eq!(back.id, np.id, "the dark picture is shared");
    }

    #[test]
    fn hide_on_an_empty_slot_is_free() {
        let b = Backdrops::new(None);
        for _ in 0..5 {
            b.hide(Slot::NowPlaying);
        }
        assert_eq!(b.epochs[1].load(Ordering::Acquire), 0, "no epoch bump while nothing was there");
        b.show(Slot::NowPlaying, &req(Source::Builtin, Effect::Plain, 32));
        b.hide(Slot::NowPlaying);
        b.hide(Slot::NowPlaying);
        assert_eq!(b.epochs[1].load(Ordering::Acquire), 1, "bumped once for the one real hide");
    }
}
