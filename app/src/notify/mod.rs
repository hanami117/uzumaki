//! "Notify on song change" (Settings → General): a desktop notification with the title,
//! "artist — album" and the cover when a new song starts and Uzumaki isn't in front.
//!
//! The song change itself is found by `hooks::Hooks::observe` (one detector for both); `State`
//! calls `Notifier::song_changed` when it says a new song began. Nothing here blocks the UI
//! thread: `song_changed` only bumps a counter and starts a thread that sleeps out the
//! debounce, waits briefly for the cover in the art cache and talks to the system.
//!
//! Rules, in the order they are applied:
//! - the setting is on;
//! - the song is playing or loading (the song restored at start-up, paused, is not news);
//! - after `DEBOUNCE` it is still the newest change (skipping fast sends nothing but the last);
//! - the window is not focused at that moment (hidden, minimised, or another app in front).
//!
//! Off for headless runs, tests and `UZUMAKI_TEST_INSTANCE`. The system part: `linux.rs`
//! (org.freedesktop.Notifications over D-Bus, each song replaces the previous notification)
//! and `mac.rs` (UNUserNotificationCenter, which needs the .app bundle).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::thread;

use crate::bridge::PlayState;
use crate::models::{Artwork, Song};

// The D-Bus message is built (and tested) everywhere; only Linux sends it.
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod linux;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(not(target_os = "macos"))]
use linux::Sys;
#[cfg(target_os = "macos")]
use mac::Sys;

/// How long a song must stay current before it is announced.
const DEBOUNCE: Duration = Duration::from_millis(1500);
/// How long to wait for the cover to land in the art cache (after the debounce).
const COVER_WAIT: Duration = Duration::from_millis(1000);
const POLL: Duration = Duration::from_millis(25);

/// What a notification says.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub title: String,
    /// "Artist — Album".
    pub body: String,
    /// The cover as a file, when it was in the art cache.
    pub cover: Option<PathBuf>,
}

/// Title and body of a song's notification; None when it has no title to show.
fn text(song: &Song) -> Option<(String, String)> {
    let title = song.title.trim();
    if title.is_empty() {
        return None;
    }
    let (artist, album) = (song.artist.trim(), song.album.trim());
    let body = match (artist.is_empty(), album.is_empty()) {
        (false, false) => format!("{artist} \u{2014} {album}"),
        (false, true) => artist.to_string(),
        (true, false) => album.to_string(),
        (true, true) => String::new(),
    };
    Some((title.to_string(), body))
}

/// The window rule: only when the setting is on and the window is not focused.
fn should_show(enabled: bool, window_focused: bool) -> bool {
    enabled && !window_focused
}

/// A song that has just started (or is about to), not one sitting paused or stopped.
fn is_starting(state: PlayState) -> bool {
    matches!(state, PlayState::Playing | PlayState::Loading | PlayState::Waiting | PlayState::Stalled | PlayState::Seeking)
}

/// The system side.
trait Backend: Send + Sync {
    /// Shows (or replaces) the song notification. Never panics, fails silently.
    fn send(&self, note: &Note);
    /// Asks for permission up front (the Mac prompt), when the setting is switched on.
    fn prepare(&self) {}
}

struct Inner {
    backend: Box<dyn Backend>,
    enabled: AtomicBool,
    focused: AtomicBool,
    /// Bumped by every song change; a pending notification is sent only if it is still the latest.
    latest: AtomicU64,
    debounce: Duration,
    cover_wait: Duration,
    /// Workers still deciding (debouncing, waiting for the cover, sending). Tests wait for zero.
    active: AtomicUsize,
}

impl Inner {
    fn is_latest(&self, generation: u64) -> bool {
        self.latest.load(Ordering::SeqCst) == generation
    }

    fn wanted(&self) -> bool {
        should_show(self.enabled.load(Ordering::SeqCst), self.focused.load(Ordering::SeqCst))
    }

    /// The worker: debounce, re-check, wait for the cover, send.
    fn announce(&self, generation: u64, title: String, body: String, art: Option<Artwork>) {
        thread::sleep(self.debounce);
        if !self.is_latest(generation) || !self.wanted() {
            return;
        }
        let mut cover = None;
        if let Some(art) = &art {
            let start = Instant::now();
            loop {
                cover = crate::art::cached_file(art);
                if cover.is_some() || start.elapsed() >= self.cover_wait || !self.is_latest(generation) {
                    break;
                }
                thread::sleep(POLL);
            }
        }
        // The song may have changed, or the window come back, while the cover was awaited.
        if self.is_latest(generation) && self.wanted() {
            self.backend.send(&Note { title, body, cover });
        }
    }
}

