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
    /// A media session of an earlier sender is still there: our LOAD interrupts it first.
    stale_session: AtomicBool,
    /// Every LOAD's payload, in order.
    load_msgs: Mutex<Vec<Value>>,
    /// A file (BUFFERED) plays from `.0` s since `.1`, until its duration `.2`.
    file: Mutex<Option<(f64, Instant, f64)>>,
    /// Refuse files (LOAD_FAILED), as for a format the receiver doesn't play.
    fail_files: AtomicBool,
    /// Drop the connection now (Wi-Fi gone), once.
    drop_conn: AtomicBool,
    /// Accept no connection (the TV is off).
    off: AtomicBool,
    connections: AtomicU64,
    /// The stream's bytes as fetched (the WAV header skipped), current LOAD.
    body: Mutex<Vec<u8>>,
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
            if fake.off.load(Ordering::SeqCst) {
                drop(tcp);
                continue;
            }
            fake.connections.fetch_add(1, Ordering::SeqCst);
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
    let playhead = || match *fake.file.lock().unwrap() {
        Some((base, t, dur)) => (base + t.elapsed().as_secs_f64()).min(dur),
        None => fake.playhead(),
    };
    let file_over = || fake.file.lock().unwrap().is_some_and(|(base, t, dur)| base + t.elapsed().as_secs_f64() >= dur);
    let apps = |launched: bool| if launched { json!([{"appId": DEFAULT_MEDIA_RECEIVER, "sessionId": "s-1", "transportId": "web-1", "displayName": "Default Media Receiver"}]) } else { json!([]) };
    loop {
        if fake.take_over.load(Ordering::SeqCst) && !took && !sender.is_empty() {
            took = true;
            reply(&mut s, RECEIVER, &sender, NS_RECEIVER, json!({"type": "RECEIVER_STATUS", "requestId": 0, "status": {"applications": [{"appId": "233637DE", "sessionId": "yt", "transportId": "yt", "displayName": "YouTube"}], "volume": {"level": volume, "muted": false}}}));
        }
        if fake.drop_conn.swap(false, Ordering::SeqCst) {
            return; // the socket closes: the sender sees the connection end
        }
        if fake.off.load(Ordering::SeqCst) {
            // Off: nothing more is said (no PONG either), as a TV that lost power.
            std::thread::sleep(Duration::from_millis(50));
            continue;
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
                ("receiver", "GET_STATUS") => {
                    // The receiver app outlives a sender's connection (a reconnect finds it).
                    launched |= fake.connections.load(Ordering::SeqCst) > 1 && fake.loads.load(Ordering::SeqCst) > 0;
                    reply(&mut s, RECEIVER, &m.source, NS_RECEIVER, json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {"applications": apps(launched), "volume": {"level": volume, "muted": false}}}));
                }
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
                    fake.load_msgs.lock().unwrap().push(v.clone());
                    let n = fake.loads.fetch_add(1, Ordering::SeqCst) + 1;
                    fake.fetched.store(0, Ordering::SeqCst);
                    fake.body.lock().unwrap().clear();
                    *fake.play_start.lock().unwrap() = None;
                    *fake.file.lock().unwrap() = None;
                    if v["media"]["streamType"] == "BUFFERED" {
                        if fake.fail_files.load(Ordering::SeqCst) {
                            reply(&mut s, "web-1", &m.source, NS_MEDIA, json!({"type": "LOAD_FAILED", "requestId": rid}));
                            continue;
                        }
                        // Reads the file (ranged), then plays it from `currentTime`.
                        let (f, at, dur) = (fake.clone(), v["currentTime"].as_f64().unwrap_or(0.0), v["media"]["duration"].as_f64().unwrap_or(1e9));
                        std::thread::spawn(move || {
                            if fetch_file(&url, &f) {
                                *f.file.lock().unwrap() = Some((at, Instant::now(), dur));
                            }
                        });
                        state = "BUFFERING";
                        reply(&mut s, "web-1", &m.source, NS_MEDIA, json!({"type": "MEDIA_STATUS", "requestId": rid, "status": [{"mediaSessionId": 8, "playerState": state, "currentTime": at}]}));
                        continue;
                    }
                    state = "BUFFERING";
                    if !fake.no_fetch.load(Ordering::SeqCst) {
                        let f = fake.clone();
                        std::thread::spawn(move || fetch(&url, &f, n));
                    }
                    if fake.stale_session.load(Ordering::SeqCst) {
                        // What a TV sends when a run that ended without STOP left its media: the
                        // old session ends because of our LOAD (live test 4, `--http chunked`).
                        reply(&mut s, "web-1", "*", NS_MEDIA, json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [{"mediaSessionId": 3, "playerState": "IDLE", "idleReason": "INTERRUPTED"}]}));
                    }
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, media_status(state, 0.0));
                }
                ("media", "STOP") => {
                    state = "IDLE";
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, json!({"type": "MEDIA_STATUS", "requestId": rid, "status": [{"mediaSessionId": 7, "playerState": "IDLE", "idleReason": "CANCELLED"}]}));
                }
                ("media", "GET_STATUS") if fake.file.lock().unwrap().is_some() || v["mediaSessionId"] == 8 => {
                    let st = if file_over() {
                        json!([{"mediaSessionId": 8, "playerState": "IDLE", "idleReason": "FINISHED", "currentTime": playhead()}])
                    } else if fake.file.lock().unwrap().is_some() {
                        json!([{"mediaSessionId": 8, "playerState": if state == "PAUSED" { "PAUSED" } else { "PLAYING" }, "currentTime": playhead()}])
                    } else {
                        json!([{"mediaSessionId": 8, "playerState": "BUFFERING", "currentTime": 0.0}])
                    };
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, json!({"type": "MEDIA_STATUS", "requestId": rid, "status": st}));
                }
                ("media", "SEEK") => {
                    let at = v["currentTime"].as_f64().unwrap_or(0.0);
                    let mut f = fake.file.lock().unwrap();
                    if let Some((_, _, dur)) = *f {
                        *f = Some((at, Instant::now(), dur));
                    }
                    drop(f);
                    reply(&mut s, "web-1", &m.source, NS_MEDIA, json!({"type": "MEDIA_STATUS", "requestId": rid, "status": [{"mediaSessionId": 8, "playerState": "PLAYING", "currentTime": at}]}));
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
                fake.body.lock().unwrap().extend_from_slice(&head[p + 4..]);
            }
            continue;
        }
        {
            let mut b = fake.body.lock().unwrap();
            if b.len() < 4 << 20 {
                b.extend_from_slice(&buf[..n]);
            }
        }
        body += n;
        let frames = (body.saturating_sub(44) / 4) as u64;
        fake.fetched.store(frames, Ordering::SeqCst);
        if frames as f64 >= FAKE_BUFFER * 44100.0 {
            fake.play_start.lock().unwrap().get_or_insert_with(Instant::now);
        }
    }
}

