//! LRCLIB in the app (`local::lrclib` does the asking): lyrics for a local song without its own
//! when it starts, the two settings (Settings → Local Files), and "Download lyrics for all local
//! songs", which fills the cache one song at a time on a background thread.
//!
//! Nothing is asked while "Download lyrics from LRCLIB" is off, in tests, or in a headless run
//! (`State::lrclib` is None there).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{Load, State};
use crate::local::lrclib::{Failure, Lrclib, Outcome, Query};
use crate::lyrics::Lyrics;
use crate::models::Song;

/// The pause after each song the batch had to ask LRCLIB about (LRCLIB asks for 200–500 ms).
const BATCH_PAUSE: Duration = Duration::from_millis(400);
/// The batch stops after this many 429/5xx in a row (it waits out each one first).
const BATCH_BUSY_MAX: u32 = 5;
/// Songs checked between two progress updates when no request is made (skips are fast).
const PROGRESS_EVERY: usize = 50;

/// What a song start needs to ask LRCLIB: the service, the query, and whether to save beside it.
pub(super) type Ask = Option<(Arc<Lrclib>, Query, bool)>;

/// The file's own lyrics, else LRCLIB's (when `ask`). Second: LRCLIB couldn't answer this time.
pub(super) fn local_lyrics(path: &Path, ask: Ask) -> (Option<Lyrics>, bool) {
    if let Some(l) = crate::local::lyrics::read(path) {
        return (Some(l), false);
    }
    let Some((lr, q, save)) = ask else { return (None, false) };
    match lr.find(path, &q, save) {
        Outcome::Answer { answer, .. } => (answer.lyrics(), false),
        Outcome::Later(f) => {
            if !matches!(f, Failure::Held(_)) {
                eprintln!("uzumaki: lyrics: LRCLIB didn't answer ({f:?}); asking again later");
            }
            (None, true)
        }
    }
}

/// The batch as Settings shows it.
#[derive(Debug, Default)]
pub struct BatchView {
    /// Running: songs checked, of how many, and how many got lyrics from LRCLIB.
    pub running: Option<Progress>,
    /// The last run's result ("Found lyrics for 12 songs.").
    pub note: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub found: usize,
}

impl State {
    /// What `want_lyrics` needs to ask LRCLIB about a local song: None when it is off, in tests
    /// and headless runs, or when the song has no title and artist to ask with.
    pub(super) fn lrclib_ask(&self, song: &Song) -> Ask {
        if !self.settings.lrclib {
            return None;
        }
        let lr = self.lrclib.clone()?;
        Some((lr, Query::of(song)?, self.settings.lrclib_save))
    }

    /// Settings → Local Files → Download lyrics from LRCLIB. Off: lyrics from LRCLIB leave the
    /// screen and the batch stops; on: the playing song is asked at once if it has none.
    pub fn set_lrclib(&mut self, on: bool) {
        if self.settings.lrclib == on {
            return;
        }
        self.settings.lrclib = on;
        self.save_settings();
        if !on {
            self.lrclib_stop();
            self.lyrics.retain(|_, l| !matches!(l, Load::Ready(Some(x)) if x.credit.is_some()));
        }
        if let Some(song) = self.playback.song.clone().filter(Song::is_local) {
            self.want_lyrics(&song);
        }
    }

    /// Settings → Local Files → Save .lrc files next to the songs.
    pub fn set_lrclib_save(&mut self, on: bool) {
        if self.settings.lrclib_save != on {
            self.settings.lrclib_save = on;
            self.save_settings();
        }
    }

