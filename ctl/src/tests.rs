//! Drives the command runner through a real `Client` against a fake app on a temp socket.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use uzumaki_ctl::{serve, Client, CollectionInfo, Kind, PlayState, Queue, Reply, Request, Server, SongInfo, Status};

use crate::cli;
use crate::run::{self, Env, Fail, Term, Timing};

pub(crate) fn song(i: usize) -> SongInfo {
    let (title, artist) = [("Lose Yourself to Dance", "Daft Punk"), ("Instant Crush", "Daft Punk"), ("Around the World", "Daft Punk"), ("Midnight City", "M83")][i % 4];
    SongInfo { id: format!("s{i}"), title: title.into(), artist: artist.into(), album: "Album".into(), duration: 200.0 + i as f64 * 7.0, playable: i != 5, ..Default::default() }
}

fn collection(kind: Kind, id: &str, title: &str, subtitle: &str, favorites: bool) -> CollectionInfo {
    CollectionInfo { kind, id: id.into(), title: title.into(), subtitle: subtitle.into(), count: Some(6), favorites, editable: kind == Kind::Playlist, year: None, pinned: None }
}

#[derive(Default)]
struct Fake {
    current: usize,
    /// (song to switch to, `Status` polls left before it does): MusicKit switches late.
    pending: Option<(usize, u32)>,
    /// Paused (`Toggle` flips it); like MusicKit, the reply to `Toggle` still shows the old state.
    paused: bool,
}

fn fake_handler() -> impl Fn(Request) -> Reply + Send + Sync + 'static {
    fake_handler_logged(Arc::default())
}

/// The canned app; every request it gets is also pushed to `log`.
pub(crate) fn fake_handler_logged(log: Arc<Mutex<Vec<Request>>>) -> impl Fn(Request) -> Reply + Send + Sync + 'static {
    let state = Mutex::new(Fake::default());
    move |req| {
        let mut f = state.lock().unwrap();
        log.lock().unwrap().push(req.clone());
        let status = |f: &Fake| Status {
            state: if f.paused { PlayState::Paused } else { PlayState::Playing },
            song: Some(song(f.current)),
            position: 83.0,
            duration: song(f.current).duration,
            volume: 0.8,
            source: "Anima ;)".into(),
            index: Some(f.current),
            count: 6,
            queued: 2,
            connected: true,
            signed_in: true,
            ..Default::default()
        };
        match req {
            Request::Status => {
                if let Some((to, polls)) = f.pending {
                    if polls == 0 {
                        f.current = to;
                        f.pending = None;
                    } else {
                        f.pending = Some((to, polls - 1));
                    }
                }
                Reply::Status(status(&f))
            }
            Request::Toggle => {
                let before = status(&f);
                f.paused = !f.paused;
                Reply::Status(before)
            }
            Request::Next => {
                f.pending = Some((f.current + 1, 2));
                Reply::Status(status(&f))
            }
            Request::PlayCollection { index, .. } => {
                f.pending = Some((index.unwrap_or(0), 1));
                Reply::Status(status(&f))
            }
            Request::Playlists => Reply::Collections {
                items: vec![
                    collection(Kind::Playlist, "p.fav", "Favourite Songs", "", true),
                    collection(Kind::Playlist, "p.anima", "Anima ;)", "", false),
                    collection(Kind::Playlist, "p.road", "Road Trip", "", false),
                    collection(Kind::Playlist, "p.road2", "Road Trip Two", "", false),
                ],
            },
            Request::Albums => Reply::Collections {
                items: vec![collection(Kind::Album, "l.1", "Discovery", "Daft Punk", false), collection(Kind::Album, "l.2", "Hurry Up, We're Dreaming", "M83", false)],
            },
            Request::Tracks { kind, id } => Reply::Tracks { collection: collection(kind, &id, "Anima ;)", "", false), songs: (0..6).map(song).collect() },
            Request::Queue => Reply::Queue(Queue { current: Some(song(f.current)), up_next: (1..30).map(song).collect(), queued: 2, source: "Anima ;)".into(), ..Default::default() }),
            Request::Search { .. } => Reply::Songs { songs: (0..6).map(song).collect() },
            Request::Move { .. } | Request::Remove { .. } => Reply::Queue(Queue { current: Some(song(f.current)), up_next: (1..4).map(song).collect(), queued: 0, ..Default::default() }),
            Request::Seek { .. } | Request::Pick { .. } | Request::Jump { .. } | Request::Prev | Request::Pause => Reply::Status(status(&f)),
            Request::Quit => Reply::Status(status(&f)),
            Request::Lyrics => fake_lyrics(&song(f.current).id),
            _ => Reply::Error { message: "not in the fake".into() },
        }
    }
}

