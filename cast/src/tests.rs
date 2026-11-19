//! A fake Cast device: a TLS server on 127.0.0.1 that speaks enough CASTV2 to be cast to, and
//! fetches the stream itself the way a receiver does (it starts "playing" once it holds
//! `FAKE_BUFFER` of audio, so the measured latency must come out near that).

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};

use super::*;
use crate::proto::{DEFAULT_MEDIA_RECEIVER, Deframer, Message, NS_HEARTBEAT, NS_MEDIA, NS_RECEIVER, RECEIVER};

const FAKE_BUFFER: f64 = 0.5;

#[derive(Default)]
struct Fake {
    /// Every message type received, in order (`ns-suffix:TYPE`).
    journal: Mutex<Vec<String>>,
    /// Fetch the stream after LOAD (off: a firewall in the way).
    no_fetch: AtomicBool,
    /// Frames of audio fetched (current LOAD).
    fetched: AtomicU64,
    loads: AtomicU64,
    /// Send a RECEIVER_STATUS with another app (someone cast to it).
    take_over: AtomicBool,
    /// When it started playing (it holds `FAKE_BUFFER` of audio): from then on its position
    /// moves in real time, as a real receiver's, never past what it fetched.
    play_start: Mutex<Option<Instant>>,
}

impl Fake {
    fn playhead(&self) -> f64 {
        let fetched = self.fetched.load(Ordering::SeqCst) as f64 / 44100.0;
        self.play_start.lock().unwrap().map_or(0.0, |t| t.elapsed().as_secs_f64().min(fetched))
    }
}

impl Fake {
    fn saw(&self, what: &str) -> bool {
        self.journal.lock().unwrap().iter().any(|j| j == what)
    }
    fn count(&self, what: &str) -> usize {
        self.journal.lock().unwrap().iter().filter(|j| *j == what).count()
    }
}

fn server_config() -> Arc<ServerConfig> {
    let cert = CertificateDer::from(include_bytes!("testdata/fake-device-cert.der").to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(include_bytes!("testdata/fake-device-key.der").to_vec()));
    let cfg = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    Arc::new(cfg)
}

/// Starts the fake; returns its port.
fn fake_device(fake: Arc<Fake>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(tcp) = conn else { continue };
            let fake = fake.clone();
            std::thread::spawn(move || device_conn(tcp, &fake));
        }
    });
    port
}

fn reply(s: &mut StreamOwned<ServerConnection, TcpStream>, src: &str, dst: &str, ns: &str, v: Value) {
    let _ = s.write_all(&Message::new(src, dst, ns, &v).frame());
    let _ = s.flush();
}