/// Owned by `State`.
pub struct Notifier {
    inner: Option<Arc<Inner>>,
}

impl Notifier {
    /// No notifications at all (headless runs, tests, test instances).
    pub fn off() -> Self {
        Self { inner: None }
    }

    /// The real one, unless this is a test instance.
    pub fn for_app() -> Self {
        let test = std::env::var_os("UZUMAKI_TEST_INSTANCE").is_some_and(|v| !v.is_empty());
        if test {
            return Self::off();
        }
        Self::with_backend(Box::new(Sys::new()), DEBOUNCE, COVER_WAIT)
    }

    fn with_backend(backend: Box<dyn Backend>, debounce: Duration, cover_wait: Duration) -> Self {
        let inner = Inner {
            backend,
            enabled: AtomicBool::new(false),
            focused: AtomicBool::new(false),
            latest: AtomicU64::new(0),
            debounce,
            cover_wait,
            active: AtomicUsize::new(0),
        };
        Self { inner: Some(Arc::new(inner)) }
    }

    /// The setting. Switching it on asks the system for permission (on a worker thread).
    pub fn set_enabled(&self, on: bool) {
        let Some(inner) = &self.inner else { return };
        let was = inner.enabled.swap(on, Ordering::SeqCst);
        if on && !was {
            let inner = inner.clone();
            let _ = thread::Builder::new().name("notify-prepare".into()).spawn(move || inner.backend.prepare());
        }
    }

    /// Whether the Uzumaki window has focus (`App::on_focus`).
    pub fn set_focused(&self, focused: bool) {
        if let Some(inner) = &self.inner {
            inner.focused.store(focused, Ordering::SeqCst);
        }
    }