/// Three timed lines (the first with word timing, the third sung by the second voice).
pub(crate) fn fake_lyrics(song_id: &str) -> Reply {
    use uzumaki_ctl::{LyricLine, LyricWord, LyricsStatus};
    let w = |start: f64, end: f64, text: &str| LyricWord { start, end, text: text.into() };
    Reply::Lyrics {
        song_id: song_id.into(),
        status: LyricsStatus::Synced,
        lines: vec![
            LyricLine { start: 10.0, end: 14.0, text: "one two three four".into(), second_voice: false, words: vec![w(10.0, 11.0, "one "), w(11.0, 12.0, "two "), w(12.0, 13.0, "three "), w(13.0, 14.0, "four")] },
            LyricLine { start: 14.0, end: 18.0, text: "second line here".into(), ..Default::default() },
            LyricLine { start: 18.0, end: 20.0, text: "answer".into(), second_voice: true, ..Default::default() },
        ],
    }
}

pub(crate) fn socket(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("uzu-test-{}-{name}", std::process::id())).join("ctl.sock")
}

pub(crate) fn start_fake(name: &str) -> (Server, PathBuf) {
    let path = socket(name);
    (serve(&path, fake_handler()).unwrap(), path)
}

fn last_file(socket: &Path) -> PathBuf {
    let mut p = socket.as_os_str().to_owned();
    p.push(".last");
    PathBuf::from(p)
}