/// Reads a file URL the way a receiver does: a HEAD, then two ranges. True when all came back right.
fn fetch_file(url: &str, fake: &Fake) -> bool {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let get = |range: &str| -> Option<(String, Vec<u8>)> {
        let mut c = TcpStream::connect(host).ok()?;
        write!(c, "GET /{path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: FakeCast\r\nRange: {range}\r\n\r\n").ok()?;
        let mut all = Vec::new();
        c.read_to_end(&mut all).ok()?;
        let p = all.windows(4).position(|w| w == b"\r\n\r\n")?;
        Some((String::from_utf8_lossy(&all[..p]).into_owned(), all[p + 4..].to_vec()))
    };
    let Some((h1, tail)) = get("bytes=4-") else { return false };
    let Some((h2, head)) = get("bytes=0-3") else { return false };
    let ok = h1.starts_with("HTTP/1.1 206") && h2.contains("Content-Range: bytes 0-3/");
    let mut whole = head;
    whole.extend(tail);
    *fake.body.lock().unwrap() = whole;
    ok
}

fn config(port: u16) -> Config {
    let mut cfg = Config::from_env();
    cfg.dir = std::env::temp_dir().join(format!("uzumaki-cast-test-{}-{port}", std::process::id()));
    cfg.port = 0;
    cfg.format = Format::Wav;
    cfg.fake = Some(CastDevice::at("127.0.0.1".parse().unwrap(), port, "Fake TV"));
    cfg.keep_local = false;
    cfg.fetch_timeout = Duration::from_secs(2);
    cfg.preroll_ms = Some(0);
    cfg.wav = crate::wav::Spec::CAPTURE;
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
    cfg.preroll_ms = Some(1500);
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
fn an_older_media_session_ending_is_not_someone_taking_the_device() {
    let fake = Arc::new(Fake::default());
    fake.stale_session.store(true, Ordering::SeqCst);
    let port = fake_device(fake.clone());
    let (c, events) = controller(config(port));
    c.connect_named("Fake TV", Meta::default());
    wait("playing", 15, || matches!(c.state(), State::Playing { .. }));
    assert!(!events.lock().unwrap().iter().any(|e| matches!(e, Event::Failed { .. })), "{:?}", events.lock().unwrap());
    // Stop: the media first, then the receiver, and the TV's word is awaited.
    c.stop();
    wait("off", 5, || c.idle());
    let j = fake.journal.lock().unwrap().clone();
    let media = j.iter().position(|x| x == "media:STOP").expect("media STOP sent");
    let receiver = j.iter().position(|x| x == "receiver:STOP").expect("receiver STOP sent");
    assert!(media < receiver, "{j:?}");
    c.shutdown();
}

#[test]
fn the_defaults_are_the_live_tests_best() {
    let cfg = Config::from_env();
    if std::env::var_os("UZUMAKI_CAST_HTTP").is_none() && std::env::var_os("UZUMAKI_CAST_RATE").is_none() && std::env::var_os("UZUMAKI_CAST_PREROLL_MS").is_none() {
        assert_eq!(cfg.http, Http::Length);
        assert_eq!(cfg.wav, crate::wav::Spec { rate: 96_000, bits: 24 });
        // 1.4 MB of 96k/24 (576 KB/s): about 2.4 s at once.
        assert_eq!(cfg.preroll(), 2430);
        let mut flac = cfg.clone();
        flac.format = Format::Flac;
        assert!(flac.preroll() > 10_000 - 1, "FLAC's bytes are few: the cap");
        let mut set = cfg;
        set.preroll_ms = Some(500);
        assert_eq!(set.preroll(), 500);
    }
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

fn loads_of(fake: &Fake) -> Vec<Value> {
    fake.load_msgs.lock().unwrap().clone()
}

fn has_event(events: &Events, f: impl Fn(&Event) -> bool) -> bool {
    events.lock().unwrap().iter().any(f)
}

/// A song that follows on by itself: the device first plays out what it holds of the old one,
/// then a new LOAD brings the new title, and the stream starts again at the new song's first frame.
#[test]
fn a_song_that_follows_on_waits_for_the_device_and_starts_at_its_first_frame() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    let mut cfg = config(port);
    cfg.preroll_ms = Some(1500);
    let (c, _events) = controller(cfg);
    c.connect_named("Fake TV", Meta { title: "One".into(), ..Meta::default() });
    wait("a measured latency", 15, || playing_latency(&c).is_some_and(|l| l > 1000));
    let behind = f64::from(playing_latency(&c).unwrap()) / 1000.0;
    // The player moves on to "Two" by itself; its first audio is recognisable.
    let t = Instant::now();
    assert!(c.play(Media::Live, Meta { title: "Two".into(), artist: "B".into(), ..Meta::default() }, 0.0, true));
    c.hub().publish(&[7u8; 4410 * pcm::FRAME_BYTES]);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(fake.loads.load(Ordering::SeqCst), 1, "no LOAD while the device still plays the old song");
    wait("the second LOAD", 10, || fake.loads.load(Ordering::SeqCst) == 2);
    let waited = t.elapsed().as_secs_f64();
    assert!(waited > behind - 0.6, "LOAD after {waited:.2} s, the device was {behind:.2} s behind");
    let load = loads_of(&fake)[1].clone();
    assert_eq!(load["media"]["metadata"]["title"], "Two");
    assert_eq!(load["media"]["metadata"]["artist"], "B");
    // The new stream begins just before the new song's first frame (the mark), not 1.5 s back.
    wait("the new song's audio", 10, || fake.body.lock().unwrap().windows(16).any(|w| w == [7u8; 16]));
    let body = fake.body.lock().unwrap().clone();
    let at = body.windows(16).position(|w| w == [7u8; 16]).unwrap().saturating_sub(44);
    let secs = at as f64 / (44100.0 * 4.0);
    assert!(secs < 0.6, "the new song starts {secs:.2} s into the new stream");
    c.stop();
    wait("off", 5, || c.idle());
    c.shutdown();
}

fn local_file(name: &str, len: usize) -> (std::path::PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("uzumaki-cast-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let p = dir.join(name);
    std::fs::write(&p, &bytes).unwrap();
    (p, bytes)
}

/// A local file the device plays itself: LOADed as BUFFERED, read with ranges, its position
/// reported for the app's player to follow, seeks are the receiver's own, and a song that follows
/// waits for the end of the file.
#[test]
fn a_local_file_plays_on_the_device_itself() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    let (c, events) = controller(config(port));
    c.connect_named("Fake TV", Meta::default());
    wait("playing", 15, || matches!(c.state(), State::Playing { .. }));
    let (path, bytes) = local_file("song.mp3", 200_000);
    let media = Media::local(&path, 3.0, "local:song");
    assert!(media.is_file());
    assert!(c.play(media, Meta { title: "Local".into(), ..Meta::default() }, 0.5, false));
    wait("the file playing", 10, || has_event(&events, |e| matches!(e, Event::Position { key, .. } if key == "local:song")));
    let load = loads_of(&fake)[1].clone();
    assert_eq!((load["media"]["streamType"].as_str(), load["media"]["contentType"].as_str()), (Some("BUFFERED"), Some("audio/mpeg")));
    assert_eq!((load["media"]["duration"].as_f64(), load["currentTime"].as_f64()), (Some(3.0), Some(0.5)));
    assert!(load["media"]["contentId"].as_str().unwrap().ends_with("/file.mp3"));
    assert_eq!(*fake.body.lock().unwrap(), bytes, "served whole, with ranges");
    wait("no delay", 5, || playing_latency(&c) == Some(0));
    // A seek: the receiver's own.
    assert!(c.seek(1.5, None));
    wait("the seek", 5, || fake.saw("media:SEEK"));
    assert_eq!(fake.loads.load(Ordering::SeqCst), 2, "no new LOAD for a seek");
    // The next song follows on: it waits for the file's end (about 1.5 s more).
    let t = Instant::now();
    assert!(c.play(Media::Live, Meta { title: "After".into(), ..Meta::default() }, 0.0, true));
    wait("the next LOAD", 10, || fake.loads.load(Ordering::SeqCst) == 3);
    assert!(t.elapsed() > Duration::from_millis(700), "LOAD after {:?}, before the file's end", t.elapsed());
    let load = loads_of(&fake)[2].clone();
    assert_eq!((load["media"]["streamType"].as_str(), load["media"]["metadata"]["title"].as_str()), (Some("LIVE"), Some("After")));
    wait("live again", 10, || matches!(c.state(), State::Playing { .. }));
    assert!(!has_event(&events, |e| matches!(e, Event::Failed { .. })), "{:?}", events.lock().unwrap());
    c.stop();
    wait("off", 5, || c.idle());
    c.shutdown();
}

#[test]
fn a_file_the_device_refuses_goes_through_the_live_stream() {
    let fake = Arc::new(Fake::default());
    fake.fail_files.store(true, Ordering::SeqCst);
    let port = fake_device(fake.clone());
    let (c, events) = controller(config(port));
    let (path, _) = local_file("refused.mp3", 1000);
    c.connect_named("Fake TV", Meta::default());
    wait("playing", 15, || matches!(c.state(), State::Playing { .. }));
    c.play(Media::local(&path, 10.0, "local:r"), Meta::default(), 0.0, false);
    wait("the live LOAD after the refusal", 10, || fake.loads.load(Ordering::SeqCst) == 3);
    assert_eq!(loads_of(&fake)[2]["media"]["streamType"], "LIVE");
    wait("playing live", 10, || playing_latency(&c).is_some_and(|l| l > 0));
    assert!(!has_event(&events, |e| matches!(e, Event::Failed { .. })), "{:?}", events.lock().unwrap());
    c.stop();
    wait("off", 5, || c.idle());
    c.shutdown();
}

/// Wi-Fi gone for a moment: the session reconnects, finds its receiver still running, and plays
/// again, saying how long the listener heard nothing.
#[test]
fn a_dropped_connection_comes_back() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    let mut cfg = config(port);
    cfg.reconnect_for = Duration::from_secs(10);
    let (c, events) = controller(cfg);
    c.connect_named("Fake TV", Meta::default());
    wait("playing", 15, || matches!(c.state(), State::Playing { .. }));
    fake.drop_conn.store(true, Ordering::SeqCst);
    wait("interrupted", 5, || has_event(&events, |e| matches!(e, Event::Interrupted { .. })));
    wait("reconnecting shown", 5, || has_event(&events, |e| matches!(e, Event::State(State::Reconnecting { .. }))));
    wait("playing again", 15, || fake.loads.load(Ordering::SeqCst) >= 2 && matches!(c.state(), State::Playing { .. }));
    assert!(fake.connections.load(Ordering::SeqCst) >= 2);
    assert_eq!(fake.count("receiver:LAUNCH"), 1, "the running receiver is used again");
    assert!(!has_event(&events, |e| matches!(e, Event::Failed { .. })), "{:?}", events.lock().unwrap());
    c.stop();
    wait("off", 5, || c.idle());
    c.shutdown();
}

