//! The controller against a stand-in for cliairplay (`scripts/airplay/fake-cliairplay.py`): no
//! network, no speaker, no audio device. The tests play the bridge (they connect to the PCM
//! socket and write PCM) and the local player (a `LocalFeed`).

#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use uzumaki_airplay::discover::{Device, Service};
use uzumaki_airplay::{Cast, Config, Connect, Controller, Event, Need, Speaker, Why};

struct Rig {
    ctl: Controller,
    events: mpsc::Receiver<Event>,
    dir: PathBuf,
    seen: Vec<Event>,
}

fn device(name: &str, model: &str, extra: &[(&str, &str)]) -> Device {
    let mut txt = vec![("features".to_string(), "0x200,0x0".to_string()), ("model".to_string(), model.to_string()), ("deviceid".to_string(), format!("id-{}", name.to_lowercase()))];
    txt.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    Device { name: name.into(), ip: "127.0.0.1".parse().unwrap(), hostname: "localhost.".into(), airplay: Some(Service { instance: name.into(), port: 7000, txt }), raop: None }
}

/// A controller whose helper is the stand-in in `mode`, with these devices instead of mDNS.
fn rig(test: &str, mode: &str, devices: Vec<Device>) -> Rig {
    rig_with(test, mode, devices, false)
}