fn device_conn(tcp: TcpStream, fake: &Arc<Fake>) {
    tcp.set_read_timeout(Some(Duration::from_millis(30))).unwrap();
    let mut s = StreamOwned::new(ServerConnection::new(server_config()).unwrap(), tcp);
    let mut de = Deframer::default();
    let mut buf = [0u8; 8192];
    let mut launched = false;
    let mut volume = 0.3;
    let mut state = "IDLE";
    let mut sender = String::new();
    let mut took = false;
    let playhead = || fake.playhead();
    let apps = |launched: bool| if launched { json!([{"appId": DEFAULT_MEDIA_RECEIVER, "sessionId": "s-1", "transportId": "web-1", "displayName": "Default Media Receiver"}]) } else { json!([]) };
    loop {
        if fake.take_over.load(Ordering::SeqCst) && !took && !sender.is_empty() {
            took = true;
            reply(&mut s, RECEIVER, &sender, NS_RECEIVER, json!({"type": "RECEIVER_STATUS", "requestId": 0, "status": {"applications": [{"appId": "233637DE", "sessionId": "yt", "transportId": "yt", "displayName": "YouTube"}], "volume": {"level": volume, "muted": false}}}));
        }
        match s.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => de.push(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(_) => return,
        }
        while let Some(Ok(m)) = de.take() {
            let v = m.json().unwrap_or(Value::Null);
            let kind = v["type"].as_str().unwrap_or("").to_string();
            let short = m.namespace.rsplit('.').next().unwrap_or("").to_string();
            fake.journal.lock().unwrap().push(format!("{short}:{kind}"));
            sender = m.source.clone();
            let rid = v["requestId"].clone();
            let media_status = |state: &str, ph: f64| json!({"type": "MEDIA_STATUS", "requestId": rid, "status": [{"mediaSessionId": 7, "playerState": state, "currentTime": ph}]});
            match (short.as_str(), kind.as_str()) {
                ("heartbeat", "PING") => reply(&mut s, &m.destination, &m.source, NS_HEARTBEAT, json!({"type": "PONG"})),
                ("receiver", "GET_STATUS") => reply(&mut s, RECEIVER, &m.source, NS_RECEIVER, json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {"applications": apps(launched), "volume": {"level": volume, "muted": false}}})),
                ("receiver", "LAUNCH") => {
                    launched = true;
                    reply(&mut s, RECEIVER, &m.source, NS_RECEIVER, json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {"applications": apps(true), "volume": {"level": volume, "muted": false}}}));
                }
                ("receiver", "SET_VOLUME") => {
                    volume = v["volume"]["level"].as_f64().unwrap_or(volume);
                    reply(&mut s, RECEIVER, &m.source, NS_RECEIVER, json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {"applications": apps(launched), "volume": {"level": volume, "muted": false}}}));
                }
                ("receiver", "STOP") => {
                    launched = false;
                    reply(&mut s, RECEIVER, &m.source, NS_RECEIVER, json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {"applications": [], "volume": {"level": volume, "muted": false}}}));
                }
                ("media", "LOAD") => {
                    let url = v["media"]["contentId"].as_str().unwrap_or("").to_string();
                    let n = fake.loads.fetch_add(1, Ordering::SeqCst) + 1;
                    fake.fetched.store(0, Ordering::SeqCst);
                    *fake.play_start.lock().unwrap() = None;
                    state = "BUFFERING";
                    if !fake.no_fetch.load(Ordering::SeqCst) {
                        let f = fake.clone();
                        std::thread::spawn(move || fetch(&url, &f, n));
                    }
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, media_status(state, 0.0));
                }
                ("media", "GET_STATUS") => {
                    if state == "BUFFERING" && playhead() > 0.0 {
                        state = "PLAYING";
                    }
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, media_status(state, playhead()));
                }
                ("media", "PAUSE") => {
                    state = "PAUSED";
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, media_status(state, playhead()));
                }
                ("media", "PLAY") => {
                    state = "PLAYING";
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, media_status(state, playhead()));
                }
                _ => {}
            }
        }
    }
}

/// GETs the stream as a receiver does and counts the audio frames, until the URL stops working.
fn fetch(url: &str, fake: &Fake, load: u64) {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let Ok(mut c) = TcpStream::connect(host) else { return };
    let _ = write!(c, "GET /{path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: FakeCast\r\nRange: bytes=0-\r\n\r\n");
    let mut head = Vec::new();
    let mut buf = [0u8; 16384];
    let mut body = 0usize;
    loop {
        let n = match c.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if fake.loads.load(Ordering::SeqCst) != load {
            return;
        }
        if body == 0 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
            head.extend_from_slice(&buf[..n]);
            if let Some(p) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                body = head.len() - (p + 4);
            }
            continue;
        }
        body += n;
        let frames = (body.saturating_sub(44) / 4) as u64;
        fake.fetched.store(frames, Ordering::SeqCst);
        if frames as f64 >= FAKE_BUFFER * 44100.0 {
            fake.play_start.lock().unwrap().get_or_insert_with(Instant::now);
        }
    }
}

