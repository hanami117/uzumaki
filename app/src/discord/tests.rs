//! The worker against a fake Discord: a Unix socket in a scratch folder that speaks the IPC
//! protocol. The real Discord's sockets are never touched (the worker is given only this path).

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::ipc::{self, OP_FRAME, OP_HANDSHAKE, OP_PING, OP_PONG};
use super::*;

const WAIT: Duration = Duration::from_secs(5);
const ID: &str = "123456789012345678";

#[derive(Debug)]
enum Seen {
    Handshake(Value),
    Frame(Value),
    Pong(Value),
    Closed,
}

struct Fake {
    dir: PathBuf,
    seen: Receiver<Seen>,
    conns: Arc<Mutex<Vec<UnixStream>>>,
    stop: Arc<AtomicBool>,
}

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    // Short: a Unix socket path may not be longer than about 100 bytes.
    let dir = PathBuf::from(format!("/tmp/uzdc-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

impl Fake {
    /// Listens on `<dir>/discord-ipc-0` and answers like Discord.
    fn start(dir: &Path) -> Self {
        let (tx, seen) = channel();
        let listener = UnixListener::bind(dir.join("discord-ipc-0")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let conns: Arc<Mutex<Vec<UnixStream>>> = Default::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (c2, s2) = (conns.clone(), stop.clone());
        thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                let Ok((mut s, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                s.set_nonblocking(false).unwrap();
                c2.lock().unwrap().push(s.try_clone().unwrap());
                let tx = tx.clone();
                thread::spawn(move || {
                    let Ok((op, hello)) = ipc::read_frame(&mut s) else { return };
                    assert_eq!(op, OP_HANDSHAKE);
                    tx.send(Seen::Handshake(hello)).ok();
                    let ready = json!({"cmd": "DISPATCH", "evt": "READY", "data": {"v": 1, "user": {"username": "fake"}}});
                    s.write_all(&ipc::encode(OP_FRAME, &ready)).unwrap();
                    // Discord pings an idle connection now and then.
                    s.write_all(&ipc::encode(OP_PING, &json!({"n": 1}))).unwrap();
                    loop {
                        match ipc::read_frame(&mut s) {
                            Ok((OP_FRAME, v)) => {
                                let reply = json!({"cmd": "SET_ACTIVITY", "evt": null, "data": {}, "nonce": v["nonce"]});
                                tx.send(Seen::Frame(v)).ok();
                                let _ = s.write_all(&ipc::encode(OP_FRAME, &reply));
                            }
                            Ok((OP_PONG, v)) => drop(tx.send(Seen::Pong(v))),
                            Ok(_) => {}
                            Err(_) => {
                                tx.send(Seen::Closed).ok();
                                return;
                            }
                        }
                    }
                });
            }
        });
        Fake { dir: dir.to_path_buf(), seen, conns, stop }
    }

    fn next(&self) -> Seen {
        self.seen.recv_timeout(WAIT).expect("the fake Discord heard nothing")
    }

    /// The next thing that is not the pong to our ping.
    fn next_event(&self) -> Seen {
        loop {
            match self.next() {
                Seen::Pong(_) => {}
                other => return other,
            }
        }
    }

    fn frame(&self) -> Value {
        match self.next_event() {
            Seen::Frame(v) => v,
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    fn quiet(&self, ms: u64) {
        match self.seen.recv_timeout(Duration::from_millis(ms)) {
            Ok(Seen::Pong(_)) | Err(_) => {}
            Ok(other) => panic!("expected silence, got {other:?}"),
        }
    }

    /// Hangs up on every client, like Discord quitting.
    fn kick(&self) {
        for c in self.conns.lock().unwrap().drain(..) {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.kick();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env(dir: &Path) -> Env {
    Env {
        paths: vec![dir.join("discord-ipc-0")],
        reconnect: Duration::from_millis(60),
        min_gap: Duration::ZERO,
        idle_poll: Duration::from_millis(20),
        io_timeout: Duration::from_secs(2),
        at_exit: false,
    }
}

fn on() -> Prefs {
    Prefs { enabled: true, client_id: Some(ID.into()), ..Prefs::default() }
}

fn song() -> Snapshot {
    Snapshot {
        title: "Redbone".into(),
        artist: "Childish Gambino".into(),
        album: "Awaken, My Love!".into(),
        album_is_fallback: false,
        artwork_url: Some("https://is1-ssl.mzstatic.com/image/thumb/x/512x512bb.jpg".into()),
        song_url: Some("https://music.apple.com/us/album/redbone/1?i=2".into()),
        playing: true,
        position: 10.0,
        duration: 200.0,
    }
}

fn activity_of(frame: &Value) -> Option<&Value> {
    frame["args"].get("activity")
}

#[test]
fn handshake_then_set_activity_with_the_song() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    assert!(p.active());
    p.update_at(Some(&song()), 5_000);
    match fake.next_event() {
        Seen::Handshake(h) => assert_eq!(h, json!({"v": 1, "client_id": ID})),
        other => panic!("{other:?}"),
    }
    let f = fake.frame();
    assert_eq!(f["cmd"], "SET_ACTIVITY");
    assert_eq!(f["args"]["pid"], std::process::id());
    assert!(f["nonce"].is_string());
    assert_eq!(
        activity_of(&f).unwrap(),
        &json!({
            "type": 2,
            "details": "Redbone",
            "state": "Childish Gambino",
            "assets": {
                "large_image": "https://is1-ssl.mzstatic.com/image/thumb/x/512x512bb.jpg",
                "large_text": "Awaken, My Love!",
                "small_image": "uzumaki",
                "small_text": "Uzumaki"
            },
            "timestamps": {"start": 4_990, "end": 5_190},
            "buttons": [{"label": "Open in Apple Music", "url": "https://music.apple.com/us/album/redbone/1?i=2"}]
        })
    );
    p.close(WAIT);
}

#[test]
fn pings_are_answered() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    p.update_at(Some(&song()), 5_000);
    let mut pong = None;
    while let Ok(s) = fake.seen.recv_timeout(Duration::from_millis(500)) {
        if let Seen::Pong(v) = s {
            pong = Some(v);
            break;
        }
    }
    assert_eq!(pong, Some(json!({"n": 1})), "the ping's payload comes back as a pong");
    p.close(WAIT);
}

#[test]
fn a_seek_is_sent_and_clock_drift_is_not() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    p.update_at(Some(&song()), 5_000);
    fake.next_event();
    fake.frame();
    // One second later, one second further: the same bar.
    p.update_at(Some(&Snapshot { position: 11.0, ..song() }), 5_001);
    fake.quiet(200);
    // A jump forward.
    p.update_at(Some(&Snapshot { position: 100.0, ..song() }), 5_002);
    let f = fake.frame();
    assert_eq!(activity_of(&f).unwrap()["timestamps"]["start"], 4_902);
    p.close(WAIT);
}

#[test]
fn pause_clears_play_sets_again_and_disabling_clears() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    p.update_at(Some(&song()), 5_000);
    fake.next_event(); // the handshake
    assert!(activity_of(&fake.frame()).is_some());
    // Paused, with "hide when paused" (the default): the card goes.
    p.update_at(Some(&Snapshot { playing: false, ..song() }), 5_010);
    let clear = fake.frame();
    assert_eq!(clear["cmd"], "SET_ACTIVITY");
    assert!(clear["args"].get("activity").is_none(), "a clear has no activity: {clear}");
    // Playing again.
    p.update_at(Some(&song()), 5_020);
    assert!(activity_of(&fake.frame()).is_some());
    // The setting goes off: cleared, hung up, no worker left.
    p.configure(&Prefs { enabled: false, ..on() });
    assert!(!p.active());
    assert!(activity_of(&fake.frame()).is_none());
    assert!(matches!(fake.next_event(), Seen::Closed));
}

#[test]
fn stopped_and_quitting_clear_the_card() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    p.update_at(Some(&song()), 5_000);
    fake.next_event();
    fake.frame();
    p.update_at(None, 5_001); // stopped
    assert!(activity_of(&fake.frame()).is_none());
    p.update_at(Some(&song()), 5_002);
    assert!(activity_of(&fake.frame()).is_some());
    p.close(WAIT); // quitting
    assert!(activity_of(&fake.frame()).is_none(), "the card is cleared before the socket closes");
    assert!(matches!(fake.next_event(), Seen::Closed));
}

#[test]
fn nothing_playing_means_no_card_and_no_connection_to_clear() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    p.update_at(None, 5_000);
    p.update_at(Some(&Snapshot { playing: false, ..song() }), 5_001);
    fake.quiet(250); // not even a handshake: there was never anything to show
    p.close(WAIT);
    fake.quiet(100);
}

#[test]
fn it_connects_when_discord_appears_and_again_when_it_goes_and_comes_back() {
    let dir = scratch();
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    p.update_at(Some(&song()), 5_000);
    // Discord isn't running yet: nothing happens, quietly.
    thread::sleep(Duration::from_millis(200));
    assert!(p.active());
    let fake = Fake::start(&dir);
    assert!(matches!(fake.next_event(), Seen::Handshake(_)), "found within a reconnect interval");
    let first = fake.frame();
    assert_eq!(activity_of(&first).unwrap()["details"], "Redbone");
    // Discord quits and starts again.
    fake.kick();
    assert!(matches!(fake.next_event(), Seen::Closed));
    assert!(matches!(fake.next_event(), Seen::Handshake(_)), "it reconnects by itself");
    assert_eq!(activity_of(&fake.frame()).unwrap()["details"], "Redbone", "and shows the song again");
    p.close(WAIT);
}

#[test]
fn updates_are_spaced_and_only_the_newest_is_sent() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut e = env(&dir);
    e.min_gap = Duration::from_millis(400);
    let mut p = Presence::with_env(e);
    p.configure(&on());
    let named = |t: &str| Snapshot { title: t.into(), ..song() };
    p.update_at(Some(&named("One")), 5_000);
    fake.next_event();
    assert_eq!(activity_of(&fake.frame()).unwrap()["details"], "One");
    // Skipping through songs: Two and Three arrive inside the gap.
    p.update_at(Some(&named("Two")), 5_000);
    p.update_at(Some(&named("Three")), 5_000);
    assert_eq!(activity_of(&fake.frame()).unwrap()["details"], "Three");
    fake.quiet(600);
    p.close(WAIT);
}