    /// A new song began (as found by the hooks' detector). Returns at once.
    pub fn song_changed(&self, song: &Song, state: PlayState) {
        let Some(inner) = &self.inner else { return };
        // Every change supersedes the pending one, even when this one is not announced.
        let generation = inner.latest.fetch_add(1, Ordering::SeqCst) + 1;
        if !is_starting(state) || !inner.wanted() {
            return;
        }
        let Some((title, body)) = text(song) else { return };
        let art = song.artwork.clone();
        inner.active.fetch_add(1, Ordering::SeqCst);
        let worker = inner.clone();
        let spawned = thread::Builder::new().name("notify".into()).spawn(move || {
            worker.announce(generation, title, body, art);
            worker.active.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            inner.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Mock(Arc<Mutex<Vec<Note>>>);

    impl Backend for Mock {
        fn send(&self, note: &Note) {
            self.0.lock().unwrap().push(note.clone());
        }
    }

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: format!("Title {id}"),
            artist: "Some Artist".into(),
            album: "Some Album".into(),
            duration: 200.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: vec![],
        }
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    fn notifier(debounce: u64, cover_wait: u64) -> (Notifier, Arc<Mutex<Vec<Note>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let n = Notifier::with_backend(Box::new(Mock(log.clone())), MS(debounce), MS(cover_wait));
        n.set_enabled(true);
        (n, log)
    }

    fn titles(log: &Mutex<Vec<Note>>) -> Vec<String> {
        log.lock().unwrap().iter().map(|n| n.title.clone()).collect()
    }

    #[test]
    fn the_body_is_artist_and_album() {
        let s = song("1");
        assert_eq!(text(&s), Some(("Title 1".into(), "Some Artist \u{2014} Some Album".into())));
        let only_artist = Song { album: " ".into(), ..s.clone() };
        assert_eq!(text(&only_artist).unwrap().1, "Some Artist");
        let only_album = Song { artist: String::new(), ..s.clone() };
        assert_eq!(text(&only_album).unwrap().1, "Some Album");
        let neither = Song { artist: String::new(), album: String::new(), ..s.clone() };
        assert_eq!(text(&neither).unwrap().1, "");
        assert_eq!(text(&Song { title: "  ".into(), ..s }), None, "nothing to show without a title");
    }

    #[test]
    fn the_focus_rule() {
        assert!(should_show(true, false), "on and not in front");
        assert!(!should_show(true, true), "the window is in front");
        assert!(!should_show(false, false), "off");
        assert!(!should_show(false, true));
    }

    #[test]
    fn only_a_starting_song_is_news() {
        assert!(is_starting(PlayState::Playing) && is_starting(PlayState::Loading));
        assert!(!is_starting(PlayState::Paused) && !is_starting(PlayState::Stopped) && !is_starting(PlayState::None));
    }

    /// Waits (generously) until every worker has decided: sent, or dropped its song.
    fn settle(n: &Notifier) {
        let inner = n.inner.as_ref().unwrap();
        let end = Instant::now() + Duration::from_secs(60);
        while inner.active.load(Ordering::SeqCst) > 0 {
            assert!(Instant::now() < end, "the notification workers never finished");
            thread::sleep(MS(5));
        }
    }

    // The debounces below are long next to what the tests do between two calls, so a slow
    // machine can't make a quick skip look like a song left playing. Results are read once
    // every worker has finished (`settle`), never after a guessed sleep.

    #[test]
    fn a_song_is_announced_after_the_debounce_not_before() {
        let (n, log) = notifier(400, 0);
        let start = Instant::now();
        n.song_changed(&song("1"), PlayState::Loading);
        assert!(titles(&log).is_empty(), "too early");
        settle(&n);
        assert!(start.elapsed() >= MS(400), "sent before the debounce ended");
        assert_eq!(titles(&log), ["Title 1"]);
        assert_eq!(log.lock().unwrap()[0].body, "Some Artist \u{2014} Some Album");
    }

    #[test]
    fn skipping_fast_sends_only_the_last_song() {
        let (n, log) = notifier(2000, 0);
        for i in 1..=5 {
            n.song_changed(&song(&i.to_string()), PlayState::Playing);
            thread::sleep(MS(20));
        }
        settle(&n);
        assert_eq!(titles(&log), ["Title 5"]);
    }

    #[test]
    fn a_change_that_is_not_announced_still_cancels_the_pending_one() {
        let (n, log) = notifier(2000, 0);
        n.song_changed(&song("1"), PlayState::Playing);
        thread::sleep(MS(30));
        n.song_changed(&song("2"), PlayState::Paused); // not news, but song 1 is no longer current
        settle(&n);
        assert!(titles(&log).is_empty());
    }

    #[test]
    fn nothing_when_off_or_in_front() {
        let (n, log) = notifier(30, 0);
        n.set_enabled(false);
        n.song_changed(&song("1"), PlayState::Playing);
        n.set_enabled(true);
        n.set_focused(true);
        n.song_changed(&song("2"), PlayState::Playing);
        settle(&n);
        assert!(titles(&log).is_empty());
        n.set_focused(false);
        n.song_changed(&song("3"), PlayState::Playing);
        settle(&n);
        assert_eq!(titles(&log), ["Title 3"]);
    }

    #[test]
    fn coming_back_to_the_window_during_the_wait_cancels_it() {
        let (n, log) = notifier(2000, 0);
        n.song_changed(&song("1"), PlayState::Playing);
        thread::sleep(MS(40));
        n.set_focused(true);
        settle(&n);
        assert!(titles(&log).is_empty());
    }

    #[test]
    fn off_and_the_restored_song_send_nothing() {
        let n = Notifier::off();
        n.set_enabled(true);
        n.set_focused(false);
        n.song_changed(&song("1"), PlayState::Playing); // no backend: nothing happens, no panic
        let (n, log) = notifier(20, 0);
        n.song_changed(&song("1"), PlayState::Paused); // restored at start-up
        settle(&n);
        assert!(titles(&log).is_empty());
    }

    #[test]
    fn the_cover_is_waited_for_and_attached_when_it_lands() {
        let dir = std::env::temp_dir().join(format!("uzumaki-notify-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cover = dir.join("cover.jpg");
        let mut s = song("1");
        s.artwork = Some(Artwork { template: format!("file://{}", cover.display()), bg: None });
        // A wait far longer than the landing, however slow the machine.
        let (n, log) = notifier(30, 60_000);
        n.song_changed(&s, PlayState::Playing);
        let later = cover.clone();
        thread::spawn(move || {
            thread::sleep(MS(200));
            std::fs::write(later, b"jpg").unwrap();
        });
        settle(&n);
        assert_eq!(log.lock().unwrap()[0].cover, Some(cover));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cover_that_never_comes_is_sent_without() {
        let mut s = song("1");
        s.artwork = Some(Artwork { template: "file:///nonexistent/uzumaki/missing.jpg".into(), bg: None });
        let (n, log) = notifier(20, 1500);
        let t = Instant::now();
        n.song_changed(&s, PlayState::Playing);
        assert!(t.elapsed() < MS(750), "the UI thread must not wait ({:?})", t.elapsed());
        settle(&n);
        assert_eq!(log.lock().unwrap().len(), 1);
        assert_eq!(log.lock().unwrap()[0].cover, None);
    }
}