fn config(port: u16) -> Config {
    let mut cfg = Config::from_env();
    cfg.dir = std::env::temp_dir().join(format!("uzumaki-cast-test-{}-{port}", std::process::id()));
    cfg.port = 0;
    cfg.format = Format::Wav;
    cfg.fake = Some(CastDevice::at("127.0.0.1".parse().unwrap(), port, "Fake TV"));
    cfg.keep_local = false;
    cfg.fetch_timeout = Duration::from_secs(2);
    cfg.preroll_ms = 0;
    cfg.http = Http::Close;
    cfg
}

type Events = Arc<Mutex<Vec<Event>>>;

fn controller(cfg: Config) -> (Controller, Events) {
    let events: Events = Arc::default();
    let e = events.clone();
    (Controller::new(cfg, move |ev| e.lock().unwrap().push(ev)), events)
}

fn wait(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let t = Instant::now();
    while !ok() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn playing_latency(c: &Controller) -> Option<u32> {
    match c.state() {
        State::Playing { latency_ms, .. } => latency_ms,
        _ => None,
    }
}

#[test]
fn a_whole_session_with_the_fake_device() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    let (c, events) = controller(config(port));
    c.connect_named("fake tv", Meta { title: "Song".into(), artist: "Artist".into(), ..Meta::default() });
    // Launch, LOAD, fetch, PLAYING, a buffer measured (silence from the app's inputs).
    wait("a measured latency", 15, || playing_latency(&c).is_some());
    let lat = playing_latency(&c).unwrap();
    assert!((300..=900).contains(&lat), "latency {lat} ms, the fake holds {FAKE_BUFFER} s");
    assert!(fake.saw("receiver:LAUNCH") && fake.saw("media:LOAD") && fake.saw("connection:CONNECT"));
    assert!(events.lock().unwrap().contains(&Event::Capture(true)), "the bridge is asked to capture");
    assert!(c.local_feed().casting() && c.local_feed().mute_local());
    #[cfg(unix)]
    assert!(c.socket().exists(), "the session's PCM socket is there");
    let lines = c.status_lines().join("\n");
    assert!(lines.contains("fetched by 127.0.0.1") && lines.contains("LOAD→fetch"), "{lines}");

    assert!(c.set_volume(40));
    wait("the volume", 5, || events.lock().unwrap().iter().any(|e| matches!(e, Event::Volume { level, .. } if (*level - 0.4).abs() < 1e-9)));
    assert!(c.pause());
    wait("paused", 5, || matches!(c.state(), State::Paused { .. }));
    assert!(c.resume());
    wait("playing again", 5, || matches!(c.state(), State::Playing { .. }));

    // A reload: a second LOAD with a new URL, fetched afresh.
    assert!(c.reload(None));
    wait("the second load's latency", 15, || fake.loads.load(Ordering::SeqCst) == 2 && playing_latency(&c).is_some() && c.status_lines().join(" ").contains("2 LOAD"));

    c.stop();
    wait("off", 5, || c.state() == State::Off && c.idle());
    assert!(fake.saw("receiver:STOP"), "{:?}", fake.journal.lock().unwrap());
    assert!(events.lock().unwrap().contains(&Event::Capture(false)));
    assert!(!c.local_feed().casting());
    #[cfg(unix)]
    assert!(!c.socket().exists(), "the socket is gone");
    assert!(!events.lock().unwrap().iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", events.lock().unwrap());
    c.shutdown();
}

/// The device holds what the pre-roll gave it: the steady delay follows the setting, and it
/// starts at once instead of waiting for the live audio to fill its buffer.
#[test]
fn the_preroll_sets_the_steady_delay() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    let mut cfg = config(port);
    cfg.preroll_ms = 1500;
    let (c, _events) = controller(cfg);
    c.connect_named("Fake TV", Meta::default());
    wait("a measured latency", 15, || playing_latency(&c).is_some());
    // A second status, so the median is not only the first.
    let first = c.status_lines().join(" ");
    wait("two measurements", 10, || c.status_lines().join(" ") != first && playing_latency(&c).is_some());
    let lat = playing_latency(&c).unwrap();
    assert!((1200..=1900).contains(&lat), "latency {lat} ms with a 1.5 s pre-roll");
    let lines = c.status_lines().join("\n");
    assert!(lines.contains("pre-roll 1500 ms") && lines.contains("underruns 0"), "{lines}");
    c.stop();
    wait("off", 5, || c.idle());
    c.shutdown();
}