fn drive(path: &Path, line: &str) -> (Result<(), Fail>, String) {
    let args: Vec<String> = line.split_whitespace().map(String::from).collect();
    let inv = cli::parse(&args).unwrap();
    let stop = AtomicBool::new(false);
    let timing = Timing { poll: Duration::from_millis(5), wait: Duration::from_millis(500), watch: Duration::from_millis(10) };
    let env = Env { term: Term { width: 80, color: false }, timing, stop: &stop, last: Some(last_file(path)) };
    let mut client = Client::connect_to(path).unwrap();
    let mut out = Vec::new();
    let result = run::run(&inv, &mut client, &env, &mut out);
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn commands_against_a_fake_app() {
    let (_server, path) = start_fake("integration");

    let (r, out) = drive(&path, "status");
    assert!(r.is_ok());
    assert!(out.starts_with("Daft Punk - Lose Yourself to Dance\n[playing] #1/6   1:23/3:20 (41%)   "), "{out}");

    // `status --line` is one line from a format; the fake plays "Lose Yourself to Dance" at 1:23/3:20.
    let (r, out) = drive(&path, "status --format={artist}:{title}:{elapsed}/{duration}:{percent}%");
    assert!(r.is_ok());
    assert_eq!(out, "Daft Punk:Lose Yourself to Dance:1:23/3:20:41%\n");
    let (_, out) = drive(&path, "status --line");
    assert_eq!(out, "Daft Punk — Lose Yourself to Dance\n", "no format anywhere: the built-in one");

    // `next` answers before the song changes; the runner polls until it has.
    let (r, out) = drive(&path, "next");
    assert!(r.is_ok());
    assert!(out.starts_with("Daft Punk - Instant Crush\n"), "{out}");

    let (_, out) = drive(&path, "-q next");
    assert_eq!(out, "");

    let (_, out) = drive(&path, "playlist anima");
    assert!(out.starts_with("Anima ;) — 6 songs, "), "{out}");
    assert!(out.contains("▶ 3  Around the World"), "{out}");

    let (r, out) = drive(&path, "playlist road");
    assert_eq!(out, "");
    let fail = r.unwrap_err();
    assert_eq!(fail.code, 1);
    assert!(fail.msg.contains("matches several playlists") && fail.msg.contains("Road Trip Two"), "{}", fail.msg);

    let (_, out) = drive(&path, "play road trip two 3");
    assert!(out.contains("[playing]"), "{out}");

    let (_, out) = drive(&path, "albums hurry");
    assert!(out.starts_with("  2  Hurry Up, We're Dreaming  M83"), "{out}");

    let (_, out) = drive(&path, "queue");
    assert!(out.starts_with("Now playing\n  "), "{out}");
    assert!(out.contains("Queued\n   1  ") && out.contains("Continue playing from Anima ;)\n"), "{out}");
    assert!(out.trim_end().ends_with("… and 9 more (uzu queue -a)"), "{out}");

    let (_, out) = drive(&path, "search daft");
    assert!(out.contains("uzu pick <n>"), "{out}");

    let (r, _) = drive(&path, "show");
    assert_eq!(r.unwrap_err().msg, "uzu: not in the fake");
}

/// The requests the fake got after `from`, minus the status polls.
fn acted(log: &Mutex<Vec<Request>>, from: usize) -> Vec<Request> {
    log.lock().unwrap()[from..].iter().filter(|r| !matches!(r, Request::Status | Request::Playlists | Request::Albums | Request::Queue)).cloned().collect()
}

#[test]
fn play_number_follows_the_last_list() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let path = socket("lastlist");
    let _server = serve(&path, fake_handler_logged(log.clone())).unwrap();
    let mark = |log: &Mutex<Vec<Request>>| log.lock().unwrap().len();

    // No record: playlist n, as before.
    let m = mark(&log);
    drive(&path, "play 3").0.unwrap();
    assert_eq!(acted(&log, m), [Request::PlayCollection { kind: Kind::Playlist, id: "p.road".into(), index: None, order: None }]);

    // After a search, the number is a result.
    drive(&path, "search daft").0.unwrap();
    let m = mark(&log);
    drive(&path, "play 2").0.unwrap();
    assert_eq!(acted(&log, m), [Request::Pick { indices: vec![1], action: cli_play() }]);
    // ... but `play 3 5` and `play -p 2` keep their meaning.
    let m = mark(&log);
    drive(&path, "play 3 5").0.unwrap();
    drive(&path, "play -p 2").0.unwrap();
    drive(&path, "play album 2").0.unwrap();
    assert_eq!(
        acted(&log, m),
        [
            Request::PlayCollection { kind: Kind::Playlist, id: "p.road".into(), index: Some(4), order: None },
            Request::PlayCollection { kind: Kind::Playlist, id: "p.anima".into(), index: None, order: None },
            Request::PlayCollection { kind: Kind::Album, id: "l.2".into(), index: None, order: None },
        ]
    );

    // A filtered album list keeps its numbering.
    drive(&path, "albums hurry").0.unwrap();
    let m = mark(&log);
    drive(&path, "play 2").0.unwrap();
    assert_eq!(acted(&log, m), [Request::PlayCollection { kind: Kind::Album, id: "l.2".into(), index: None, order: None }]);
    let (r, _) = drive(&path, "play 3");
    assert!(r.unwrap_err().msg.contains("only 2 albums"));

    // A collection's songs: play from song n.
    drive(&path, "playlist anima").0.unwrap();
    let m = mark(&log);
    drive(&path, "play 4").0.unwrap();
    assert_eq!(acted(&log, m), [Request::PlayCollection { kind: Kind::Playlist, id: "p.anima".into(), index: Some(3), order: None }]);

    // The queue: jump to the song's current place in up next.
    drive(&path, "queue").0.unwrap();
    let m = mark(&log);
    drive(&path, "play 3").0.unwrap();
    assert_eq!(acted(&log, m), [Request::Jump { index: 2 }]);
    let (r, _) = drive(&path, "play 25");
    assert!(r.unwrap_err().msg.contains("only 20 songs"));
    // A song that left the queue (the fake's queue is s1..s29; pretend s40 was printed).
    super::last::save(&last_file(&path), &super::last::Last::Queue { ids: vec!["s40".into()] });
    let (r, _) = drive(&path, "play 1");
    assert_eq!(r.unwrap_err().msg, "uzu: that song is no longer in the queue");
}

fn cli_play() -> uzumaki_ctl::PickAction {
    uzumaki_ctl::PickAction::Play
}

