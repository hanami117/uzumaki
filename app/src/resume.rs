//! Resume where you left off (Settings → Playback, on by default).
//!
//! The current song, its position, the play order (the list, Up Next, the place in it) and the
//! session History are kept in `$XDG_STATE_HOME/uzumaki/resume.json` (`~/.local/state/…`, on
//! macOS too): state, not settings. The next start shows them again, paused at that position
//! (`state/resume.rs`); nothing ever plays until the user presses Play.
//!
//! When it is written: the UI thread hands a fresh `Snapshot` over (`publish`) whenever the
//! song, the queue or the play state changes, and every `SAVE_EVERY` while playing; the
//! position follows every time report (`note_position`). A writer thread puts it on disk, off
//! the UI thread, so a crash loses at most half a minute. Quitting writes it once more, from
//! an `atexit` handler (every way of quitting reaches it: Ctrl/Cmd+Q, the tray's Quit, the
//! window's close, macOS ending the app) and, on SIGTERM/SIGINT/SIGHUP (a logout, `kill`), from
//! a signal thread before it asks the app to quit.
//!
//! Only the real app arms this (`arm`); headless runs and tests never write.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::models::Song;

/// While playing, the snapshot is written at least this often.
pub const SAVE_EVERY: Duration = Duration::from_secs(30);
/// At most this many songs per saved list (a "play all" of a big library stays cheap); longer
/// lists keep a window around the current song.
pub const MAX_LIST: usize = 2000;
/// Songs kept before the current one when a list is cut.
const KEEP_BEFORE: usize = 200;
const VERSION: u32 = 1;

/// What is saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    /// The song shown in the player.
    pub song: Song,
    /// Seconds into it.
    pub position: f64,
    /// The list playback started from, in its own order (to undo shuffle).
    pub original: Vec<Song>,
    /// That list in play order.
    pub context: Vec<Song>,
    /// The place in `context` (of `song`, unless `user_playing`).
    pub pos: usize,
    /// Up Next: Play Next / Add to Queue songs.
    pub user: Vec<Song>,
    /// `song` came from `user` (Play Next), so it isn't `context[pos]`.
    pub user_playing: bool,
    /// "Continue playing from …".
    pub source: String,
    pub shuffled: bool,
    /// The session History, oldest first.
    pub history: Vec<Song>,
}

impl Snapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        song: Song,
        position: f64,
        original: &[Song],
        context: &[Song],
        pos: usize,
        user: Vec<Song>,
        user_playing: bool,
        source: String,
        shuffled: bool,
        history: Vec<Song>,
    ) -> Snapshot {
        // A very long list: keep a window around the current song.
        let (context, pos) = if context.len() > MAX_LIST {
            let start = pos.saturating_sub(KEEP_BEFORE).min(context.len() - MAX_LIST);
            (context[start..start + MAX_LIST].to_vec(), pos - start)
        } else {
            (context.to_vec(), pos)
        };
        // Unshuffling a cut list gives the cut list's songs in their own order.
        let original = if original.len() > MAX_LIST {
            if shuffled {
                let mut o: Vec<Song> = original.iter().filter(|s| context.iter().any(|c| c.id == s.id)).cloned().collect();
                o.truncate(MAX_LIST);
                o
            } else {
                context.clone()
            }
        } else {
            original.to_vec()
        };
        let mut user = user;
        user.truncate(MAX_LIST);
        Snapshot { version: VERSION, song, position: position.max(0.0), original, context, pos, user, user_playing, source, shuffled, history }
    }

    /// Ready to show at start, or None when nothing of it can play here. Drops songs Apple said
    /// it can't stream, local files that are gone, and in local only mode every Apple song.
    /// When the song itself is dropped, the next one that is left takes its place, at 0:00.
    pub fn prepare(mut self, local_only: bool, exists: impl Fn(&Path) -> bool) -> Option<Snapshot> {
        if self.version != VERSION {
            return None;
        }
        let keep = |s: &Song| s.playable && if s.is_local() { s.local_path().is_some_and(&exists) } else { !local_only };
        let song_ok = keep(&self.song);
        // The context, with the place re-counted over what is left.
        let pos = self.pos.min(self.context.len());
        let kept_before = self.context[..pos].iter().filter(|s| keep(s)).count();
        let at_pos_kept = self.context.get(pos).is_some_and(&keep);
        self.context.retain(&keep);
        self.original.retain(&keep);
        self.user.retain(&keep);
        self.history.retain(&keep);
        if song_ok {
            self.pos = if self.user_playing {
                // The place after which the queued songs play: the last kept song up to it.
                (kept_before + usize::from(at_pos_kept)).saturating_sub(1)
            } else if at_pos_kept {
                kept_before
            } else {
                // The list no longer has it at its place: play it as a queued song.
                self.user_playing = true;
                kept_before.saturating_sub(1)
            };
            self.position = self.position.max(0.0);
            if self.song.duration > 0.0 && self.position >= self.song.duration {
                self.position = 0.0;
            }
            return Some(self);
        }
        // The song is gone: the next one takes over, from the start.
        self.position = 0.0;
        if let Some(next) = (!self.user.is_empty()).then(|| self.user.remove(0)) {
            self.song = next;
            self.user_playing = true;
            self.pos = (kept_before + usize::from(at_pos_kept)).saturating_sub(1);
            return Some(self);
        }
        // `kept_before` is where the songs after the old place start (the old song itself is
        // gone; with `user_playing` the one at `pos` was already heard).
        let next = if at_pos_kept { kept_before + 1 } else { kept_before };
        let next = self.context.get(next).cloned()?;
        self.pos = if at_pos_kept { kept_before + 1 } else { kept_before };
        self.song = next;
        self.user_playing = false;
        Some(self)
    }
}