#[test]
fn another_app_taking_the_device_ends_the_session() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    let (c, events) = controller(config(port));
    c.connect_named("Fake TV", Meta::default());
    wait("playing", 15, || matches!(c.state(), State::Playing { .. }));
    fake.take_over.store(true, Ordering::SeqCst);
    wait("taken", 5, || events.lock().unwrap().iter().any(|e| matches!(e, Event::Failed { why: Why::Taken, .. })));
    wait("off", 5, || c.state() == State::Off && c.idle());
    assert!(!fake.saw("receiver:STOP"), "someone else's app is not stopped");
    assert!(events.lock().unwrap().contains(&Event::Capture(false)));
    c.shutdown();
}

#[test]
fn a_device_that_never_fetches_is_a_firewall() {
    let fake = Arc::new(Fake::default());
    fake.no_fetch.store(true, Ordering::SeqCst);
    let port = fake_device(fake.clone());
    let (c, events) = controller(config(port));
    c.connect_named("Fake TV", Meta::default());
    wait("no fetch", 10, || events.lock().unwrap().iter().any(|e| matches!(e, Event::Failed { why: Why::NoFetch, .. })));
    wait("off", 5, || c.idle());
    assert_eq!(fake.count("receiver:STOP"), 1, "our receiver is stopped again");
    assert!(Why::NoFetch.message("TV").contains("firewall"));
    c.shutdown();
}

#[test]
fn the_address_to_use_is_chosen_from_all_the_device_advertises() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    // A device with global IPv6 addresses and an IPv4 one: the session talks IPv4 (here the
    // loopback, as "this computer's subnet"), serves IPv4, and the stream plays.
    let mut cfg = config(port);
    let mut dev = CastDevice::at("2001:db8:1:2::8fd8".parse().unwrap(), port, "Fake TV");
    dev.addrs = vec!["2001:db8:1:2::8fd8".parse().unwrap(), "2001:db8:1:2:4129::758f".parse().unwrap(), "127.0.0.1".parse().unwrap()];
    cfg.fake = Some(dev.clone());
    let (c, events) = controller(cfg.clone());
    c.connect(dev.clone(), Meta::default());
    wait("playing over IPv4", 15, || matches!(c.state(), State::Playing { .. }));
    assert!(c.status_lines().join(" ").contains("fetched by 127.0.0.1"), "{:?}", c.status_lines());
    c.stop();
    wait("off", 5, || c.idle());
    assert!(!events.lock().unwrap().iter().any(|e| matches!(e, Event::Failed { .. })));
    c.shutdown();
    // Forced IPv4 on a device without one: a clear failure, nothing sent.
    cfg.family = Family::V4;
    let v6only = CastDevice::at("2001:db8:1:2::8fd8".parse().unwrap(), port, "Fake TV");
    let (c, events) = controller(cfg);
    c.connect(v6only, Meta::default());
    wait("no IPv4", 5, || events.lock().unwrap().iter().any(|e| matches!(e, Event::Failed { why: Why::Unreachable, detail, .. } if detail.contains("V4"))));
}

#[test]
fn nobody_there_is_unreachable() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port(); // closed again
    let (c, events) = controller(config(port));
    c.connect_named("Fake TV", Meta::default());
    wait("unreachable", 8, || events.lock().unwrap().iter().any(|e| matches!(e, Event::Failed { why: Why::Unreachable, .. })));
    assert_eq!(c.state(), State::Off);
}