/// `rig`, with the shared PTP clock (the stand-in's `--ptp-daemon` only waits to be ended).
fn rig_with(test: &str, mode: &str, devices: Vec<Device>, ptp_daemon: bool) -> Rig {
    let dir = std::env::temp_dir().join(format!("uzu-ap-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/airplay/fake-cliairplay.py");
    let helper = dir.join("cliairplay");
    let script = format!(
        "#!/bin/sh\nFAKE_AIRPLAY='{mode}' FAKE_AIRPLAY_JOURNAL='{}' FAKE_AIRPLAY_RAW='{}' FAKE_AIRPLAY_DIR='{}' exec python3 '{}' \"$@\"\n",
        dir.join("journal.txt").display(),
        dir.join("pcm.bin").display(),
        dir.display(),
        fake.display()
    );
    std::fs::write(&helper, script).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = Config {
        helper: Some(helper),
        dir: dir.join("run"),
        seed: 0x1234,
        debug: None,
        extra: vec![],
        keep_local: false,
        // Room for a slow python start on a loaded machine; only "hang" waits it out.
        connect_timeout: Duration::from_secs(if mode == "hang" { 2 } else { 10 }),
        retry_backoff: vec![Duration::from_millis(300), Duration::from_millis(300)],
        // Wide enough that a loaded machine still writes "the audio on its way" inside it.
        flush_drop: Duration::from_millis(1500),
        discover_timeout: Duration::from_secs(1),
        stall_grace: Duration::from_millis(600),
        capture_timeout: Duration::from_millis(1500),
        fake: devices,
        ptp_daemon,
        // Never the real clock's port: an installed app casting on this computer answers there.
        daemon_control: "127.0.0.1:1".into(),
    };
    let (tx, events) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let ctl = Controller::new(cfg, move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    // Most tests play an app without a bridge (nothing to capture); `with_bridge` is the app with one.
    ctl.set_bridge(false);
    Rig { ctl, events, dir, seen: Vec::new() }
}

/// A rig whose app runs a bridge: the speaker plays only once the bridge's capture is ready.
fn with_bridge(test: &str, devices: Vec<Device>) -> Rig {
    let r = rig(test, "", devices);
    r.ctl.set_bridge(true);
    r
}

impl Rig {
    /// Waits for an event `want` accepts; the ones before it are kept in `seen`.
    fn wait(&mut self, what: &str, want: impl Fn(&Event) -> bool) -> Event {
        let t = Instant::now();
        loop {
            let left = Duration::from_secs(10).checked_sub(t.elapsed()).unwrap_or_else(|| panic!("timed out waiting for {what}; saw {:?}\njournal:\n{}", self.seen, self.journal()));
            match self.events.recv_timeout(left) {
                Ok(e) => {
                    self.seen.push(e.clone());
                    if want(&e) {
                        return e;
                    }
                }
                Err(_) => panic!("timed out waiting for {what}; saw {:?}\njournal:\n{}", self.seen, self.journal()),
            }
        }
    }

    fn journal(&self) -> String {
        std::fs::read_to_string(self.dir.join("journal.txt")).unwrap_or_default()
    }

    /// One speaker's own journal.
    fn journal_of(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(format!("{name}.txt"))).unwrap_or_default()
    }

    /// The frames one speaker received, as (left, right).
    fn frames_of(&self, name: &str) -> Vec<(i16, i16)> {
        let bytes = std::fs::read(self.dir.join(format!("{name}.pcm"))).unwrap_or_default();
        bytes.as_chunks::<4>().0.iter().map(|b| (i16::from_le_bytes([b[0], b[1]]), i16::from_le_bytes([b[2], b[3]]))).collect()
    }

    /// Waits for the group to be exactly these speakers, all playing.
    fn playing(&mut self, names: &[&str]) {
        let want: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        self.wait(&format!("{names:?} to play"), |e| matches!(e, Event::Speakers(list) if list.iter().all(|s| s.playing) && list.iter().map(|s| s.name.clone()).collect::<Vec<_>>() == want));
    }

    fn samples(&self) -> Vec<i16> {
        std::fs::read(self.dir.join("pcm.bin")).unwrap_or_default().as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect()
    }

    /// The bridge: connects to the PCM socket once it is there, and says its capture is ready.
    fn bridge(&self) -> UnixStream {
        let s = self.socket();
        self.ctl.capture_ready(true, None);
        s
    }

    /// Connects to the PCM socket only (the capture hasn't said it's ready).
    fn socket(&self) -> UnixStream {
        let socket = self.ctl.socket();
        let mut s = None;
        wait_for("the PCM socket", || {
            s = UnixStream::connect(&socket).ok();
            s.is_some()
        });
        s.unwrap()
    }

    fn on(&mut self) {
        self.wait("the speaker to play", |e| matches!(e, Event::State(Cast::On { .. })));
    }

    fn end(self) {
        self.ctl.shutdown();
        assert!(!self.ctl.socket().exists(), "the socket is gone");
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pcm(v: i16, frames: usize) -> Vec<u8> {
    std::iter::repeat_n(v.to_le_bytes(), frames * 2).flatten().collect()
}

fn opts() -> Connect {
    Connect { volume: 30, password: None, auth: None }
}

#[test]
fn picking_a_speaker_plays_and_picking_this_computer_stops() {
    let mut r = with_bridge("basic", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    // Nothing exists until asked.
    assert!(!r.ctl.socket().exists());
    r.ctl.browse(true);
    let Event::Devices(list) = r.wait("the device list", |e| matches!(e, Event::Devices(_))) else { unreachable!() };
    assert_eq!(list.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), vec!["Bedroom"]);
    r.ctl.browse(false);
    assert!(r.ctl.connect(&list[0].id, opts()));
    assert!(!r.ctl.connect("nobody", opts()));
    r.wait("connecting", |e| matches!(e, Event::State(Cast::Connecting { name, .. }) if name == "Bedroom"));
    r.wait("the capture request", |e| *e == Event::Capture(true));
    let feed = r.ctl.local_feed();
    assert!(feed.casting() && feed.mute_local(), "connected: this computer goes quiet");
    let mut bridge = r.bridge();
    bridge.write_all(&pcm(0x1111, 4096)).unwrap();
    let Event::State(Cast::On { latency_ms, .. }) = r.wait("the speaker to play", |e| matches!(e, Event::State(Cast::On { .. }))) else { unreachable!() };
    assert!(latency_ms.is_some_and(|l| l > 0 && l < 5000), "latency {latency_ms:?}");
    let j = r.journal();
    assert!(j.contains("--protocol auto") && j.contains("--volume 30") && j.contains("--dacp 0000000000001234"), "{j}");
    assert!(j.contains("cmd START_UNIX_MS=0\ncmd ACTION=START"), "{j}");

    // The speaker's volume.
    r.ctl.set_volume(55);
    wait_for("the volume command", || r.journal().contains("cmd VOLUME=55"));

    // Back to this computer.
    r.ctl.disconnect();
    r.wait("the capture to stop", |e| *e == Event::Capture(false));
    r.wait("off", |e| *e == Event::State(Cast::Off));
    assert!(r.journal().contains("cmd ACTION=STOP"), "{}", r.journal());
    assert!(!feed.casting() && !feed.mute_local(), "this computer plays again");
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", r.seen);
    wait_for("the socket to go", || !r.ctl.socket().exists());
    r.end();
}

#[test]
fn a_seek_flushes_and_drops_the_audio_on_its_way() {
    let mut r = with_bridge("flush", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    r.ctl.flush(); // no speaker: nothing
    r.ctl.connect_named("bedroom", opts());
    r.wait("the capture request", |e| *e == Event::Capture(true));
    let mut bridge = r.bridge();
    bridge.write_all(&pcm(0x1111, 4096)).unwrap();
    r.on();
    wait_for("the first audio", || r.samples().iter().filter(|&&s| s == 0x1111).count() == 8192);
    // The seek: what is still on its way (0x2222) belongs to the old position.
    r.ctl.flush();
    wait_for("the flush command", || r.journal().contains("cmd ACTION=FLUSH"));
    bridge.write_all(&pcm(0x2222, 2048)).unwrap();
    std::thread::sleep(Duration::from_millis(1600));
    bridge.write_all(&pcm(0x3333, 4096)).unwrap();
    wait_for("the audio after the seek", || r.samples().iter().filter(|&&s| s == 0x3333).count() == 8192);
    // The speaker is started again on the new audio (and shown as playing throughout).
    wait_for("the second start", || r.journal().matches("status started").count() == 2);
    assert_eq!(r.journal().matches("cmd ACTION=START").count(), 2, "{}", r.journal());
    assert!(!r.samples().contains(&0x2222), "the audio of the old position was dropped");
    assert_eq!(r.journal().matches("args ").count(), 1, "one helper for the whole session");
    r.end();
}

#[test]
fn one_session_serves_the_bridge_and_the_local_player_in_turn() {
    let mut r = with_bridge("turns", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    let local = r.ctl.local_feed();
    assert!(!local.mute_local(), "no speaker yet: this computer plays");
    r.ctl.connect_named("Bedroom", opts());
    r.wait("the capture request", |e| *e == Event::Capture(true));
    let mut bridge = r.bridge();
    // 1. Apple Music plays: the bridge's samples arrive, also when split mid-frame.
    let a = pcm(0x1111, 4096);
    bridge.write_all(&a[..4097]).unwrap();
    bridge.write_all(&a[4097..]).unwrap();
    wait_for("the bridge's audio", || r.samples().iter().filter(|&&s| s == 0x1111).count() == 8192);
    r.on();
    // 2. A local file takes over: its samples arrive, the bridge's (paused, 0x3333) are dropped.
    local.set_active(true);
    local.push(vec![0x2222; 4096 * 2]);
    wait_for("the local audio", || r.samples().iter().filter(|&&s| s == 0x2222).count() == 8192);
    bridge.write_all(&pcm(0x3333, 4096)).unwrap();
    // 3. Local paused: silence keeps the stream going.
    let before = r.samples().len();
    wait_for("silence while the local player is paused", || r.samples().len() >= before + 2 * 4410);
    assert!(r.samples()[before..].iter().all(|&s| s == 0), "only silence while paused");
    // 4. Back to Apple Music on the same session.
    local.set_active(false);
    std::thread::sleep(Duration::from_millis(100));
    bridge.write_all(&pcm(0x4444, 4096)).unwrap();
    wait_for("the bridge's audio again", || r.samples().iter().filter(|&&s| s == 0x4444).count() == 8192);
    let all = r.samples();
    assert!(!all.contains(&0x3333), "the bridge's audio was dropped while the local player was in charge");
    let first = |v: i16| all.iter().position(|&s| s == v).unwrap();
    assert!(first(0x1111) < first(0x2222) && first(0x2222) < first(0x4444), "in order");
    assert_eq!(r.journal().matches("args ").count(), 1, "one helper session for both");
    r.end();
    assert!(!local.casting() && !local.mute_local(), "stopped: this computer plays again");
}

#[test]
fn with_no_player_silence_starts_the_speaker_and_keeps_it() {
    let mut r = rig("silence", "", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    // No bridge and no local file: the speaker still starts (it would give up after two minutes of nothing).
    r.on();
    wait_for("half a second of silence", || r.samples().len() >= 44100);
    assert!(r.samples().iter().all(|&s| s == 0));
    r.end();
}

#[test]
fn switching_speakers_ends_the_first_session_before_the_second() {
    let mut r = rig("switch", "", vec![device("Bedroom", "AudioAccessory5,1", &[]), device("Kitchen", "Sonos One", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    r.on();
    r.ctl.connect_named("Kitchen", opts());
    r.wait("the second speaker", |e| matches!(e, Event::State(Cast::On { name, .. }) if name == "Kitchen"));
    let order: Vec<String> = r
        .seen
        .iter()
        .filter_map(|e| match e {
            Event::State(Cast::Off) => Some("off".into()),
            Event::State(Cast::Connecting { name, .. }) => Some(format!("connecting {name}")),
            Event::State(Cast::On { name, .. }) => Some(format!("on {name}")),
            Event::Capture(on) => Some(format!("capture {on}")),
            _ => None,
        })
        .collect();
    // (A speaker found by name says "connecting" twice: by name, then with its id.)
    let mut order = order;
    order.dedup();
    assert_eq!(order, ["connecting Bedroom", "capture true", "on Bedroom", "capture false", "off", "connecting Kitchen", "capture true", "on Kitchen"]);
    let j = r.journal();
    assert_eq!(j.matches("args ").count(), 2, "{j}");
    assert_eq!(j.matches("cmd ACTION=STOP").count(), 1, "{j}");
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. })));
    r.end();
}

#[test]
fn the_speaker_shows_as_playing_only_once_the_bridge_captures() {
    // The first-try bug: the helper started and the bridge sent (silent) PCM, but its tap had not
    // caught MusicKit's <audio>, so the music played on this computer while the picker said "Bedroom".
    let mut r = with_bridge("ready-late", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    r.wait("the capture request", |e| *e == Event::Capture(true));
    let mut bridge = r.socket();
    bridge.write_all(&pcm(0, 4096)).unwrap();
    wait_for("the helper to start", || r.journal().contains("cmd ACTION=START"));
    std::thread::sleep(Duration::from_millis(500));
    while let Ok(e) = r.events.try_recv() {
        r.seen.push(e);
    }
    assert!(!r.seen.iter().any(|e| matches!(e, Event::State(Cast::On { .. }))), "started, but the capture isn't ready: still connecting; {:?}", r.seen);
    // The page tapped the element.
    r.ctl.capture_ready(true, None);
    r.on();
    // The page reloads: connecting again until its new graph is ready, then playing again.
    r.ctl.capture_ready(false, Some("page reloaded"));
    r.wait("connecting again", |e| matches!(e, Event::State(Cast::Connecting { .. })));
    r.ctl.capture_ready(true, None);
    r.on();
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", r.seen);
    assert_eq!(r.journal().matches("args ").count(), 1, "the same session throughout");
    r.end();
}

#[test]
fn a_capture_that_never_gets_ready_falls_back_to_this_computer_with_an_error() {
    let mut r = with_bridge("never-ready", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    r.wait("the capture request", |e| *e == Event::Capture(true));
    let mut bridge = r.socket();
    bridge.write_all(&pcm(0, 4096)).unwrap();
    r.ctl.capture_ready(false, Some("could not tap <audio id=apple-music-player>"));
    let Event::Failed { why, name, still } = r.wait("the failure", |e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert_eq!((why, name.as_str()), (Why::NoAudio, "Bedroom"));
    assert!(still.is_empty());
    r.wait("off", |e| *e == Event::State(Cast::Off));
    assert!(r.seen.contains(&Event::Capture(false)), "the bridge stops capturing: {:?}", r.seen);
    assert!(!r.seen.iter().any(|e| matches!(e, Event::State(Cast::On { .. }))), "never shown as playing: {:?}", r.seen);
    let feed = r.ctl.local_feed();
    assert!(!feed.casting() && !feed.mute_local(), "back on this computer");
    assert!(Why::NoAudio.message("Bedroom").contains("Bedroom"));
    r.end();
}

#[test]
fn ready_without_the_socket_is_not_enough_and_a_new_session_waits_for_its_own_word() {
    let mut r = with_bridge("ready-no-socket", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    // A word from before the session doesn't count.
    r.ctl.capture_ready(true, None);
    r.ctl.connect_named("Bedroom", opts());
    r.wait("the capture request", |e| *e == Event::Capture(true));
    let Event::Failed { why, .. } = r.wait("the failure", |e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert_eq!(why, Why::NoAudio, "no bridge on the socket and no fresh word");
    assert!(!r.seen.iter().any(|e| matches!(e, Event::State(Cast::On { .. }))), "{:?}", r.seen);
    r.end();
}

fn failure(test: &str, mode: &str, dev: Device, started: bool) -> (Why, Vec<Event>) {
    let mut r = rig(test, mode, vec![dev]);
    r.ctl.connect_named("Bedroom", opts());
    if started {
        r.on();
    }
    let Event::Failed { why, name, still } = r.wait("the failure", |e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert_eq!(name, "Bedroom");
    assert!(still.is_empty(), "nothing else plays: {still:?}");
    r.wait("off", |e| *e == Event::State(Cast::Off));
    let feed = r.ctl.local_feed();
    assert!(!feed.casting() && !feed.mute_local(), "back on this computer");
    let seen = r.seen.clone();
    r.end();
    (why, seen)
}

#[test]
fn a_speaker_that_is_off_or_never_answers_is_unreachable() {
    let hp = || device("Bedroom", "AudioAccessory5,1", &[]);
    assert_eq!(failure("off", "connect_failed", hp(), false).0, Why::Unreachable);
    let (why, seen) = failure("hang", "hang", hp(), false);
    assert_eq!(why, Why::Unreachable);
    assert!(!seen.contains(&Event::Capture(true)), "never connected: the bridge was never asked to capture");
}

/// The HomePod mini "Bedroom" while an iPhone plays to it (its TXT flags as seen live).
fn busy_homepod() -> Device {
    device("Bedroom", "AudioAccessory5,1", &[("flags", "0x1b8c04")])
}

#[test]
fn a_busy_speaker_is_taken_over_on_a_later_try() {
    let mut r = rig("busy-then-ok", "busy=2", vec![busy_homepod()]);
    r.ctl.connect_named("Bedroom", opts());
    r.on();
    let j = r.journal();
    assert_eq!(j.matches("args ").count(), 3, "two refusals, then it plays: {j}");
    assert_eq!(j.matches("code=connect_failed").count(), 2, "{j}");
    // Connecting throughout: no failure, no fall back to this computer between the tries.
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. } | Event::State(Cast::Off))), "{:?}", r.seen);
    r.end();
}

#[test]
fn a_speaker_that_stays_busy_says_so_after_three_tries() {
    let mut r = rig("busy-always", "busy", vec![busy_homepod()]);
    let t = Instant::now();
    r.ctl.connect_named("Bedroom", opts());
    let Event::Failed { why, name, .. } = r.wait("the failure", |e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert_eq!((why, name.as_str()), (Why::Busy, "Bedroom"));
    assert_eq!(why.message("Bedroom"), "“Bedroom” is busy with another device.");
    assert!(t.elapsed() >= Duration::from_millis(600), "it waited between the tries: {:?}", t.elapsed());
    r.wait("off", |e| *e == Event::State(Cast::Off));
    assert_eq!(r.journal().matches("args ").count(), 3, "{}", r.journal());
    assert!(!r.seen.contains(&Event::Capture(true)), "never connected: the bridge was never asked to capture");
    let feed = r.ctl.local_feed();
    assert!(!feed.casting() && !feed.mute_local(), "back on this computer");
    r.end();
    // RTSP's own "busy" (453 Not Enough Bandwidth) says so too, whatever the flags.
    assert_eq!(failure("busy-453", "busy,http=453", device("Bedroom", "AudioAccessory5,1", &[]), false).0, Why::Busy);
}

#[test]
fn a_speaker_that_is_off_is_tried_three_times_then_unreachable() {
    let mut r = rig("off-tries", "connect_failed", vec![device("Bedroom", "AudioAccessory5,1", &[("flags", "0x98404")])]);
    r.ctl.connect_named("Bedroom", opts());
    assert!(matches!(r.wait("the failure", |e| matches!(e, Event::Failed { .. })), Event::Failed { why: Why::Unreachable, .. }));
    assert_eq!(r.journal().matches("args ").count(), 3, "{}", r.journal());
    r.end();
}

#[test]
fn picking_this_computer_while_waiting_to_try_again_ends_quietly() {
    let mut r = rig("busy-stop", "busy", vec![busy_homepod()]);
    r.ctl.connect_named("Bedroom", opts());
    wait_for("the first refusal", || r.journal().contains("code=connect_failed"));
    r.ctl.disconnect();
    r.wait("off", |e| *e == Event::State(Cast::Off));
    std::thread::sleep(Duration::from_millis(800));
    while let Ok(e) = r.events.try_recv() {
        r.seen.push(e);
    }
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", r.seen);
    assert_eq!(r.journal().matches("args ").count(), 1, "no more tries: {}", r.journal());
    r.end();
}

#[test]
fn a_lost_connection_returns_to_this_computer() {
    let (why, seen) = failure("lost", "die_after=1.2", device("Bedroom", "AudioAccessory5,1", &[]), true);
    assert_eq!(why, Why::Lost);
    let at = |e: &Event| seen.iter().position(|x| x == e).unwrap();
    assert!(at(&Event::Capture(true)) < at(&Event::Capture(false)), "the capture is stopped: {seen:?}");
}

#[test]
fn a_clock_nobody_answers_ends_the_session_with_the_firewall_hint() {
    let (why, _) = failure("clock", "stalled", device("Bedroom", "AudioAccessory5,1", &[]), false);
    assert_eq!(why, Why::Clock);
    assert!(why.message("Bedroom").contains("firewall"));
}

#[test]
fn a_homepod_that_refuses_points_to_the_home_app_and_a_missing_helper_or_speaker_says_so() {
    assert_eq!(failure("refused", "auth_required", device("Bedroom", "AudioAccessory5,1", &[]), false).0, Why::Refused);
    // No speaker of that name.
    let mut r = rig("notfound", "", vec![device("Kitchen", "Sonos One", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    assert!(matches!(r.wait("not found", |e| matches!(e, Event::Failed { .. })), Event::Failed { why: Why::NotFound, .. }));
    r.end();
    // No helper in this install.
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let cfg = Config { helper: None, fake: vec![device("Bedroom", "AudioAccessory5,1", &[])], dir: std::env::temp_dir().join(format!("uzu-ap-nohelper-{}", std::process::id())), ptp_daemon: false, ..Config::from_env(1) };
    let ctl = Controller::new(cfg, move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    ctl.connect_named("Bedroom", opts());
    let mut got = Vec::new();
    while let Ok(e) = rx.recv_timeout(Duration::from_secs(5)) {
        got.push(e.clone());
        if e == Event::State(Cast::Off) {
            break;
        }
    }
    assert!(got.contains(&Event::Failed { name: "Bedroom".into(), why: Why::NoHelper, still: vec![] }), "{got:?}");
    ctl.shutdown();
}

#[test]
fn a_password_is_asked_for_and_a_wrong_one_asked_again() {
    let mut r = rig("password", "need_password", vec![device("Bedroom", "Sonos One", &[("pw", "true")])]);
    r.ctl.connect_named("Bedroom", opts());
    assert!(matches!(r.wait("the question", |e| matches!(e, Event::Auth { .. })), Event::Auth { need: Need::Password, wrong: false, .. }));
    r.ctl.answer("nope");
    assert!(matches!(r.wait("the question again", |e| matches!(e, Event::Auth { .. })), Event::Auth { need: Need::Password, wrong: true, .. }));
    r.ctl.answer(" secret ");
    r.on();
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", r.seen);
    r.end();
}

#[test]
fn an_apple_tv_is_paired_with_its_pin_and_then_plays() {
    let mut r = rig("pin", "need_pin", vec![device("Bedroom", "AppleTV14,1", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    assert!(matches!(r.wait("the PIN question", |e| matches!(e, Event::Auth { .. })), Event::Auth { need: Need::Pin, wrong: false, .. }));
    assert!(r.journal().contains("pair-setup --pair-setup --port 7000 --dacp 0000000000001234 127.0.0.1"), "{}", r.journal());
    r.ctl.answer("1234");
    let Event::Paired { id, credentials } = r.wait("the pairing", |e| matches!(e, Event::Paired { .. })) else { unreachable!() };
    assert_eq!((id.as_str(), credentials.len()), ("id-bedroom", 192));
    r.on();
    assert!(r.journal().lines().any(|l| l.starts_with("args ") && l.contains("--auth abab")), "{}", r.journal());
    r.end();

    // A wrong PIN: said so, and nothing plays.
    let mut r = rig("pin-wrong", "need_pin", vec![device("Bedroom", "AppleTV14,1", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    r.wait("the PIN question", |e| matches!(e, Event::Auth { .. }));
    r.ctl.answer("0000");
    assert!(matches!(r.wait("the failure", |e| matches!(e, Event::Failed { .. })), Event::Failed { why: Why::Pairing, .. }));
    // Cancelled while it asks: no word.
    r.ctl.connect_named("Bedroom", opts());
    r.wait("the PIN question", |e| matches!(e, Event::Auth { .. }));
    let n = r.seen.len();
    r.ctl.disconnect();
    std::thread::sleep(Duration::from_millis(400));
    while let Ok(e) = r.events.try_recv() {
        r.seen.push(e);
    }
    assert!(!r.seen[n..].iter().any(|e| matches!(e, Event::Failed { .. } | Event::Paired { .. })), "{:?}", &r.seen[n..]);
    r.end();
}

#[test]
fn quitting_ends_the_helper() {
    let mut r = rig("quit", "", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    r.on();
    let pid_file = r.dir.join("uzumaki-airplay-helper.pid");
    let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    let alive = |pid: i32| std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(alive(pid));
    let t = Instant::now();
    r.ctl.shutdown();
    assert!(t.elapsed() < Duration::from_secs(4), "shutdown took {:?}", t.elapsed());
    wait_for("the helper to end", || !alive(pid));
    assert!(r.journal().contains("cmd ACTION=STOP"), "the speaker was told to stop");
    assert!(!r.dir.join("run").exists() && !pid_file.exists());
    // Nothing starts after that.
    r.ctl.connect_named("Bedroom", opts());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.journal().matches("args ").count(), 1);
    r.end();
}

// ---- several speakers (multi-room) ----

/// The bridge playing a ramp: frame n is (n & 0x7fff, n >> 15), so every frame says where it is
/// in the stream. Written about as fast as it plays, until `stop`.
fn ramp(mut bridge: UnixStream, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut n: u32 = 0;
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            let mut chunk = Vec::with_capacity(4096 * 4);
            for _ in 0..4096 {
                chunk.extend_from_slice(&((n & 0x7fff) as i16).to_le_bytes());
                chunk.extend_from_slice(&((n >> 15) as i16).to_le_bytes());
                n += 1;
            }
            if bridge.write_all(&chunk).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    })
}

/// `key=<number>` in a journal line.
fn num(line: &str, key: &str) -> u64 {
    line.split(' ').find_map(|w| w.strip_prefix(&format!("{key}="))).and_then(|v| v.parse().ok()).unwrap_or_else(|| panic!("no {key} in {line}"))
}

fn started_lines(journal: &str) -> Vec<(u64, u64)> {
    journal.lines().filter(|l| l.starts_with("status started")).map(|l| (num(l, "requested_unix_ms"), num(l, "at_unix_ms"))).collect()
}

fn two() -> Vec<Device> {
    vec![device("Bedroom", "AudioAccessory5,1", &[]), device("Kitchen", "Sonos One", &[])]
}

#[test]
fn a_speaker_added_while_one_plays_joins_it_in_step() {
    let mut r = with_bridge("join", two());
    r.ctl.connect_named("Bedroom", opts());
    r.wait("the capture request", |e| *e == Event::Capture(true));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let feeder = ramp(r.bridge(), stop.clone());
    r.playing(&["Bedroom"]);
    std::thread::sleep(Duration::from_millis(300));
    r.ctl.add_named("Kitchen", opts());
    r.playing(&["Bedroom", "Kitchen"]);
    wait_for("Kitchen's audio", || r.frames_of("Kitchen").len() > 4096);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    feeder.join().unwrap();

    let (bed, kit) = (r.journal_of("Bedroom"), r.journal_of("Kitchen"));
    assert!(kit.contains("cmd START_JOIN=1"), "Kitchen joins the group that plays: {kit}");
    assert_eq!(bed.matches("cmd ACTION=START").count(), 1, "Bedroom plays on undisturbed: {bed}");
    assert!(!bed.contains("ACTION=FLUSH"), "{bed}");
    // Frame 0 of Bedroom's stream is heard at its start; Kitchen's first frame sits at index p of
    // that same stream, so it must be heard p / 44.1 ms later.
    let bed_at = started_lines(&bed)[0].1;
    let (kit_req, kit_at) = started_lines(&kit)[0];
    let (b, k) = (r.frames_of("Bedroom"), r.frames_of("Kitchen"));
    let p = b.iter().position(|f| *f == k[0]).expect("Kitchen's first frame is in Bedroom's stream") as u64;
    let due = bed_at + (p * 1000 + 22050) / 44100;
    assert!(kit_req.abs_diff(due) <= 1, "asked for {kit_req}, due {due} (frame {p})");
    // Its helper could start it only later (it had to wait for its clock): it cut as much audio,
    // so the first frame it plays is the one Bedroom plays at that instant.
    let cut = kit.lines().find(|l| l.starts_with("status content_cut")).map_or(0, |l| num(l, "cut_bytes") / 4);
    assert_eq!(cut == 0, kit_at == kit_req, "a cut exactly when it starts later: {kit}");
    let heard = p + cut;
    let due = bed_at + (heard * 1000 + 22050) / 44100;
    assert!(kit_at.abs_diff(due) <= 1, "Kitchen plays frame {heard} at {kit_at}, Bedroom at {due}");
    // Both got the same bytes from there on.
    let n = k.len().min(b.len() - p as usize);
    assert!(n > 4096 && b[p as usize..p as usize + n] == k[..n], "the same audio, frame for frame");
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. } | Event::State(Cast::Off))), "{:?}", r.seen);
    r.end();
}

#[test]
fn one_speaker_off_does_not_stop_the_other() {
    let mut r = rig("one-off", "Kitchen:connect_failed", two());
    r.ctl.connect_named("Bedroom", opts());
    r.playing(&["Bedroom"]);
    r.ctl.add_named("Kitchen", opts());
    let Event::Failed { name, why, still } = r.wait("Kitchen's failure", |e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert_eq!((name.as_str(), why, still), ("Kitchen", Why::Unreachable, vec!["Bedroom".to_string()]));
    assert_eq!(r.journal_of("Kitchen").matches("args ").count(), 3, "tried three times");
    std::thread::sleep(Duration::from_millis(300));
    while let Ok(e) = r.events.try_recv() {
        r.seen.push(e);
    }
    assert!(!r.seen.iter().any(|e| matches!(e, Event::State(Cast::Off) | Event::Capture(false))), "Bedroom plays on: {:?}", r.seen);
    assert_eq!(r.ctl.speakers(), vec![Speaker { id: "id-bedroom".into(), name: "Bedroom".into(), playing: true }]);
    assert!(r.ctl.local_feed().mute_local(), "still casting");
    assert!(!r.journal_of("Bedroom").contains("ACTION=STOP"));
    r.end();
}

#[test]
fn a_busy_speaker_joins_the_group_on_a_later_try() {
    let mut r = rig("group-busy", "Kitchen:busy=1", two());
    r.ctl.connect_named("Bedroom", opts());
    r.playing(&["Bedroom"]);
    r.ctl.add_named("Kitchen", opts());
    r.playing(&["Bedroom", "Kitchen"]);
    assert_eq!(r.journal_of("Kitchen").matches("args ").count(), 2, "{}", r.journal_of("Kitchen"));
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", r.seen);
    r.end();
}

#[test]
fn both_speakers_off_falls_back_to_this_computer() {
    let mut r = rig("both-off", "connect_failed", two());
    r.ctl.connect_named("Bedroom", opts());
    r.ctl.add_named("Kitchen", opts());
    let mut failed = Vec::new();
    while failed.len() < 2 {
        let Event::Failed { name, still, .. } = r.wait("the failures", |e| matches!(e, Event::Failed { .. })) else { unreachable!() };
        failed.push((name, still));
    }
    assert_eq!(failed.last().unwrap().1, Vec::<String>::new(), "the last one out says the sound is back here: {failed:?}");
    r.wait("off", |e| *e == Event::State(Cast::Off));
    assert!(!r.ctl.local_feed().casting());
    r.end();
}

#[test]
fn each_speaker_has_its_own_volume() {
    let mut r = rig("volumes", "", two());
    r.ctl.connect_named("Bedroom", Connect { volume: 20, ..opts() });
    r.playing(&["Bedroom"]);
    r.ctl.add_named("Kitchen", Connect { volume: 45, ..opts() });
    r.playing(&["Bedroom", "Kitchen"]);
    assert!(r.journal_of("Bedroom").contains("--volume 20") && r.journal_of("Kitchen").contains("--volume 45"));
    r.ctl.set_speaker_volume("id-kitchen", 70);
    wait_for("Kitchen's volume", || r.journal_of("Kitchen").contains("cmd VOLUME=70"));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!r.journal_of("Bedroom").contains("cmd VOLUME="), "Bedroom keeps its own: {}", r.journal_of("Bedroom"));
    r.ctl.set_volume(10);
    wait_for("every speaker's volume", || r.journal_of("Bedroom").contains("cmd VOLUME=10") && r.journal_of("Kitchen").contains("cmd VOLUME=10"));
    r.end();
}

#[test]
fn a_seek_restarts_every_speaker_together() {
    let mut r = rig("group-seek", "clock", two());
    r.ctl.connect_named("Bedroom", opts());
    r.playing(&["Bedroom"]);
    r.ctl.add_named("Kitchen", opts());
    r.playing(&["Bedroom", "Kitchen"]);
    r.ctl.flush();
    wait_for("both restarted", || started_lines(&r.journal_of("Bedroom")).len() == 2 && started_lines(&r.journal_of("Kitchen")).len() == 2);
    let (bed, kit) = (r.journal_of("Bedroom"), r.journal_of("Kitchen"));
    assert!(bed.contains("cmd ACTION=FLUSH") && kit.contains("cmd ACTION=FLUSH"));
    let (b, k) = (started_lines(&bed)[1], started_lines(&kit)[1]);
    assert!(b.0 > 0 && b.0 == k.0, "one instant for both: {b:?} {k:?}");
    assert_eq!((b.1, k.1), (b.0, k.0), "an instant both could make");
    let after = kit.rsplit("cmd ACTION=FLUSH").next().unwrap();
    assert!(!after.contains("START_JOIN"), "a common start, not a join: {after}");
    // Since the flush both got the same audio from the first frame.
    let (bf, kf) = (r.frames_of("Bedroom"), r.frames_of("Kitchen"));
    assert!(!bf.is_empty() && !kf.is_empty());
    r.end();
}

#[test]
fn taking_one_speaker_out_leaves_the_other() {
    let mut r = rig("remove", "", two());
    r.ctl.connect_named("Bedroom", opts());
    r.ctl.add_named("Kitchen", opts());
    r.playing(&["Bedroom", "Kitchen"]);
    r.ctl.remove("id-kitchen");
    r.wait("Bedroom alone", |e| matches!(e, Event::Speakers(list) if list.len() == 1 && list[0].name == "Bedroom"));
    wait_for("Kitchen told to stop", || r.journal_of("Kitchen").contains("cmd ACTION=STOP"));
    assert!(!r.journal_of("Bedroom").contains("ACTION=STOP"));
    assert!(r.ctl.local_feed().mute_local());
    r.ctl.remove("id-bedroom");
    r.wait("off", |e| *e == Event::State(Cast::Off));
    assert!(!r.ctl.local_feed().casting());
    assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", r.seen);
    r.end();
}

#[test]
fn picking_one_speaker_alone_ends_the_group() {
    let mut r = rig("exclusive", "", vec![device("Bedroom", "AudioAccessory5,1", &[]), device("Kitchen", "Sonos One", &[]), device("Office", "Sonos One", &[])]);
    r.ctl.connect_named("Bedroom", opts());
    r.ctl.add_named("Kitchen", opts());
    r.playing(&["Bedroom", "Kitchen"]);
    r.ctl.connect_named("Office", opts());
    r.playing(&["Office"]);
    assert!(r.journal_of("Bedroom").contains("ACTION=STOP") && r.journal_of("Kitchen").contains("ACTION=STOP"));
    r.end();
}

#[test]
fn the_group_shares_one_clock_that_ends_with_it() {
    let mut r = rig_with("ptp", "", two(), true);
    r.ctl.connect_named("Bedroom", opts());
    r.ctl.add_named("Kitchen", opts());
    r.playing(&["Bedroom", "Kitchen"]);
    let j = r.journal();
    assert_eq!(j.matches("ptp-daemon --ptp-daemon --dacp 0000000000001234").count(), 1, "one clock for the group: {j}");
    assert_eq!(j.lines().filter(|l| l.starts_with("args ") && l.contains("--ptp-shared")).count(), 2, "{j}");
    let pids: Vec<i32> = std::fs::read_to_string(r.dir.join("uzumaki-airplay-helper.pid")).unwrap().lines().map(|l| l.trim().parse().unwrap()).collect();
    assert_eq!(pids.len(), 3, "two helpers and the clock");
    let alive = |pid: i32| std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    r.ctl.disconnect();
    r.wait("off", |e| *e == Event::State(Cast::Off));
    wait_for("every process to end", || pids.iter().all(|&p| !alive(p)));
    r.end();
}

#[test]
fn how_far_behind_the_speaker_is_follows_its_start_its_padding_and_a_flush() {
    let mut r = rig("behind", "reanchor", vec![device("Bedroom", "AudioAccessory5,1", &[])]);
    assert_eq!(r.ctl.behind(), None, "nothing casts");
    r.ctl.connect_named("Bedroom", opts());
    r.on();
    // Started ~250 ms ahead (as the real helper), then 2 s of padding after a gap: everything
    // written since is heard 2 s later than it would have been.
    wait_for("the padding", || r.ctl.behind().is_some_and(|b| b >= Duration::from_millis(2000)));
    let b = r.ctl.behind().unwrap();
    assert!(b <= Duration::from_millis(3000), "{b:?}");
    // A flush starts the speaker again: no padding since that start.
    r.ctl.flush();
    wait_for("the second start", || r.journal().matches("status started").count() == 2);
    wait_for("the new start's delay", || r.ctl.behind().is_some_and(|b| b < Duration::from_millis(1500)));
    r.ctl.disconnect();
    r.wait("off", |e| *e == Event::State(Cast::Off));
    assert_eq!(r.ctl.behind(), None, "back on this computer");
    r.end();
}
