//! One Cast session: connect, launch the Default Media Receiver, serve and `LOAD` the stream (or a
//! local file the device plays itself), then follow the device's statuses until the session ends.
//! See `lib.rs` for the states.
//!
//! Song changes: a skip, a seek or a picked song starts the device afresh at once. A song that
//! follows on by itself waits until the device has played what it still holds of the old one
//! (its buffer, or the end of its file), then a new `LOAD` starts the new song with its own title
//! and cover; the live stream starts at the new song's first frame (`Route::Live { from }`), so
//! nothing is lost but a short pause between the songs.
//!
//! A lost connection (the TV off, Wi-Fi gone, this computer asleep) is tried again for
//! `Config::reconnect_for` (state `Reconnecting`) before the session ends.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::channel::{Channel, Incoming};
use crate::discover::CastDevice;
use crate::proto::{DEFAULT_MEDIA_RECEIVER, Message, NS_CONNECTION, NS_HEARTBEAT, NS_MEDIA, NS_RECEIVER, RECEIVER, SENDER};
use crate::health::Health;
use crate::stream::{Allow, Fetch, Opts, Route, Server, local_addr_for};
use crate::{Cmd, Event, Inner, Media, Meta, State, Why, log, pcm};

pub(crate) enum In {
    /// From the device connection of that generation (older ones are ignored after a reconnect).
    Net(u64, Incoming),
    Cmd(Cmd),
}

/// A natural song change, waiting for the device to reach it.
struct Pending {
    media: Media,
    meta: Meta,
    at: f64,
    /// The hub frame where the new song begins (the live stream starts again there).
    mark: u64,
    /// The live stream's time of that frame on the device's connection.
    served_mark: f64,
    /// Time spent playing since (a fallback, should the device's time not get there).
    played_for: Duration,
}

/// How much earlier than the song change the mark is set: the change is announced a moment after
/// the new song's audio began.
const MARK_EARLY: f64 = 0.25;
/// The longest a natural change waits (time spent playing) for the device to get there.
const FOLLOW_WAIT_LIVE: Duration = Duration::from_secs(20);
const FOLLOW_WAIT_FILE: Duration = Duration::from_secs(6);
/// Between reconnect attempts.
const RETRY_EVERY: Duration = Duration::from_secs(2);
/// The wall clock ran ahead of the monotonic one by this much: this computer slept.
const SLEPT: Duration = Duration::from_secs(5);

/// The wall clock moved `wall` while the monotonic clock (which stops in sleep) moved `mono`.
pub(crate) fn slept(wall: Duration, mono: Duration) -> bool {
    wall > mono + SLEPT
}

const PING_EVERY: Duration = Duration::from_secs(5);
/// How long a STOP waits for the TV to confirm it.
const STOP_WAIT: Duration = Duration::from_millis(2000);
const STATUS_EVERY: Duration = Duration::from_secs(2);
/// How many buffer measurements the reported latency is the median of.
const LATENCY_OF: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Connecting,
    Launching,
    Loading,
    Buffering,
    Playing,
    Paused,
}

struct Session<'a> {
    inner: &'a Arc<Inner>,
    device: CastDevice,
    ch: Option<Channel>,
    tx: mpsc::Sender<In>,
    /// The connection's generation (a reconnect makes a new one).
    generation: u64,
    /// What the device plays (or will, once loaded).
    media: Media,
    /// Where a file starts at its next LOAD.
    file_at: f64,
    pending: Option<Pending>,
    /// The device reached the end of its file.
    finished: bool,
    /// The last PLAYING status: when, and the device's time then.
    last_play: Option<(Instant, f64)>,
    device_time: f64,
    /// The device played once this session (a lost connection is then worth trying again).
    ever_played: bool,
    /// Reconnecting until then (None: connected as usual).
    reconnect_until: Option<Instant>,
    clocks: (SystemTime, Instant),
    phase: Phase,
    next_id: u64,
    /// Our receiver app: its session and transport ids.
    app: Option<(String, String)>,
    media_session: Option<i64>,
    /// The requestId of our last LOAD: its MEDIA_STATUS names our media session.
    load_req: Option<u64>,
    server: Option<Server>,
    inputs: Option<pcm::Inputs>,
    meta: Meta,
    load_at: Option<Instant>,
    fetch: Option<Arc<Fetch>>,
    fetch_at: Option<Instant>,
    played: bool,
    health: Health,
    buffers: Vec<i64>,
    last_heard: Instant,
    last_ping: Instant,
    last_status: Instant,
    phase_since: Instant,
    shown: State,
    volume: Option<(f64, bool)>,
    /// A LOAD of ours replaced the previous one: its IDLE/INTERRUPTED is expected.
    reloading: bool,
}

enum End {
    User,
    Failed(Why, String),
}