/// The TV is switched off: nothing answers, the session tries again for a while, then ends with
/// a clear reason.
#[test]
fn a_device_that_goes_away_ends_after_trying_again() {
    let fake = Arc::new(Fake::default());
    let port = fake_device(fake.clone());
    let mut cfg = config(port);
    cfg.silence_timeout = Duration::from_secs(2);
    cfg.reconnect_for = Duration::from_secs(3);
    let (c, events) = controller(cfg);
    c.connect_named("Fake TV", Meta::default());
    wait("playing", 15, || matches!(c.state(), State::Playing { .. }));
    fake.off.store(true, Ordering::SeqCst);
    wait("interrupted", 8, || has_event(&events, |e| matches!(e, Event::Interrupted { silent_ms } if *silent_ms >= 1500)));
    wait("given up", 15, || has_event(&events, |e| matches!(e, Event::Failed { why: Why::Lost, .. })));
    wait("off", 5, || c.state() == State::Off && c.idle());
    assert!(events.lock().unwrap().contains(&Event::Capture(false)));
    assert!(Why::Lost.message("TV").contains("on this network"));
    c.shutdown();
}

#[test]
fn sleep_is_the_wall_clock_running_ahead() {
    assert!(!session::slept(Duration::from_millis(120), Duration::from_millis(100)));
    assert!(session::slept(Duration::from_secs(600), Duration::from_millis(100)));
}