/// `$XDG_STATE_HOME/uzumaki/resume.json`, else `~/.local/state/uzumaki/resume.json`.
pub fn path() -> Option<PathBuf> {
    Some(crate::update::state_dir()?.join("resume.json"))
}

pub fn load(path: &Path) -> Option<Snapshot> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomic (temp file, then rename).
pub fn save(path: &Path, snap: &Snapshot) -> std::io::Result<()> {
    let json = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

// ---- the app's copy, written at exit ----

#[derive(Default)]
struct Slot {
    /// None until `arm` (headless, tests: nothing is ever written).
    path: Option<PathBuf>,
    /// What to write; None removes the file (the setting is off, or nothing to resume).
    snap: Option<Arc<Snapshot>>,
    /// The latest position, newer than `snap`'s.
    position: f64,
    /// Something changed since the last write.
    dirty: bool,
    /// `publish` was called at least once: before it, the file on disk is still the last run's
    /// and must be left alone (the app may quit before the restore landed).
    published: bool,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot { path: None, snap: None, position: 0.0, dirty: false, published: false });
/// Serialises writes (the writer thread, the exit handler, the signal thread).
static WRITE: Mutex<()> = Mutex::new(());
/// Wakes the writer thread.
static KICK: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn slot() -> std::sync::MutexGuard<'static, Slot> {
    SLOT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The real app: write to `path` from now on, at exit too. `quit` asks the app to quit (from
/// the signal thread). Once per process.
pub fn arm(path: PathBuf, quit: impl Fn() + Send + 'static) {
    static ONCE: Once = Once::new();
    let mut quit = Some(quit);
    ONCE.call_once(|| {
        slot().path = Some(path);
        std::thread::Builder::new().name("resume-save".into()).spawn(writer).ok();
        // SAFETY: `at_exit` is a plain `extern "C" fn()` that lives for the whole process.
        if unsafe { libc::atexit(at_exit) } != 0 {
            eprintln!("uzumaki: couldn't arrange saving the playback at quit");
        }
        if let Some(q) = quit.take() {
            signals::install(q);
        }
    });
}

/// The UI thread's new snapshot (None: nothing to resume, the file goes). Written soon.
pub fn publish(snap: Option<Snapshot>) {
    {
        let mut s = slot();
        if s.path.is_none() {
            return;
        }
        if let Some(sn) = &snap {
            s.position = sn.position;
        }
        s.snap = snap.map(Arc::new);
        s.dirty = true;
        s.published = true;
    }
    kick();
}

/// The position now (cheap; every time report). Written with the next save or at exit.
pub fn note_position(seconds: f64) {
    let mut s = slot();
    if s.snap.is_some() && (s.position - seconds).abs() >= 0.5 {
        s.position = seconds;
        s.dirty = true;
    }
}

/// Asks the writer thread to write now.
pub fn kick() {
    let (m, c) = &KICK;
    *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
    c.notify_one();
}

fn writer() {
    let (m, c) = &KICK;
    loop {
        {
            let mut due = m.lock().unwrap_or_else(|e| e.into_inner());
            while !*due {
                due = c.wait(due).unwrap_or_else(|e| e.into_inner());
            }
            *due = false;
        }
        flush();
    }
}

/// Writes what changed, now, on this thread.
pub fn flush() {
    let _w = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let (path, snap, position) = {
        let mut s = slot();
        if !s.dirty || !s.published {
            return;
        }
        let Some(path) = s.path.clone() else { return };
        s.dirty = false;
        (path, s.snap.clone(), s.position)
    };
    match snap {
        Some(snap) => {
            let mut snap = (*snap).clone();
            snap.position = position;
            if let Err(e) = save(&path, &snap) {
                eprintln!("uzumaki: couldn't save the playback to resume: {e}");
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

extern "C" fn at_exit() {
    flush();
}

#[cfg(unix)]
mod signals {
    //! SIGTERM, SIGINT and SIGHUP save the playback, then quit the app the normal way. The
    //! handler only writes a byte to a pipe (async-signal-safe); a thread reads it, saves and
    //! asks the app to quit. A second signal, or an app that hasn't quit 5 s later, ends the
    //! process at once. Child processes (Electron, hooks) get the default handlers back at exec.

    use std::sync::atomic::{AtomicI32, Ordering};
    use std::time::Duration;

    static PIPE_W: AtomicI32 = AtomicI32::new(-1);
    const SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

    extern "C" fn on_signal(sig: libc::c_int) {
        let fd = PIPE_W.load(Ordering::Relaxed);
        if fd >= 0 {
            let b = sig as u8;
            // SAFETY: write(2) is async-signal-safe; one byte from a valid buffer.
            unsafe { libc::write(fd, (&b as *const u8).cast(), 1) };
        }
    }

    pub fn install(quit: impl Fn() + Send + 'static) {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: a plain pipe(2) into a two-int array.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return;
        }
        let (r, w) = (fds[0], fds[1]);
        PIPE_W.store(w, Ordering::Relaxed);
        let spawned = std::thread::Builder::new().name("signals".into()).spawn(move || {
            let mut b = 0u8;
            // SAFETY: reads one byte into `b` from our own pipe.
            while unsafe { libc::read(r, (&mut b as *mut u8).cast(), 1) } != 1 {}
            // A second signal now ends the process the default way.
            for s in SIGNALS {
                // SAFETY: restoring the default disposition.
                unsafe { libc::signal(s, libc::SIG_DFL) };
            }
            super::flush();
            quit();
            std::thread::sleep(Duration::from_secs(5));
            std::process::exit(0);
        });
        if spawned.is_err() {
            return;
        }
        for s in SIGNALS {
            // SAFETY: `on_signal` only calls write(2).
            unsafe { libc::signal(s, on_signal as *const () as libc::sighandler_t) };
        }
    }
}

#[cfg(not(unix))]
mod signals {
    pub fn install(_quit: impl Fn() + Send + 'static) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_write_has_the_latest_position_and_off_removes_the_file() {
        let dir = std::env::temp_dir().join(format!("uzumaki-resume-exit-{}", std::process::id()));
        let path = dir.join("resume.json");
        slot().path = Some(path.clone()); // `arm` without its threads and handlers
        flush();
        assert!(!path.exists(), "nothing published yet: the last run's file stays as it is");
        publish(Some(snap(&["a", "b"], 0, &[])));
        note_position(99.0);
        flush(); // what the atexit handler and the signal thread run
        assert_eq!(load(&path).unwrap().position, 99.0);
        note_position(99.2); // under half a second: not worth a write
        assert!(!slot().dirty);
        publish(None);
        flush();
        assert!(!path.exists());
        *slot() = Slot::default();
        let _ = std::fs::remove_dir_all(dir);
    }

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: id.into(),
            artist: "A".into(),
            album: "B".into(),
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

    fn snap(ids: &[&str], pos: usize, user: &[&str]) -> Snapshot {
        let list: Vec<Song> = ids.iter().map(|i| song(i)).collect();
        Snapshot::new(list[pos].clone(), 61.5, &list, &list, pos, user.iter().map(|i| song(i)).collect(), false, "Mix".into(), false, vec![song("h1")])
    }

    fn ids(v: &[Song]) -> Vec<&str> {
        v.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn a_snapshot_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("uzumaki-resume-{}", std::process::id()));
        let path = dir.join("resume.json");
        let s = snap(&["a", "b", "c"], 1, &["u"]);
        save(&path, &s).unwrap();
        assert_eq!(load(&path), Some(s));
        assert!(!path.with_extension("json.tmp").exists(), "written by rename");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path), None, "a broken file is ignored");
        assert_eq!(load(&dir.join("missing.json")), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn long_lists_keep_a_window_around_the_song() {
        let list: Vec<Song> = (0..5000).map(|i| song(&i.to_string())).collect();
        let s = Snapshot::new(list[3000].clone(), 0.0, &list, &list, 3000, vec![], false, String::new(), false, vec![]);
        assert_eq!(s.context.len(), MAX_LIST);
        assert_eq!(s.context[s.pos].id, "3000");
        assert_eq!(s.pos, KEEP_BEFORE);
        assert_eq!(s.original, s.context);
        let s = Snapshot::new(list[4990].clone(), 0.0, &list, &list, 4990, vec![], false, String::new(), false, vec![]);
        assert_eq!(s.context[s.pos].id, "4990");
        assert_eq!(s.context.last().unwrap().id, "4999");
    }

    #[test]
    fn prepare_keeps_everything_that_can_play() {
        let s = snap(&["a", "b", "c"], 1, &["u"]).prepare(false, |_| true).unwrap();
        assert_eq!((s.song.id.as_str(), s.pos, s.position), ("b", 1, 61.5));
        assert_eq!(ids(&s.user), ["u"]);
    }

    #[test]
    fn local_only_keeps_only_local_files() {
        let mut s = snap(&["a", "file:///m/x.flac", "b", "file:///m/y.flac"], 1, &["u", "file:///m/z.flac"]);
        s.history.push(song("file:///m/h.flac"));
        let s = s.prepare(true, |_| true).unwrap();
        assert_eq!(s.song.id, "file:///m/x.flac");
        assert_eq!(ids(&s.context), ["file:///m/x.flac", "file:///m/y.flac"]);
        assert_eq!(s.pos, 0);
        assert_eq!(ids(&s.user), ["file:///m/z.flac"]);
        assert_eq!(ids(&s.history), ["file:///m/h.flac"]);
        assert_eq!(s.position, 61.5);
    }

    #[test]
    fn local_only_with_an_apple_song_moves_on_to_the_next_local_one() {
        let s = snap(&["a", "b", "file:///m/y.flac", "c"], 1, &[]).prepare(true, |_| true).unwrap();
        assert_eq!(s.song.id, "file:///m/y.flac");
        assert_eq!(s.pos, 0);
        assert_eq!(s.position, 0.0, "a different song starts at the beginning");
        assert!(snap(&["a", "b"], 0, &[]).prepare(true, |_| true).is_none(), "nothing local: nothing to resume");
    }

    #[test]
    fn missing_files_and_dead_songs_are_dropped() {
        let mut s = snap(&["a", "file:///gone.flac", "c", "d"], 1, &[]);
        s.context[3].playable = false;
        s.original[3].playable = false;
        let s = s.prepare(false, |p| p != Path::new("/gone.flac")).unwrap();
        assert_eq!(s.song.id, "c", "the next song takes the gone one's place");
        assert_eq!(ids(&s.context), ["a", "c"]);
        assert_eq!(s.pos, 1);
        assert_eq!(s.position, 0.0);
        // The gone song with Up Next: the first queued song plays next.
        let s = snap(&["a", "file:///gone.flac", "c"], 1, &["u", "v"]).prepare(false, |p| p != Path::new("/gone.flac")).unwrap();
        assert_eq!(s.song.id, "u");
        assert!(s.user_playing);
        assert_eq!(ids(&s.user), ["v"]);
        assert_eq!(s.context[s.pos].id, "a", "the list goes on after the place before the gone song");
    }

    #[test]
    fn a_position_past_the_end_starts_over_and_other_versions_are_ignored() {
        let mut s = snap(&["a"], 0, &[]);
        s.position = 500.0;
        assert_eq!(s.clone().prepare(false, |_| true).unwrap().position, 0.0);
        s.version = 99;
        assert!(s.prepare(false, |_| true).is_none());
    }

    #[test]
    fn a_queued_song_playing_keeps_its_place() {
        let list: Vec<Song> = ["a", "b", "c"].iter().map(|i| song(i)).collect();
        let s = Snapshot::new(song("q"), 10.0, &list, &list, 1, vec![], true, String::new(), false, vec![]);
        let s = s.prepare(false, |_| true).unwrap();
        assert!(s.user_playing);
        assert_eq!((s.song.id.as_str(), s.pos), ("q", 1));
    }
}