pub(crate) fn run(inner: &Arc<Inner>, mut device: CastDevice, meta: Meta, rx: &mpsc::Receiver<In>, tx: mpsc::Sender<In>) {
    let name = device.name.clone();
    match crate::discover::choose(&device.addrs, inner.cfg.family, &crate::discover::local_v4()) {
        Some((ip, why)) => {
            log(format!("using {ip} ({why}); the device advertises {}", device.addrs.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
            device.ip = ip;
        }
        None => {
            let detail = format!("no {:?} address among {:?}", inner.cfg.family, device.addrs);
            log(format!("can't connect: {detail}"));
            inner.emit(Event::State(State::Off));
            return inner.emit(Event::Failed { name, why: Why::Unreachable, detail });
        }
    }
    inner.emit(Event::State(State::Connecting { name: name.clone() }));
    {
        let mut r = inner.report.lock().unwrap();
        *r = crate::Report { device: Some(format!("“{}” {} at {}:{}{}", device.name, device.model, device.ip, device.port, if device.group { " (Cast Group)" } else { "" })), ..Default::default() };
    }
    log(format!("connecting to “{}” ({}, {}:{}{})", device.name, device.model, device.ip, device.port, if device.group { ", a Cast Group" } else { "" }));
    let addr = SocketAddr::new(device.ip, device.port);
    let ch = match open(addr, inner.cfg.connect_timeout, &tx, 1) {
        Ok(c) => c,
        Err(e) => {
            log(format!("can't connect: {e}"));
            inner.emit(Event::State(State::Off));
            return inner.emit(Event::Failed { name, why: Why::Unreachable, detail: e });
        }
    };
    let now = Instant::now();
    let mut s = Session {
        inner,
        device,
        ch: Some(ch),
        tx,
        generation: 1,
        media: Media::Live,
        file_at: 0.0,
        pending: None,
        finished: false,
        last_play: None,
        device_time: 0.0,
        ever_played: false,
        reconnect_until: None,
        clocks: (SystemTime::now(), now),
        phase: Phase::Connecting,
        next_id: 1,
        app: None,
        media_session: None,
        load_req: None,
        server: None,
        inputs: None,
        meta,
        load_at: None,
        fetch: None,
        fetch_at: None,
        played: false,
        health: Health::new(now),
        buffers: Vec::new(),
        last_heard: now,
        last_ping: now,
        last_status: now,
        phase_since: now,
        shown: State::Off,
        volume: None,
        reloading: false,
    };
    s.hello();
    loop {
        let end = s.run(rx);
        let end = match end {
            End::Failed(Why::Lost, why) if s.ever_played && !inner.closed.load(std::sync::atomic::Ordering::SeqCst) => match s.reconnect(rx, &why) {
                Ok(()) => continue,
                Err(end) => end,
            },
            end => end,
        };
        return s.finish(end, rx);
    }
}

fn open(addr: SocketAddr, timeout: Duration, tx: &mpsc::Sender<In>, generation: u64) -> Result<Channel, String> {
    let t = tx.clone();
    Channel::open_tls(addr, timeout, move |m| {
        let _ = t.send(In::Net(generation, m));
    })
}

impl Session<'_> {
    fn hello(&mut self) {
        self.send(RECEIVER, NS_CONNECTION, json!({"type": "CONNECT", "origin": {}, "userAgent": "Uzumaki", "senderInfo": {"sdkType": 2, "version": "15.605.1.3", "platform": 6, "systemVersion": "", "connectionType": 1}}));
        let id = self.id();
        self.send(RECEIVER, NS_RECEIVER, json!({"type": "GET_STATUS", "requestId": id}));
    }

    /// The connection is gone: tries again (new address from mDNS too: the TV may have a new one
    /// after a restart) until `reconnect_for` is over. Ok: connected again, `run` goes on.
    fn reconnect(&mut self, rx: &mpsc::Receiver<In>, why: &str) -> Result<(), End> {
        let cfg = self.inner.cfg.clone();
        let silent = self.last_heard.elapsed();
        if self.reconnect_until.is_none() {
            log(format!("lost “{}” ({why}; nothing heard for {} ms): reconnecting for up to {} s", self.name(), silent.as_millis(), cfg.reconnect_for.as_secs()));
            self.reconnect_until = Some(Instant::now() + cfg.reconnect_for);
            self.inner.emit(Event::Interrupted { silent_ms: silent.as_millis() as u64 });
        } else {
            log(format!("lost “{}” again while reconnecting ({why})", self.name()));
        }
        let until = self.reconnect_until.unwrap();
        // What the device played is gone with it: start again where it was.
        if let Some(p) = self.pending.take() {
            self.media = p.media;
            self.meta = p.meta;
            self.file_at = p.at;
        } else if self.media.is_file() {
            self.file_at = self.device_now().unwrap_or(self.device_time);
        }
        self.drop_channel();
        self.app = None;
        self.media_session = None;
        self.load_req = None;
        self.load_at = None;
        self.fetch = None;
        self.played = false;
        self.last_play = None;
        self.buffers.clear();
        self.set_phase(Phase::Connecting);
        let name = self.name();
        let mut attempt = 0u32;
        loop {
            // Commands meanwhile: Stop ends it; songs and seeks are kept for the next LOAD.
            let wait = if attempt == 0 { Duration::ZERO } else { RETRY_EVERY };
            let t = Instant::now();
            loop {
                match rx.recv_timeout(wait.saturating_sub(t.elapsed())) {
                    Ok(In::Cmd(Cmd::Stop)) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err(End::User),
                    Ok(In::Cmd(c)) => {
                        let _ = self.on_cmd(c);
                    }
                    Ok(In::Net(..)) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                }
            }
            if Instant::now() >= until {
                return Err(End::Failed(Why::Lost, format!("{why}; no answer for {} s", cfg.reconnect_for.as_secs())));
            }
            attempt += 1;
            if attempt > 1
                && cfg.fake.is_none()
                && let Some(d) = crate::discover::discover(Duration::from_secs(3), Some(&name)).ok().and_then(|l| crate::discover::pick(&l, &name).cloned())
            {
                if d.addrs != self.device.addrs || d.port != self.device.port {
                    log(format!("“{name}” answers at {} now", d.addrs.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
                }
                self.device.addrs = d.addrs;
                self.device.port = d.port;
            }
            let Some((ip, _)) = crate::discover::choose(&self.device.addrs, cfg.family, &crate::discover::local_v4()) else { continue };
            self.device.ip = ip;
            self.generation += 1;
            match open(SocketAddr::new(ip, self.device.port), cfg.connect_timeout, &self.tx, self.generation) {
                Ok(ch) => {
                    log(format!("reconnected to “{name}” at {ip} (attempt {attempt})"));
                    self.ch = Some(ch);
                    // This computer may have a new address too: the server moves with it.
                    if self.server.as_ref().is_some_and(|s| Some(s.addr().ip()) != local_addr_for(ip)) {
                        log("this computer's address changed: a new stream server");
                        self.server = None;
                    }
                    let now = Instant::now();
                    self.last_heard = now;
                    self.last_ping = now;
                    self.last_status = now;
                    self.phase_since = now;
                    self.clocks = (SystemTime::now(), now);
                    self.hello();
                    return Ok(());
                }
                Err(e) => log(format!("reconnect attempt {attempt}: {e}")),
            }
        }
    }

    /// Closes the connection without waiting for it (a dead one may take its write timeout).
    fn drop_channel(&mut self) {
        if let Some(ch) = self.ch.take() {
            let _ = std::thread::Builder::new().name("cast-channel-drop".into()).spawn(move || drop(ch));
        }
    }

    /// The device's position now, while it plays (extrapolated from its last PLAYING status).
    fn device_now(&self) -> Option<f64> {
        self.last_play.map(|(t, at)| at + t.elapsed().as_secs_f64())
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn send(&self, dest: &str, ns: &str, payload: Value) {
        if let Some(ch) = &self.ch {
            ch.send(&Message::new(SENDER, dest, ns, &payload));
        }
    }

    fn name(&self) -> String {
        self.device.name.clone()
    }

    fn set_phase(&mut self, p: Phase) {
        if self.phase != p {
            self.phase = p;
            self.phase_since = Instant::now();
        }
        self.show();
    }

    fn latency_ms(&self) -> Option<u32> {
        if self.media.is_file() && self.played {
            return Some(0); // the device is the player
        }
        if self.buffers.is_empty() {
            return None;
        }
        let mut last: Vec<i64> = self.buffers.iter().rev().take(LATENCY_OF).copied().collect();
        last.sort_unstable();
        Some(last[last.len() / 2].max(0) as u32)
    }

    fn show(&mut self) {
        let name = self.name();
        let st = match self.phase {
            _ if self.reconnect_until.is_some() && self.phase < Phase::Playing => State::Reconnecting { name },
            Phase::Connecting => State::Connecting { name },
            Phase::Launching => State::Launching { name },
            Phase::Loading => State::Loading { name },
            Phase::Buffering => State::Buffering { name },
            Phase::Playing => State::Playing { name, latency_ms: self.latency_ms() },
            Phase::Paused => State::Paused { name },
        };
        // Latency changes of under 50 ms are not worth an event.
        let same = match (&st, &self.shown) {
            (State::Playing { latency_ms: Some(a), .. }, State::Playing { latency_ms: Some(b), .. }) => a.abs_diff(*b) < 50,
            (a, b) => a == b,
        };
        if !same {
            self.shown = st.clone();
            self.inner.emit(Event::State(st));
        }
    }

    fn run(&mut self, rx: &mpsc::Receiver<In>) -> End {
        let cfg = self.inner.cfg.clone();
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(In::Net(g, _)) if g != self.generation => {}
                Ok(In::Net(_, Incoming::Msg(m))) => {
                    self.last_heard = Instant::now();
                    if let Some(end) = self.on_message(&m) {
                        return end;
                    }
                }
                Ok(In::Net(_, Incoming::Closed(why))) => return End::Failed(Why::Lost, why),
                Ok(In::Cmd(c)) => {
                    if let Some(end) = self.on_cmd(c) {
                        return end;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return End::User,
            }
            let now = Instant::now();
            let wall = SystemTime::now();
            let (w0, m0) = std::mem::replace(&mut self.clocks, (wall, now));
            let dt = now.duration_since(m0);
            if slept(wall.duration_since(w0).unwrap_or_default(), dt) {
                // The connection is most likely dead, and the device stopped long ago.
                self.last_heard = m0;
                return End::Failed(Why::Lost, "this computer slept".into());
            }
            if self.phase == Phase::Playing
                && let Some(p) = &mut self.pending
            {
                p.played_for += dt;
            }
            if self.pending_due() {
                self.apply_pending();
            }
            if now.duration_since(self.last_ping) >= PING_EVERY {
                self.last_ping = now;
                self.send(RECEIVER, NS_HEARTBEAT, json!({"type": "PING"}));
            }
            if now.duration_since(self.last_heard) >= cfg.silence_timeout {
                return End::Failed(Why::Lost, format!("nothing heard for {} s", cfg.silence_timeout.as_secs()));
            }
            if matches!(self.phase, Phase::Connecting | Phase::Launching) && now.duration_since(self.phase_since) >= cfg.launch_timeout {
                return End::Failed(Why::LoadFailed, "the receiver didn't start".into());
            }
            self.watch_fetch(now);
            if self.phase == Phase::Loading
                && let Some(t) = self.load_at
                && self.fetch.is_none()
                && now.duration_since(t) >= cfg.fetch_timeout
            {
                return End::Failed(Why::NoFetch, format!("no fetch of the stream within {} s", cfg.fetch_timeout.as_secs()));
            }
            if self.phase >= Phase::Loading
                && now.duration_since(self.last_status) >= STATUS_EVERY
                && let Some((_, transport)) = self.app.clone()
            {
                self.last_status = now;
                let id = self.id();
                let mut msg = json!({"type": "GET_STATUS", "requestId": id});
                if let Some(ms) = self.media_session {
                    msg["mediaSessionId"] = json!(ms);
                }
                self.send(&transport, NS_MEDIA, msg);
            }
        }
    }

    /// The device fetched the current URL (first time since the LOAD).
    fn watch_fetch(&mut self, now: Instant) {
        let Some(server) = &self.server else { return };
        let newest = server.newest();
        let is_new = match (&newest, &self.fetch) {
            (Some(n), Some(o)) => !Arc::ptr_eq(n, o),
            (Some(_), None) => true,
            _ => false,
        };
        if !is_new {
            return;
        }
        let f = newest.unwrap();
        let after = self.load_at.map(|t| now.duration_since(t).as_millis() as u64);
        if self.fetch.is_none() {
            log(format!("the device fetched the stream {} ms after LOAD (from {})", after.unwrap_or(0), f.peer));
            let mut r = self.inner.report.lock().unwrap();
            r.fetched_by = Some(f.peer);
            r.load_to_fetch_ms = after;
        } else {
            log(format!("the device fetched the stream again (from {}): a reconnect", f.peer));
        }
        if self.fetch_at.is_none() {
            self.fetch_at = Some(now);
        }
        self.fetch = Some(f);
        if self.phase == Phase::Loading {
            self.set_phase(Phase::Buffering);
        }
    }

    fn on_cmd(&mut self, c: Cmd) -> Option<End> {
        let ms = self.media_session;
        let transport = self.app.as_ref().map(|a| a.1.clone());
        match c {
            Cmd::Stop => return Some(End::User),
            Cmd::Pause | Cmd::Resume => {
                let (Some(ms), Some(t)) = (ms, transport) else {
                    log("pause/resume: nothing loaded yet");
                    return None;
                };
                let kind = if matches!(c, Cmd::Pause) { "PAUSE" } else { "PLAY" };
                log(format!("{kind} sent to the receiver (the stream keeps going)"));
                let id = self.id();
                self.send(&t, NS_MEDIA, json!({"type": kind, "mediaSessionId": ms, "requestId": id}));
            }
            Cmd::Volume(level) => {
                log(format!("device volume → {:.0}%", level * 100.0));
                let id = self.id();
                self.send(RECEIVER, NS_RECEIVER, json!({"type": "SET_VOLUME", "volume": {"level": level}, "requestId": id}));
            }
            Cmd::Reload(meta) => {
                if let Some(m) = meta {
                    self.meta = m;
                }
                self.pending = None;
                if self.media.is_file() {
                    self.file_at = self.device_now().unwrap_or(self.device_time);
                }
                if self.ready() {
                    log("reload: new URL, new LOAD");
                    self.load(None);
                }
            }
            Cmd::Play { media, meta, at, follow } => {
                let follow = follow && self.ready() && self.played && !(self.media.is_file() && self.finished);
                if follow {
                    let mark = self.inner.hub.published().saturating_sub((MARK_EARLY * pcm::RATE as f64) as u64);
                    let served_mark = match (&self.media, &self.fetch) {
                        (Media::Live, Some(f)) => f.served_secs() - MARK_EARLY,
                        _ => f64::INFINITY,
                    };
                    // Two changes before the device got to the first: the earlier mark stays.
                    let (mark, served_mark) = self.pending.as_ref().map_or((mark, served_mark), |p| (p.mark, p.served_mark));
                    log(format!("the next song follows on: “{}” once the device has played out the current one", meta.title));
                    self.pending = Some(Pending { media, meta, at, mark, served_mark, played_for: Duration::ZERO });
                    if self.pending_due() {
                        self.apply_pending();
                    }
                } else {
                    self.pending = None;
                    self.media = media;
                    self.meta = meta;
                    self.file_at = at;
                    if self.ready() {
                        self.load(None);
                    }
                }
            }
            Cmd::Seek { at, meta } => {
                if let Some(m) = meta {
                    self.meta = m;
                }
                self.pending = None;
                if !self.media.is_file() {
                    self.inner.hub.forget();
                    if self.ready() {
                        self.load(None);
                    }
                } else if let (Some(ms), Some(t), true) = (self.media_session, transport, self.played && !self.finished) {
                    log(format!("SEEK to {at:.1} s"));
                    let id = self.id();
                    self.send(&t, NS_MEDIA, json!({"type": "SEEK", "mediaSessionId": ms, "requestId": id, "currentTime": at}));
                    self.last_play = None;
                    self.device_time = at;
                } else {
                    self.file_at = at;
                    if self.ready() {
                        self.load(None);
                    }
                }
            }
        }
        None
    }

    /// The receiver runs and the server is up: a LOAD can go out.
    fn ready(&self) -> bool {
        self.server.is_some() && self.app.is_some()
    }

    /// The device got to the pending song change.
    fn pending_due(&self) -> bool {
        let Some(p) = &self.pending else { return false };
        if !self.ready() || !self.played {
            return true;
        }
        match &self.media {
            Media::Live => self.device_now().is_some_and(|t| t >= p.served_mark) || p.played_for >= FOLLOW_WAIT_LIVE,
            Media::File(f) => self.finished || (f.duration > 0.0 && self.device_now().is_some_and(|t| t >= f.duration - 0.3)) || p.played_for >= FOLLOW_WAIT_FILE,
        }
    }

    fn apply_pending(&mut self) {
        let Some(p) = self.pending.take() else { return };
        let from = (self.media.is_file() || self.played).then_some(p.mark);
        self.media = p.media;
        self.meta = p.meta;
        self.file_at = p.at;
        if self.ready() {
            log(format!("the device got to the next song: LOAD “{}”", self.meta.title));
            self.load(if self.media.is_file() { None } else { from });
        }
    }

    fn on_message(&mut self, m: &Message) -> Option<End> {
        let v = m.json().unwrap_or(Value::Null);
        let kind = v["type"].as_str().unwrap_or("");
        match (m.namespace.as_str(), kind) {
            (NS_CONNECTION, "CLOSE") => {
                if self.app.as_ref().is_some_and(|a| a.1 == m.source) {
                    return Some(End::Failed(Why::Ended, "the receiver closed our connection".into()));
                }
                return Some(End::Failed(Why::Lost, "the device closed the connection".into()));
            }
            (NS_RECEIVER, "RECEIVER_STATUS") => return self.on_receiver_status(&v["status"]),
            (NS_RECEIVER, "LAUNCH_ERROR") => return Some(End::Failed(Why::LoadFailed, format!("LAUNCH_ERROR {}", v["reason"]))),
            (NS_MEDIA, "MEDIA_STATUS") => return self.on_media_status(&v["status"], v["requestId"].as_u64()),
            (NS_MEDIA, "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST" | "INVALID_PLAYER_STATE") => {
                if kind == "LOAD_CANCELLED" && self.reloading {
                    return None;
                }
                log(format!("{kind}: {}", m.payload));
                if kind == "LOAD_FAILED" && self.file_failed() {
                    return None;
                }
                if matches!(kind, "LOAD_FAILED" | "LOAD_CANCELLED") || self.phase < Phase::Buffering {
                    return Some(End::Failed(Why::LoadFailed, format!("{kind} {}", v["reason"].as_str().unwrap_or(""))));
                }
            }
            _ => {}
        }
        None
    }

    fn on_receiver_status(&mut self, st: &Value) -> Option<End> {
        if let Some(vol) = st.get("volume") {
            let now = (vol["level"].as_f64().unwrap_or(0.0), vol["muted"].as_bool().unwrap_or(false));
            if self.volume != Some(now) {
                self.volume = Some(now);
                self.inner.report.lock().unwrap().volume = Some(now);
                self.inner.emit(Event::Volume { level: now.0, muted: now.1 });
            }
        }
        let apps = st["applications"].as_array().cloned().unwrap_or_default();
        let ours = apps.iter().find(|a| a["appId"].as_str() == Some(DEFAULT_MEDIA_RECEIVER));
        let other = apps.iter().find(|a| a["appId"].as_str() != Some(DEFAULT_MEDIA_RECEIVER) && !a["isIdleScreen"].as_bool().unwrap_or(false));
        match &self.app {
            None => match ours {
                Some(a) => {
                    let (sid, tid) = (a["sessionId"].as_str().unwrap_or("").to_string(), a["transportId"].as_str().unwrap_or("").to_string());
                    if sid.is_empty() || tid.is_empty() {
                        return None;
                    }
                    log(format!("the Default Media Receiver runs (session {sid}){}", if self.phase == Phase::Connecting { ", already" } else { "" }));
                    self.app = Some((sid, tid.clone()));
                    self.send(&tid, NS_CONNECTION, json!({"type": "CONNECT", "origin": {}, "userAgent": "Uzumaki"}));
                    if let Err(e) = self.start_stream() {
                        return Some(End::Failed(Why::LoadFailed, e));
                    }
                    self.pending = None;
                    self.load(None);
                }
                None if self.phase == Phase::Connecting => {
                    if let Some(o) = other {
                        log(format!("the device runs {} now; launching the Default Media Receiver in its place", o["displayName"].as_str().unwrap_or("another app")));
                    }
                    self.set_phase(Phase::Launching);
                    let id = self.id();
                    self.send(RECEIVER, NS_RECEIVER, json!({"type": "LAUNCH", "appId": DEFAULT_MEDIA_RECEIVER, "requestId": id}));
                }
                None => {}
            },
            Some((sid, _)) => {
                let still = ours.is_some_and(|a| a["sessionId"].as_str() == Some(sid));
                if !still {
                    return Some(match other {
                        Some(o) => End::Failed(Why::Taken, format!("now {}", o["displayName"].as_str().unwrap_or("another app"))),
                        None => End::Failed(Why::Ended, "the receiver was stopped".into()),
                    });
                }
            }
        }
        None
    }

    /// The HTTP server (and the app's inputs).
    fn start_stream(&mut self) -> Result<(), String> {
        if self.server.is_none() {
            self.start_server()?;
        }
        let cfg = &self.inner.cfg;
        if cfg.app_inputs && self.inputs.is_none() {
            self.inputs = Some(pcm::Inputs::start(&cfg.socket(), self.inner.feed.clone(), self.inner.hub.clone()));
            self.inner.feed.set_casting(true);
            self.inner.emit(Event::Capture(true));
        }
        Ok(())
    }

    fn start_server(&mut self) -> Result<(), String> {
        let cfg = &self.inner.cfg;
        let bind = local_addr_for(self.device.ip).unwrap_or(self.device.ip);
        // Served over IPv6, the device may fetch from a temporary address of its own: its /64 too.
        let mut allow = Allow::device(&self.device.addrs, bind.is_ipv6());
        if self.device.group {
            // A group's leader may let a member fetch: every Cast device seen may.
            allow.add_others(&self.inner.known_ips());
        }
        match Server::start(bind, cfg.port, Opts { format: cfg.format, http: cfg.http, preroll_ms: cfg.preroll(), wav: cfg.wav }, self.inner.hub.clone(), allow.clone()) {
            Ok(s) => {
                log(format!("serving to {}", allow.describe()));
                let mut r = self.inner.report.lock().unwrap();
                r.url_host = Some(s.addr().to_string());
                r.format = Some(cfg.format);
                r.setup = format!("setup: {}, HTTP {}, pre-roll {} ms{}, streamType {}, {}", if cfg.format == crate::Format::Wav { format!("wav {}", cfg.wav.label()) } else { "flac 44.1k/16".into() }, cfg.http.name(), cfg.preroll(), if cfg.preroll_ms.is_none() { format!(" (auto: {} KB)", cfg.burst_bytes / 1000) } else { String::new() }, cfg.stream_type, if bind.is_ipv6() { "IPv6" } else { "IPv4" });
                drop(r);
                self.server = Some(s);
            }
            Err(e) => return Err(format!("can't start the stream server on {bind}:{}: {e}", cfg.port)),
        }
        Ok(())
    }

    /// LOADs `self.media` (a new URL each time). `from`: the live stream starts at that hub frame.
    fn load(&mut self, from: Option<u64>) {
        let (Some(server), Some((sid, transport))) = (&self.server, self.app.clone()) else { return };
        self.reloading = self.load_req.is_some();
        let route = match &self.media {
            Media::Live => Route::Live { from },
            Media::File(f) => Route::File { path: f.path.clone(), content_type: f.content_type.clone() },
        };
        let url = server.rotate_to(route);
        let cfg = &self.inner.cfg;
        let m = &self.meta;
        let mut metadata = json!({"metadataType": 3, "title": if m.title.is_empty() { "Uzumaki" } else { &m.title }});
        if !m.artist.is_empty() {
            metadata["artist"] = json!(m.artist);
        }
        if !m.album.is_empty() {
            metadata["albumName"] = json!(m.album);
        }
        if let Some(img) = &m.image {
            metadata["images"] = json!([{"url": img}]);
        }
        let id = self.id();
        let shown_url = url.rsplit_once("/cast/").map_or(url.clone(), |(h, t)| format!("{h}/cast/<token>/{}", t.rsplit('/').next().unwrap_or("")));
        let (stream_type, content_type, at) = match &self.media {
            Media::Live => (cfg.stream_type.clone(), cfg.format.content_type().to_string(), 0.0),
            Media::File(f) => ("BUFFERED".to_string(), f.content_type.clone(), self.file_at.max(0.0)),
        };
        let what = match (&self.media, from) {
            (Media::File(f), _) => format!("the file {:?} at {at:.1} s", f.path.file_name().unwrap_or_default()),
            (Media::Live, Some(f)) => format!("the live stream from the new song's start ({:.1} s back)", self.inner.hub.published().saturating_sub(f) as f64 / pcm::RATE as f64),
            (Media::Live, None) => "the live stream".into(),
        };
        log(format!("LOAD {shown_url} ({content_type}, {stream_type}): {what}"));
        let mut media = json!({"contentId": url, "contentUrl": url, "streamType": stream_type, "contentType": content_type, "metadata": metadata});
        if let Media::File(f) = &self.media
            && f.duration > 0.0
        {
            media["duration"] = json!(f.duration);
        }
        self.send(&transport, NS_MEDIA, json!({"type": "LOAD", "requestId": id, "sessionId": sid, "autoplay": !self.inner.feed.holding(), "currentTime": at, "media": media}));
        self.finished = false;
        self.last_play = None;
        self.device_time = at;
        self.load_at = Some(Instant::now());
        self.health = Health::new(Instant::now());
        self.fetch = None;
        self.fetch_at = None;
        self.played = false;
        self.buffers.clear();
        self.media_session = None;
        self.load_req = Some(id);
        {
            let mut r = self.inner.report.lock().unwrap();
            r.loads += 1;
            r.fetched_by = None;
            r.load_to_fetch_ms = None;
            r.fetch_to_playing_ms = None;
            r.load_to_playing_ms = None;
            r.buffer_ms.clear();
        }
        self.set_phase(Phase::Loading);
    }

    fn on_media_status(&mut self, st: &Value, request: Option<u64>) -> Option<End> {
        let s = st.as_array().and_then(|a| a.first())?;
        let ms = s["mediaSessionId"].as_i64();
        let reply_to_load = self.load_req.is_some() && request == self.load_req;
        match (self.media_session, ms) {
            // The answer to our LOAD names our media session.
            (_, Some(m)) if reply_to_load => self.media_session = Some(m),
            (None, Some(m)) if self.load_req.is_some() && s["playerState"].as_str() != Some("IDLE") => self.media_session = Some(m),
            (Some(ours), Some(m)) if m != ours => {
                log(format!("a status of another media session ({m}, not ours {ours}): {} {}, ignored", s["playerState"].as_str().unwrap_or(""), s["idleReason"].as_str().unwrap_or("")));
                return None;
            }
            (None, Some(m)) => {
                // Before ours is known: an older session (a run that ended without STOP) ending
                // because of our LOAD. Not someone else taking the device.
                log(format!("a status of an older media session ({m}): {} {}, ignored", s["playerState"].as_str().unwrap_or(""), s["idleReason"].as_str().unwrap_or("")));
                return None;
            }
            _ => {}
        }
        let state = s["playerState"].as_str().unwrap_or("");
        let device_time = s["currentTime"].as_f64().unwrap_or(0.0);
        let now = Instant::now();
        self.device_time = device_time;
        self.last_play = (state == "PLAYING").then_some((now, device_time));
        if let Media::File(f) = &self.media {
            return self.on_file_status(state, device_time, s["idleReason"].as_str().unwrap_or(""), f.key.clone());
        }
        // The status may arrive before our poll saw the fetch.
        self.watch_fetch(now);
        let served = self.fetch.as_ref().map_or(0.0, |f| f.served_secs());
        {
            let mut r = self.inner.report.lock().unwrap();
            r.player_state = state.to_string();
            r.served_s = served;
            r.device_time_s = device_time;
        }
        for note in self.health.observe(now, state, device_time, served) {
            log(note.line());
        }
        {
            let mut r = self.inner.report.lock().unwrap();
            r.underruns = self.health.underruns;
            r.stalls = self.health.stalls;
            r.load_to_audio_ms = self.health.load_to_audio_ms;
        }
        match state {
            "PLAYING" => {
                self.reloading = false;
                self.connected_again();
                if !self.played {
                    self.played = true;
                    let ms = |t: Option<Instant>| t.map(|t| now.duration_since(t).as_millis() as u64);
                    let (l2p, f2p) = (ms(self.load_at), ms(self.fetch_at));
                    log(format!("PLAYING: {} ms after LOAD, {} ms after the fetch", l2p.unwrap_or(0), f2p.map_or("?".into(), |v| v.to_string())));
                    let mut r = self.inner.report.lock().unwrap();
                    r.load_to_playing_ms = l2p;
                    r.fetch_to_playing_ms = f2p;
                }
                // Only once the position moves: PLAYING at 0 is the device still waiting.
                if self.fetch.is_some() && self.health.audible() {
                    let buffer = ((served - device_time) * 1000.0).round() as i64;
                    self.buffers.push(buffer);
                    if self.buffers.len() > 50 {
                        self.buffers.remove(0);
                    }
                    log(format!("buffer {buffer} ms (served {served:.2} s, device at {device_time:.2} s)"));
                    let mut r = self.inner.report.lock().unwrap();
                    r.buffer_ms = self.buffers.clone();
                }
                self.set_phase(Phase::Playing);
            }
            "BUFFERING" => {
                log(format!("BUFFERING (served {served:.2} s, device at {device_time:.2} s)"));
                if self.phase != Phase::Loading || self.fetch.is_some() {
                    self.set_phase(Phase::Buffering);
                }
            }
            "PAUSED" => {
                log(format!("PAUSED (served {served:.2} s, device at {device_time:.2} s)"));
                self.connected_again();
                self.set_phase(Phase::Paused);
            }
            "IDLE" => {
                let why = s["idleReason"].as_str().unwrap_or("");
                log(format!("IDLE {why} (served {served:.2} s, device at {device_time:.2} s)"));
                match why {
                    "INTERRUPTED" | "CANCELLED" if self.reloading => {}
                    "ERROR" => return Some(End::Failed(Why::LoadFailed, "the receiver reported an error".into())),
                    "FINISHED" if !self.reloading => {
                        // The Content-Length is used up (or the receiver dropped the stream): a new URL.
                        log("the receiver reached the end of the stream: loading a new URL");
                        self.reloading = true;
                        self.load(None);
                    }
                    "FINISHED" => {}
                    "INTERRUPTED" | "CANCELLED" => return Some(End::Failed(Why::Taken, format!("IDLE {why}"))),
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    /// Statuses of a file the device plays itself: no buffer to measure, the device is the player.
    fn on_file_status(&mut self, state: &str, at: f64, idle: &str, key: String) -> Option<End> {
        match state {
            "PLAYING" => {
                self.reloading = false;
                self.connected_again();
                if !self.played {
                    self.played = true;
                    let ms = self.load_at.map_or(0, |t| t.elapsed().as_millis());
                    log(format!("PLAYING the file: {ms} ms after LOAD, at {at:.2} s"));
                    self.inner.report.lock().unwrap().load_to_playing_ms = Some(ms as u64);
                }
                self.inner.emit(Event::Position { key, at });
                self.set_phase(Phase::Playing);
            }
            "PAUSED" => {
                self.connected_again();
                self.set_phase(Phase::Paused);
            }
            "BUFFERING" => {
                if self.phase != Phase::Paused {
                    self.set_phase(Phase::Buffering);
                }
            }
            "IDLE" => {
                log(format!("IDLE {idle} (the file, at {at:.2} s)"));
                match idle {
                    "FINISHED" => {
                        self.finished = true;
                        if self.pending_due() {
                            self.apply_pending();
                        }
                    }
                    "INTERRUPTED" | "CANCELLED" if self.reloading => {}
                    "ERROR" => {
                        if !self.file_failed() {
                            return Some(End::Failed(Why::LoadFailed, "the receiver reported an error".into()));
                        }
                    }
                    "INTERRUPTED" | "CANCELLED" => return Some(End::Failed(Why::Taken, format!("IDLE {idle}"))),
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    /// The device can't play the file it was given: the live stream instead (the local player's
    /// own output, as before files were sent). True when that was done.
    fn file_failed(&mut self) -> bool {
        let Media::File(f) = &self.media else { return false };
        log(format!("the device can't play {:?} ({}): the live stream instead", f.path.file_name().unwrap_or_default(), f.content_type));
        self.media = Media::Live;
        self.load(None);
        true
    }

    /// The local outputs play again and the bridge stops capturing for us.
    fn release_outputs(&mut self) {
        if self.inputs.is_some() {
            self.inner.feed.set_casting(false);
            self.inputs = None;
            self.inner.emit(Event::Capture(false));
        }
    }

    /// Playing again after a lost connection.
    fn connected_again(&mut self) {
        self.ever_played = true;
        if self.reconnect_until.take().is_some() {
            log(format!("“{}” plays again", self.name()));
        }
    }

    fn finish(mut self, end: End, rx: &mpsc::Receiver<In>) {
        let name = self.name();
        self.inner.feed.set_hold(false);
        // Our media stops first (else the TV plays out its buffer: ~8 beeps after `q` in live
        // test 3), then our receiver goes away; we wait for the TV to say so.
        if let Some((sid, transport)) = self.app.clone()
            && !matches!(end, End::Failed(Why::Taken | Why::Ended | Why::Lost, _))
        {
            if let Some(ms) = self.media_session {
                let id = self.id();
                self.send(&transport, NS_MEDIA, json!({"type": "STOP", "mediaSessionId": ms, "requestId": id}));
            }
            let id = self.id();
            self.send(RECEIVER, NS_RECEIVER, json!({"type": "STOP", "sessionId": sid, "requestId": id}));
            let t = Instant::now();
            let deadline = t + STOP_WAIT;
            let mut media_idle = self.media_session.is_none();
            let mut gone = false;
            while !gone && Instant::now() < deadline {
                let Ok(In::Net(_, Incoming::Msg(m))) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) else { continue };
                let v = m.json().unwrap_or(Value::Null);
                match v["type"].as_str() {
                    Some("MEDIA_STATUS") => {
                        let idle = v["status"].as_array().is_some_and(|a| a.is_empty() || a.iter().any(|x| x["playerState"].as_str() == Some("IDLE")));
                        if idle && !media_idle {
                            media_idle = true;
                            // The TV is quiet: this computer's output comes back now, not once
                            // the receiver has gone too (the step back is where the listener was).
                            self.release_outputs();
                            log(format!("the TV stopped the media after {} ms", t.elapsed().as_millis()));
                        }
                    }
                    Some("RECEIVER_STATUS") => {
                        let apps = v["status"]["applications"].as_array().cloned().unwrap_or_default();
                        gone = !apps.iter().any(|a| a["sessionId"].as_str() == Some(sid.as_str()));
                    }
                    _ => {}
                }
            }
            if gone {
                log(format!("the TV confirmed the stop after {} ms", t.elapsed().as_millis()));
            } else {
                log(format!("no stop confirmation from the TV within {} ms", STOP_WAIT.as_millis()));
            }
        }
        if !matches!(end, End::Failed(Why::Lost, _)) {
            self.send(RECEIVER, NS_CONNECTION, json!({"type": "CLOSE"}));
        } else {
            self.drop_channel();
        }
        self.release_outputs();
        self.server = None;
        let r = self.inner.report.lock().unwrap().clone();
        let summary = r.lines(&State::Off).into_iter().skip(1).collect::<Vec<_>>().join("; ");
        match &end {
            End::User => log(format!("session with “{name}” ended ({summary})")),
            End::Failed(why, detail) => log(format!("session with “{name}” failed: {why:?} {detail} ({summary})")),
        }
        self.inner.emit(Event::State(State::Off));
        if let End::Failed(why, detail) = end {
            self.inner.emit(Event::Failed { name, why, detail });
        }
        // Dropping the channel joins its thread after the goodbye is written.
    }
}
