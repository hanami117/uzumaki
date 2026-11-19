//! One Cast session: connect, launch the Default Media Receiver, serve and `LOAD` the stream, then
//! follow the device's statuses until the session ends. See `lib.rs` for the states.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::channel::{Channel, Incoming};
use crate::discover::CastDevice;
use crate::proto::{DEFAULT_MEDIA_RECEIVER, Message, NS_CONNECTION, NS_HEARTBEAT, NS_MEDIA, NS_RECEIVER, RECEIVER, SENDER};
use crate::health::Health;
use crate::stream::{Allow, Fetch, Opts, Server, local_addr_for};
use crate::{Cmd, Event, Inner, Meta, State, Why, log, pcm};

pub(crate) enum In {
    Net(Incoming),
    Cmd(Cmd),
}

const PING_EVERY: Duration = Duration::from_secs(5);
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
    ch: Channel,
    phase: Phase,
    next_id: u64,
    /// Our receiver app: its session and transport ids.
    app: Option<(String, String)>,
    media_session: Option<i64>,
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
    let t = tx.clone();
    let ch = match Channel::open_tls(addr, inner.cfg.connect_timeout, move |m| {
        let _ = t.send(In::Net(m));
    }) {
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
        ch,
        phase: Phase::Connecting,
        next_id: 1,
        app: None,
        media_session: None,
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
    s.send(RECEIVER, NS_CONNECTION, json!({"type": "CONNECT", "origin": {}, "userAgent": "Uzumaki", "senderInfo": {"sdkType": 2, "version": "15.605.1.3", "platform": 6, "systemVersion": "", "connectionType": 1}}));
    let id = s.id();
    s.send(RECEIVER, NS_RECEIVER, json!({"type": "GET_STATUS", "requestId": id}));
    let end = s.run(rx);
    s.finish(end);
}

impl Session<'_> {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn send(&self, dest: &str, ns: &str, payload: Value) {
        self.ch.send(&Message::new(SENDER, dest, ns, &payload));
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
                Ok(In::Net(Incoming::Msg(m))) => {
                    self.last_heard = Instant::now();
                    if let Some(end) = self.on_message(&m) {
                        return end;
                    }
                }
                Ok(In::Net(Incoming::Closed(why))) => return End::Failed(Why::Lost, why),
                Ok(In::Cmd(c)) => {
                    if let Some(end) = self.on_cmd(c) {
                        return end;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return End::User,
            }
            let now = Instant::now();
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
                if self.server.is_some() && self.app.is_some() {
                    log("reload: new URL, new LOAD");
                    self.reloading = true;
                    self.load(true);
                }
            }
        }
        None
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
            (NS_MEDIA, "MEDIA_STATUS") => return self.on_media_status(&v["status"]),
            (NS_MEDIA, "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST" | "INVALID_PLAYER_STATE") => {
                if kind == "LOAD_CANCELLED" && self.reloading {
                    return None;
                }
                log(format!("{kind}: {}", m.payload));
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
                    self.load(false);
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
        if self.server.is_some() {
            return Ok(());
        }
        let cfg = &self.inner.cfg;
        let bind = local_addr_for(self.device.ip).unwrap_or(self.device.ip);
        // Served over IPv6, the device may fetch from a temporary address of its own: its /64 too.
        let mut allow = Allow::device(&self.device.addrs, bind.is_ipv6());
        if self.device.group {
            // A group's leader may let a member fetch: every Cast device seen may.
            allow.add_others(&self.inner.known_ips());
        }
        match Server::start(bind, cfg.port, Opts { format: cfg.format, http: cfg.http, preroll_ms: cfg.preroll_ms }, self.inner.hub.clone(), allow.clone()) {
            Ok(s) => {
                log(format!("serving to {}", allow.describe()));
                let mut r = self.inner.report.lock().unwrap();
                r.url_host = Some(s.addr().to_string());
                r.format = Some(cfg.format);
                r.setup = format!("setup: {}, HTTP {}, pre-roll {} ms, streamType {}, {}", cfg.format.ext(), cfg.http.name(), cfg.preroll_ms, cfg.stream_type, if bind.is_ipv6() { "IPv6" } else { "IPv4" });
                drop(r);
                self.server = Some(s);
            }
            Err(e) => return Err(format!("can't start the stream server on {bind}:{}: {e}", cfg.port)),
        }
        if cfg.app_inputs {
            self.inputs = Some(pcm::Inputs::start(&cfg.socket(), self.inner.feed.clone(), self.inner.hub.clone()));
            self.inner.feed.set_casting(true);
            self.inner.emit(Event::Capture(true));
        }
        Ok(())
    }

    fn load(&mut self, fresh: bool) {
        let (Some(server), Some((sid, transport))) = (&self.server, self.app.clone()) else { return };
        let url = if fresh { server.rotate() } else { server.url() };
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
        let shown_url = url.rsplit_once("/cast/").map_or(url.clone(), |(h, _)| format!("{h}/cast/<token>/stream.{}", cfg.format.ext()));
        log(format!("LOAD {shown_url} ({}, {})", cfg.format.content_type(), cfg.stream_type));
        self.send(
            &transport,
            NS_MEDIA,
            json!({
                "type": "LOAD", "requestId": id, "sessionId": sid, "autoplay": true, "currentTime": 0,
                "media": {"contentId": url, "contentUrl": url, "streamType": cfg.stream_type, "contentType": cfg.format.content_type(), "metadata": metadata},
            }),
        );
        self.load_at = Some(Instant::now());
        self.health = Health::new(Instant::now());
        self.fetch = None;
        self.fetch_at = None;
        self.played = false;
        self.buffers.clear();
        self.media_session = None;
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

    fn on_media_status(&mut self, st: &Value) -> Option<End> {
        let s = st.as_array().and_then(|a| a.first())?;
        if let Some(ms) = s["mediaSessionId"].as_i64() {
            self.media_session = Some(ms);
        }
        let state = s["playerState"].as_str().unwrap_or("");
        let device_time = s["currentTime"].as_f64().unwrap_or(0.0);
        let now = Instant::now();
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
                self.set_phase(Phase::Paused);
            }
            "IDLE" => {
                let why = s["idleReason"].as_str().unwrap_or("");
                log(format!("IDLE {why} (served {served:.2} s, device at {device_time:.2} s)"));
                match why {
                    "INTERRUPTED" | "CANCELLED" if self.reloading => {}
                    "ERROR" => return Some(End::Failed(Why::LoadFailed, "the receiver reported an error".into())),
                    "FINISHED" => return Some(End::Failed(Why::Lost, "the receiver thinks the stream ended".into())),
                    "INTERRUPTED" | "CANCELLED" => return Some(End::Failed(Why::Taken, format!("IDLE {why}"))),
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    fn finish(mut self, end: End) {
        let name = self.name();
        // Our receiver goes away with the session (the TV goes back to its idle screen).
        if let Some((sid, _)) = self.app.clone()
            && !matches!(end, End::Failed(Why::Taken | Why::Ended | Why::Lost, _))
        {
            let id = self.id();
            self.send(RECEIVER, NS_RECEIVER, json!({"type": "STOP", "sessionId": sid, "requestId": id}));
        }
        if !matches!(end, End::Failed(Why::Lost, _)) {
            self.send(RECEIVER, NS_CONNECTION, json!({"type": "CLOSE"}));
        }
        if self.inputs.is_some() {
            self.inner.feed.set_casting(false);
            self.inputs = None;
            self.inner.emit(Event::Capture(false));
        }
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