#[test]
fn jump_by_title_move_and_del() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let path = socket("queueedit");
    let _server = serve(&path, fake_handler_logged(log.clone())).unwrap();
    // The fake's up next is s1.. : Instant Crush, Around the World, Midnight City, Lose Yourself, ...
    let (r, _) = drive(&path, "jump midnight");
    assert!(r.is_ok());
    assert!(acted(&log, 0).contains(&Request::Jump { index: 2 }));
    let (r, _) = drive(&path, "jump daft punk");
    assert!(r.unwrap_err().msg.contains("no song in Up Next matches 'daft punk'"));
    let (_, _) = drive(&path, "jump lose");
    assert!(acted(&log, 0).contains(&Request::Jump { index: 3 })); // first in queue order

    let m = log.lock().unwrap().len();
    let (r, out) = drive(&path, "move 3 1");
    assert!(r.is_ok());
    assert_eq!(acted(&log, m), [Request::Move { from: 2, to: 0 }]);
    assert!(out.starts_with("Now playing\n") && out.contains("Continue playing"), "{out}");
    let m = log.lock().unwrap().len();
    let (_, out) = drive(&path, "-q del 2 1");
    assert_eq!(out, "");
    assert_eq!(acted(&log, m), [Request::Remove { indices: vec![1, 0] }]);
}

#[test]
fn watch_draws_until_stopped_and_restores_the_cursor() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let path = socket("watch");
    let _server = serve(&path, fake_handler_logged(log.clone())).unwrap();
    let inv = cli::parse(&["watch".to_string()]).unwrap();
    let stop = AtomicBool::new(false);
    let env = Env { term: Term { width: 60, color: false }, timing: Timing { watch: Duration::from_millis(5), ..Timing::default() }, stop: &stop, last: None };
    let mut client = Client::connect_to(&path).unwrap();
    let mut out = Vec::new();
    std::thread::scope(|s| {
        // Stopped once a third status is asked: by then two frames were drawn (the second
        // redraws over the first), however slowly the loop runs.
        s.spawn(|| {
            while log.lock().unwrap().iter().filter(|r| matches!(r, Request::Status)).count() < 3 {
                std::thread::sleep(Duration::from_millis(5));
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        run::run(&inv, &mut client, &env, &mut out).unwrap();
    });
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("\x1b[?25l") && text.ends_with("\x1b[?25h"));
    assert!(text.contains("\x1b[3F\x1b[2KDaft Punk - "), "{text:?}");
}

/// The fake app with "Road Trip Two" pinned on top and "Anima ;)" second. The reply keeps the
/// old order on purpose: uzu puts pinned playlists first whatever order they arrive in.
pub(crate) fn pinned_handler(log: Arc<Mutex<Vec<Request>>>) -> impl Fn(Request) -> Reply + Send + Sync + 'static {
    let inner = fake_handler_logged(log);
    move |req| match inner(req) {
        Reply::Collections { mut items } if items.iter().any(|c| c.kind == Kind::Playlist) => {
            for c in items.iter_mut() {
                c.pinned = match c.id.as_str() {
                    "p.road2" => Some(0),
                    "p.anima" => Some(1),
                    _ => None,
                };
            }
            Reply::Collections { items }
        }
        reply => reply,
    }
}

#[test]
fn pinned_playlists_print_first_and_numbers_follow() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let path = socket("pinned");
    let _server = serve(&path, pinned_handler(log.clone())).unwrap();
    let mark = |log: &Mutex<Vec<Request>>| log.lock().unwrap().len();

    let (_, out) = drive(&path, "playlists");
    assert_eq!(out, "  1  Road Trip Two    pinned  6 songs\n  2  Anima ;)         pinned  6 songs\n  3  Favourite Songs          6 songs\n  4  Road Trip                6 songs\n");
    let m = mark(&log);
    drive(&path, "play 1").0.unwrap();
    drive(&path, "play -p 4").0.unwrap();
    // Two match "road": the pinned one wins.
    drive(&path, "play road").0.unwrap();
    assert_eq!(
        acted(&log, m),
        [
            Request::PlayCollection { kind: Kind::Playlist, id: "p.road2".into(), index: None, order: None },
            Request::PlayCollection { kind: Kind::Playlist, id: "p.road".into(), index: None, order: None },
            Request::PlayCollection { kind: Kind::Playlist, id: "p.road2".into(), index: None, order: None },
        ]
    );
    // No record of a list: playlist n is still the printed n.
    let _ = std::fs::remove_file(last_file(&path));
    let m = mark(&log);
    drive(&path, "play 2").0.unwrap();
    assert_eq!(acted(&log, m), [Request::PlayCollection { kind: Kind::Playlist, id: "p.anima".into(), index: None, order: None }]);
}