#[test]
fn a_changed_choice_is_applied_to_the_card_on_show() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&on());
    p.update_at(Some(&song()), 5_000);
    fake.next_event();
    fake.frame();
    p.configure(&Prefs { hide_title: true, ..on() });
    p.update_at(Some(&song()), 5_000);
    let f = fake.frame();
    assert_eq!(activity_of(&f).unwrap()["details"], "Listening to music");
    p.close(WAIT);
}

#[test]
fn off_without_the_setting_or_an_application_id() {
    let dir = scratch();
    let fake = Fake::start(&dir);
    let mut p = Presence::with_env(env(&dir));
    p.configure(&Prefs { enabled: false, ..on() });
    assert!(!p.active(), "off: no thread");
    p.update_at(Some(&song()), 5_000);
    fake.quiet(150);
    // On, but with no usable id (this build's own is a placeholder until the real one is set).
    p.configure(&Prefs { enabled: true, client_id: Some("not an id".into()), ..Prefs::default() });
    assert_eq!(p.active(), CLIENT_ID.bytes().any(|b| b != b'0'));
    p.configure(&on());
    assert!(p.active());
    p.configure(&Prefs { client_id: Some("987654321098765432".into()), ..on() });
    assert!(p.active(), "a different id restarts the worker");
    p.close(WAIT);
    assert!(!Presence::off().active());
    let mut never = Presence::off();
    never.configure(&on());
    assert!(!never.active(), "headless, tests and test instances never start a worker");
}

#[test]
fn the_client_id_is_the_configured_one_when_it_is_one() {
    let p = |id: Option<&str>| Prefs { client_id: id.map(String::from), ..Prefs::default() };
    assert_eq!(p(Some(" 123456789012345678 ")).effective_client_id().as_deref(), Some("123456789012345678"));
    for bad in ["", "abc", "12345", "000000000000000000", "1234567890123456789012345678"] {
        assert_eq!(p(Some(bad)).effective_client_id().as_deref(), p(None).effective_client_id().as_deref(), "{bad:?} falls back to the app's");
    }
    assert_eq!(p(None).available(), p(None).effective_client_id().is_some());
}