    /// "Download lyrics for all local songs": every local song without lyrics of its own and
    /// not in the cache is asked, one at a time with a pause between, on a background thread.
    pub fn lrclib_all(&mut self) {
        if self.lrclib_batch.running.is_some() || !self.settings.lrclib {
            return;
        }
        let (Some(lr), Some(handle)) = (self.lrclib.clone(), self.handle.clone()) else { return };
        let songs: Vec<Song> = self.local_lib.songs.clone();
        if songs.is_empty() {
            self.lrclib_batch.note = Some("No local songs yet.".into());
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let save = self.settings.lrclib_save;
        self.lrclib_batch = BatchView { running: Some(Progress { total: songs.len(), ..Progress::default() }), note: None, cancel: Some(cancel.clone()) };
        let report = handle.clone();
        let progress = move |p: Progress| report.update(move |s: &mut State, _| s.lrclib_progress(p));
        let spawned = std::thread::Builder::new().name("lrclib-all".into()).spawn(move || {
            let note = run_batch(&lr, &songs, save, &cancel, progress);
            handle.update(move |s: &mut State, _| s.lrclib_done(note));
        });
        if spawned.is_err() {
            self.lrclib_batch = BatchView::default();
        }
    }

    /// Stops the batch (Stop, or LRCLIB switched off). Its thread ends after the song it is on.
    pub fn lrclib_stop(&mut self) {
        if let Some(c) = &self.lrclib_batch.cancel {
            c.store(true, Ordering::Relaxed);
        }
    }

    fn lrclib_progress(&mut self, p: Progress) {
        if self.lrclib_batch.running.is_some() {
            self.lrclib_batch.running = Some(p);
        }
    }

    fn lrclib_done(&mut self, note: String) {
        self.lrclib_batch = BatchView { running: None, note: Some(note), cancel: None };
        // The playing song may have just got lyrics.
        if let Some(song) = self.playback.song.clone().filter(|s| s.is_local()) {
            if matches!(self.lyrics_of(&song), Some(Load::Ready(None))) {
                self.want_lyrics(&song);
            }
        }
    }
}

/// The batch's loop (on its thread): returns the note Settings shows afterwards.
fn run_batch(lr: &Lrclib, songs: &[Song], save: bool, cancel: &AtomicBool, progress: impl Fn(Progress)) -> String {
    let mut p = Progress { total: songs.len(), ..Progress::default() };
    let mut since_report = 0;
    let mut stopped: Option<&str> = None;
    'songs: for song in songs {
        if cancel.load(Ordering::Relaxed) {
            stopped = Some("Stopped.");
            break;
        }
        let mut asked = false;
        if let (Some(path), Some(q)) = (song.local_path(), Query::of(song)) {
            if crate::local::lyrics::read(path).is_none() {
                let mut busy = 0;
                loop {
                    // Wait out a hold (a 429's Retry-After), still listening for Stop.
                    while let Some(t) = lr.client.held_until() {
                        if cancel.load(Ordering::Relaxed) {
                            stopped = Some("Stopped.");
                            break 'songs;
                        }
                        std::thread::sleep(t.saturating_duration_since(Instant::now()).min(Duration::from_millis(200)));
                    }
                    match lr.find(path, &q, save) {
                        Outcome::Answer { answer, cached } => {
                            asked = !cached;
                            if answer.lyrics().is_some() {
                                p.found += 1;
                            }
                            break;
                        }
                        Outcome::Later(Failure::Held(_)) => {}
                        Outcome::Later(Failure::Busy(code)) => {
                            busy += 1;
                            if busy >= BATCH_BUSY_MAX {
                                eprintln!("uzumaki: lyrics: LRCLIB busy (HTTP {code}), batch stopped");
                                stopped = Some("LRCLIB is busy. Try again later.");
                                break 'songs;
                            }
                        }
                        Outcome::Later(Failure::Network(e)) => {
                            eprintln!("uzumaki: lyrics: can't reach LRCLIB ({e}), batch stopped");
                            stopped = Some("Couldn\u{2019}t reach lrclib.net. Try again later.");
                            break 'songs;
                        }
                    }
                }
            }
        }
        p.done += 1;
        since_report += 1;
        if asked || since_report >= PROGRESS_EVERY {
            since_report = 0;
            progress(p);
        }
        if asked {
            std::thread::sleep(BATCH_PAUSE);
        }
    }
    progress(p);
    let found = match p.found {
        0 => "No new lyrics found".to_string(),
        1 => "Lyrics for 1 song".to_string(),
        n => format!("Lyrics for {} songs", crate::ui::local_files::thousands(n)),
    };
    match stopped {
        Some(why) => format!("{found} ({} of {} checked). {why}", crate::ui::local_files::thousands(p.done), crate::ui::local_files::thousands(p.total)),
        None => format!("{found}. Every local song checked."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::lrclib::fake::{NOT_FOUND, Server, record};
    use std::fs;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("uzumaki-lrclib-state-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn track(dir: &Path, name: &str, title: &str) -> Song {
        let path = dir.join(name);
        fs::write(&path, b"not really audio").unwrap();
        crate::local::LocalTrack {
            path, mtime: 1, size: 1, title: title.into(), artist: "Ann".into(), album: "First".into(), album_fallback: false,
            album_artist: "Ann".into(), album_artist_tagged: false, compilation: false, track: None, disc: None, year: None,
            genre: None, duration: 100.0, cover: None,
        }
        .to_song()
    }

    /// LRCLIB knows "Known"; everything else is a 404 (search: nothing).
    fn server() -> Server {
        Server::start(|path| {
            if path.contains("track_name=Known") {
                (200, vec![], record(100.0, Some("[00:01.00]From LRCLIB"), None, false).to_string())
            } else if path.starts_with("/api/search") {
                (200, vec![], "[]".into())
            } else {
                (404, vec![], NOT_FOUND.into())
            }
        })
    }

    fn state(srv: &Server, dir: &Path) -> State {
        let mut s = State::offline();
        s.lrclib = Some(Arc::new(Lrclib::new(&srv.base, dir.join("cache"))));
        s
    }

    fn shown(s: &State, song: &Song) -> Option<Lyrics> {
        match s.lyrics_of(song) {
            Some(Load::Ready(l)) => l.clone(),
            other => panic!("not ready: {other:?}"),
        }
    }

    #[test]
    fn a_local_song_without_lyrics_gets_lrclibs_credited_and_asks_once() {
        let d = tmp("start");
        let srv = server();
        let mut s = state(&srv, &d);
        let song = track(&d, "a.mp3", "Known");
        s.want_lyrics(&song);
        let l = shown(&s, &song).expect("lyrics");
        assert_eq!(l.credit, Some(crate::local::lrclib::CREDIT));
        assert_eq!(l.lines[0].text, "From LRCLIB");
        s.lyrics.clear();
        s.want_lyrics(&song);
        assert!(shown(&s, &song).is_some());
        assert_eq!(srv.asked().len(), 1, "the second start reads the cache");
        assert!(!d.join("a.lrc").exists(), "nothing beside the song by default");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn own_lyrics_never_ask_and_off_asks_nothing() {
        let d = tmp("own");
        let srv = server();
        let mut s = state(&srv, &d);
        let song = track(&d, "a.mp3", "Known");
        fs::write(d.join("a.lrc"), "[00:01.00]Mine\n").unwrap();
        s.want_lyrics(&song);
        let l = shown(&s, &song).unwrap();
        assert_eq!((l.lines[0].text.as_str(), l.credit), ("Mine", None));
        fs::remove_file(d.join("a.lrc")).unwrap();
        s.settings.lrclib = false;
        s.want_lyrics(&song);
        assert_eq!(shown(&s, &song), None);
        assert!(srv.asked().is_empty(), "{:?}", srv.asked());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn save_beside_writes_an_lrc_when_on() {
        let d = tmp("save");
        let srv = server();
        let mut s = state(&srv, &d);
        s.settings.lrclib_save = true;
        let song = track(&d, "a.mp3", "Known");
        s.want_lyrics(&song);
        assert_eq!(fs::read_to_string(d.join("a.lrc")).unwrap(), "[00:01.00]From LRCLIB\n");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn offline_is_quiet_and_keeps_what_shows() {
        let d = tmp("offline");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut s = State::offline();
        s.lrclib = Some(Arc::new(Lrclib::new(&format!("http://127.0.0.1:{port}"), d.join("cache"))));
        let song = track(&d, "a.mp3", "Known");
        s.want_lyrics(&song);
        assert_eq!(shown(&s, &song), None, "no lyrics, no error");
        // Lyrics already showing stay when a later ask can't get through.
        let l = Lyrics { synced: false, lines: vec![crate::lyrics::Line { text: "x".into(), ..Default::default() }], credit: Some("c") };
        s.lyrics.insert(song.id.clone(), Load::Ready(Some(l)));
        s.want_lyrics(&song);
        assert!(shown(&s, &song).is_some());
        assert_eq!(s.lrclib.as_ref().unwrap().cache.get(song.local_path().unwrap()), None, "nothing cached");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn switching_off_drops_lrclib_lyrics() {
        let d = tmp("off");
        let srv = server();
        let mut s = state(&srv, &d);
        let song = track(&d, "a.mp3", "Known");
        s.want_lyrics(&song);
        assert!(shown(&s, &song).is_some());
        s.set_lrclib(false);
        assert!(s.lyrics_of(&song).is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_batch_skips_own_and_cached_songs_and_reports() {
        let d = tmp("batch");
        let srv = server();
        let lr = Lrclib::new(&srv.base, d.join("cache"));
        let known = track(&d, "a.mp3", "Known");
        let unknown = track(&d, "b.mp3", "Unknown");
        let own = track(&d, "c.mp3", "Known");
        fs::write(d.join("c.txt"), "Own words").unwrap();
        let songs = vec![known.clone(), unknown, own];
        let seen = std::sync::Mutex::new(Vec::new());
        let note = run_batch(&lr, &songs, false, &AtomicBool::new(false), |p| seen.lock().unwrap().push(p));
        assert_eq!(note, "Lyrics for 1 song. Every local song checked.");
        let asked = srv.asked();
        assert_eq!(asked.iter().filter(|a| a.starts_with("/api/get")).count(), 2, "own lyrics skipped: {asked:?}");
        assert_eq!(seen.lock().unwrap().last(), Some(&Progress { done: 3, total: 3, found: 1 }));
        // Again: everything is in the cache now, nothing is sent.
        let note = run_batch(&lr, &songs, false, &AtomicBool::new(false), |_| {});
        assert_eq!(note, "Lyrics for 1 song. Every local song checked.");
        assert_eq!(srv.asked().len(), asked.len());
        // Stop before the first song.
        let note = run_batch(&lr, &songs, false, &AtomicBool::new(true), |_| {});
        assert!(note.ends_with("(0 of 3 checked). Stopped."), "{note}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_batch_stops_when_offline() {
        let d = tmp("batch-off");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let lr = Lrclib::new(&format!("http://127.0.0.1:{port}"), d.join("cache"));
        let songs = vec![track(&d, "a.mp3", "Known"), track(&d, "b.mp3", "Other")];
        let note = run_batch(&lr, &songs, false, &AtomicBool::new(false), |_| {});
        assert!(note.contains("Couldn\u{2019}t reach lrclib.net"), "{note}");
        let _ = fs::remove_dir_all(&d);
    }
}