#[test]
fn offline_listing_puts_the_settings_pins_first() {
    let dir = std::env::temp_dir().join(format!("uzu-offline-pins-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("playlists.json"), r#"{"saved_at":1,"data":[{"id":"p.1","title":"Mine","track_count":4},{"id":"p.2","title":"Favourite Songs","favorites":true},{"id":"p.3","title":"Gym"}]}"#).unwrap();
    std::fs::write(dir.join("settings.json"), r#"{"pins":[{"kind":"Playlist","id":"p.3","title":"Gym"},{"kind":"Album","id":"p.1","title":"Not a playlist"}]}"#).unwrap();
    let inv = cli::parse(&["playlists".to_string()]).unwrap();
    let stop = AtomicBool::new(false);
    let env = Env { term: Term { width: 60, color: false }, timing: Timing::default(), stop: &stop, last: None };
    let mut out = Vec::new();
    run::run(&inv, &mut run::Offline { dir: dir.clone(), settings: Some(dir.join("settings.json")) }, &env, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "(Uzumaki isn't running — from the cache)\n  1  Gym              pinned\n  2  Favourite Songs\n  3  Mine                     4 songs\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offline_listing_uses_the_cache_note() {
    let dir = std::env::temp_dir().join(format!("uzu-offline-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("playlists.json"), r#"{"saved_at":1,"data":[{"id":"p.1","title":"Mine","track_count":4},{"id":"p.2","title":"Favourite Songs","favorites":true}]}"#).unwrap();
    let inv = cli::parse(&["playlists".to_string()]).unwrap();
    let stop = AtomicBool::new(false);
    let env = Env { term: Term { width: 60, color: false }, timing: Timing::default(), stop: &stop, last: None };
    let mut out = Vec::new();
    run::run(&inv, &mut run::Offline { dir: dir.clone(), settings: None }, &env, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "(Uzumaki isn't running — from the cache)\n  1  Favourite Songs\n  2  Mine             4 songs\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Not a test: `UZU_FAKE_SOCKET=/tmp/uzu-fake/ctl.sock cargo test -p uzumaki-ctl fake_server -- --ignored`
/// serves the canned app for ten minutes so the real `uzu` binary can be tried against it.
#[test]
#[ignore]
fn fake_server_for_manual_checks() {
    let path = PathBuf::from(std::env::var_os("UZU_FAKE_SOCKET").expect("set UZU_FAKE_SOCKET"));
    let _server = serve(&path, fake_handler()).unwrap();
    std::thread::sleep(Duration::from_secs(600));
}

#[test]
fn quit_replies_then_the_app_goes_away() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let path = socket("quit");
    let server = serve(&path, fake_handler_logged(log.clone())).unwrap();
    // The fake app stops listening shortly after it has answered Quit.
    let watcher = {
        let log = log.clone();
        std::thread::spawn(move || {
            while !log.lock().unwrap().contains(&Request::Quit) {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(100));
            drop(server);
        })
    };
    let mut out = Vec::new();
    let was_running = crate::quit(&path, Duration::from_secs(5), &mut out).unwrap();
    watcher.join().unwrap();
    assert!(was_running);
    assert_eq!(String::from_utf8(out).unwrap(), "Uzumaki quit.\n");
    assert_eq!(acted(&log, 0), [Request::Quit]);
    assert!(Client::connect_to(&path).is_err());
}

#[test]
fn quit_fails_when_the_app_stays_up() {
    let (_server, path) = start_fake("quitstuck");
    // Quit answers, but this fake never stops listening.
    let mut out = Vec::new();
    let err = crate::quit(&path, Duration::from_millis(300), &mut out).unwrap_err();
    assert!(err.msg.contains("still running"), "{}", err.msg);
    assert!(out.is_empty());
}

#[test]
fn quit_with_nothing_running_says_so() {
    let path = socket("quitnone");
    let mut out = Vec::new();
    assert!(!crate::quit(&path, Duration::from_millis(100), &mut out).unwrap());
    assert_eq!(String::from_utf8(out).unwrap(), "Uzumaki isn't running.\n");
}
