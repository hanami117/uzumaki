//! Chromecast: plays what the app plays on a Cast device (a Chromecast, a Google TV, a Nest
//! speaker, a Cast Group) through Google's Default Media Receiver, which needs no registration.
//!
//! The audio is the players' own output, already decoded, as for AirPlay: the bridge's capture of
//! MusicKit's `<audio>` (on this session's Unix socket) or the local file player's samples. The
//! device fetches it from a small HTTP server in this process (see `stream` for who may), as an
//! endless WAV or FLAC. Nothing is written to disk. No keys, no CDM, no encrypted stream touched.
//!
//! CASTV2 is spoken directly (`proto`, `channel`): TLS to the device, `CONNECT`, `LAUNCH` the
//! Default Media Receiver, `LOAD` our URL, then `MEDIA_STATUS` every 2 s, whose `currentTime`
//! against the audio served gives the device's buffer (the latency, logged as `cast: buffer`).
//!
//! A [`Controller`] is the whole API; it does nothing until asked (no thread, no network).
//!
//! Environment (spike switches; see `Config::from_env`): `UZUMAKI_CAST_FORMAT=wav|flac`,
//! `UZUMAKI_CAST_PORT` (fixed HTTP port), `UZUMAKI_CAST_STREAM_TYPE=LIVE|BUFFERED`,
//! `UZUMAKI_CAST_HOST` (+ `UZUMAKI_CAST_DEVICE_PORT`, `UZUMAKI_CAST_TARGET`): one made-up device
//! instead of mDNS, `UZUMAKI_CAST_LOCAL=1`: keep this computer's output on, `UZUMAKI_CAST_IP=4|6`:
//! talk to the device over that family only (default: IPv4 when it has one on our subnet).

pub mod channel;
pub mod discover;
pub mod flac;
pub mod health;
pub mod pcm;
pub mod proto;
mod session;
pub mod stream;
pub mod wav;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

pub use discover::{CastDevice, Family};
pub use pcm::LocalFeed;
pub use stream::{Format, Http};

fn log(msg: impl AsRef<str>) {
    eprintln!("cast: {}", msg.as_ref());
}

/// What the Android TV receiver (CrKey 1.56) wants buffered before it plays a progressive
/// stream, in bytes (live tests 3-4: ~1.36 MB without a length, less with a Content-Length).
/// The automatic pre-roll sends this much at once, so it starts at once
/// (`UZUMAKI_CAST_BURST_BYTES`, tool `--burst`).
pub const DEFAULT_BURST_BYTES: u64 = 1_400_000;

#[derive(Clone, Debug)]
pub struct Config {
    /// Where the session's PCM socket lives (a private folder).
    pub dir: PathBuf,
    /// The HTTP server's port (0: any free one).
    pub port: u16,
    pub format: Format,
    /// Audio sent at once at each fetch (see `stream::Opts`): the device starts with that much
    /// buffered and plays that much behind (`UZUMAKI_CAST_PREROLL_MS`). None: `burst_bytes` worth
    /// of the stream's format.
    pub preroll_ms: Option<u64>,
    pub burst_bytes: u64,
    /// What a WAV stream carries (`UZUMAKI_CAST_RATE`, `UZUMAKI_CAST_BITS`; default 96k/24).
    pub wav: wav::Spec,
    /// The body's framing (`UZUMAKI_CAST_HTTP=close|chunked|length`, default length: best in
    /// live test 4).
    pub http: Http,
    /// IPv4 / IPv6 / automatic (`UZUMAKI_CAST_IP=4|6`).
    pub family: Family,
    /// `LIVE` (default) or `BUFFERED`, as told to the receiver in `LOAD`.
    pub stream_type: String,
    /// A device to use instead of mDNS (tests).
    pub fake: Option<CastDevice>,
    /// The app's inputs (bridge socket, local player, silence) run with the session. Off for the
    /// command line tool, which publishes to `Controller::hub` itself.
    pub app_inputs: bool,
    pub keep_local: bool,
    pub connect_timeout: Duration,
    /// After `LAUNCH`, the receiver must be up within this.
    pub launch_timeout: Duration,
    /// After `LOAD`, the device must fetch the URL within this (else: a firewall, usually).
    pub fetch_timeout: Duration,
    /// Nothing heard from the device for this long (it answers our PINGs): the connection is lost.
    pub silence_timeout: Duration,
    pub discover_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let fake = var("UZUMAKI_CAST_HOST").and_then(|h| h.parse::<IpAddr>().ok()).map(|ip| {
            let port = var("UZUMAKI_CAST_DEVICE_PORT").and_then(|p| p.parse().ok()).unwrap_or(8009);
            CastDevice::at(ip, port, &var("UZUMAKI_CAST_TARGET").unwrap_or_else(|| "Test Cast".into()))
        });
        Config {
            dir: std::env::temp_dir().join(format!("uzumaki-cast-{}", std::process::id())),
            port: var("UZUMAKI_CAST_PORT").and_then(|p| p.parse().ok()).unwrap_or(0),
            format: var("UZUMAKI_CAST_FORMAT").and_then(|f| Format::parse(&f)).unwrap_or(Format::Wav),
            preroll_ms: var("UZUMAKI_CAST_PREROLL_MS").and_then(|v| v.parse::<u64>().ok()).map(|v| v.min(10_000)),
            burst_bytes: var("UZUMAKI_CAST_BURST_BYTES").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_BURST_BYTES),
            wav: wav::Spec::new(
                var("UZUMAKI_CAST_RATE").and_then(|v| wav::Spec::parse_rate(&v)).unwrap_or(wav::Spec::default().rate),
                var("UZUMAKI_CAST_BITS").and_then(|v| v.parse().ok()).unwrap_or(wav::Spec::default().bits),
            )
            .unwrap_or_default(),
            http: var("UZUMAKI_CAST_HTTP").and_then(|v| Http::parse(&v)).unwrap_or(Http::Length),
            family: var("UZUMAKI_CAST_IP").and_then(|f| Family::parse(&f)).unwrap_or_default(),
            stream_type: var("UZUMAKI_CAST_STREAM_TYPE").map(|s| s.to_uppercase()).filter(|s| s == "LIVE" || s == "BUFFERED").unwrap_or_else(|| "LIVE".into()),
            fake,
            app_inputs: true,
            keep_local: var("UZUMAKI_CAST_LOCAL").is_some_and(|v| v == "1"),
            connect_timeout: Duration::from_secs(5),
            launch_timeout: Duration::from_secs(20),
            fetch_timeout: Duration::from_secs(15),
            silence_timeout: Duration::from_secs(15),
            discover_timeout: Duration::from_secs(6),
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("pcm.sock")
    }

    /// The pre-roll in ms: the one set, else `burst_bytes` of the stream (FLAC: as 44.1/16 WAV
    /// at 60%), at most 10 s.
    pub fn preroll(&self) -> u64 {
        self.preroll_ms.unwrap_or_else(|| {
            let bps = match self.format {
                Format::Wav => self.wav.bytes_per_sec(),
                Format::Flac => wav::Spec::CAPTURE.bytes_per_sec() * 6 / 10,
            };
            (self.burst_bytes * 1000 / bps.max(1)).min(10_000)
        })
    }
}

/// What the device shows (`MusicTrackMediaMetadata`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub image: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum State {
    #[default]
    Off,
    Connecting { name: String },
    Launching { name: String },
    /// `LOAD` sent; waiting for the device to fetch the stream.
    Loading { name: String },
    /// The device fetches and fills its buffer.
    Buffering { name: String },
    /// `latency_ms`: the device's buffer (median of the last statuses), once measured.
    Playing { name: String, latency_ms: Option<u32> },
    Paused { name: String },
}

impl State {
    pub fn name(&self) -> Option<&str> {
        match self {
            State::Off => None,
            State::Connecting { name } | State::Launching { name } | State::Loading { name } | State::Buffering { name } | State::Playing { name, .. } | State::Paused { name } => Some(name),
        }
    }

    pub fn label(&self) -> String {
        match self {
            State::Off => "off".into(),
            State::Connecting { .. } => "connecting".into(),
            State::Launching { .. } => "starting the receiver".into(),
            State::Loading { .. } => "waiting for the device to fetch the stream".into(),
            State::Buffering { .. } => "buffering".into(),
            State::Playing { latency_ms: Some(ms), .. } => format!("playing, {ms} ms behind"),
            State::Playing { .. } => "playing".into(),
            State::Paused { .. } => "paused".into(),
        }
    }
}

/// Why a session ended on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    NotFound,
    Unreachable,
    /// Another app took the device (someone cast to it).
    Taken,
    /// The receiver was stopped on the device (or by another sender).
    Ended,
    Lost,
    /// The device never fetched the stream.
    NoFetch,
    LoadFailed,
}

impl Why {
    pub fn message(self, name: &str) -> String {
        match self {
            Why::NotFound => format!("Couldn't find “{name}” on this network."),
            Why::Unreachable => format!("Couldn't connect to “{name}”. Check that it is on and on the same network."),
            Why::Taken => format!("“{name}” is playing something else now."),
            Why::Ended => format!("“{name}” stopped playing."),
            Why::Lost => format!("The connection to “{name}” was lost."),
            Why::NoFetch => format!("“{name}” couldn't fetch the music from this computer. A firewall may be blocking it."),
            Why::LoadFailed => format!("“{name}” couldn't play the stream."),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Devices(Vec<CastDevice>),
    State(State),
    /// The bridge's capture must run (true) or stop, with the socket to send it to.
    Capture(bool),
    Failed { name: String, why: Why, detail: String },
    /// The device's own volume (0..1) and mute.
    Volume { level: f64, muted: bool },
}

/// The last numbers of the session, for `uzu cast status` and the log.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub device: Option<String>,
    pub url_host: Option<String>,
    pub format: Option<Format>,
    pub fetched_by: Option<IpAddr>,
    pub load_to_fetch_ms: Option<u64>,
    pub fetch_to_playing_ms: Option<u64>,
    pub load_to_playing_ms: Option<u64>,
    pub served_s: f64,
    pub device_time_s: f64,
    pub buffer_ms: Vec<i64>,
    pub volume: Option<(f64, bool)>,
    pub player_state: String,
    pub loads: u32,
    pub underruns: u32,
    pub stalls: u32,
    /// LOAD until the device's position first moved (the audio is heard).
    pub load_to_audio_ms: Option<u64>,
    pub setup: String,
}

impl Report {
    pub fn lines(&self, state: &State) -> Vec<String> {
        let mut v = vec![format!("state: {}{}", state.label(), state.name().map(|n| format!(" on “{n}”")).unwrap_or_default())];
        if let Some(d) = &self.device {
            v.push(format!("device: {d}"));
        }
        if let (Some(h), Some(f)) = (&self.url_host, self.format) {
            v.push(format!("stream: {} from {h}, {} LOAD(s)", f.ext(), self.loads));
        }
        if let Some(ip) = self.fetched_by {
            v.push(format!("fetched by {ip}"));
        }
        let ms = |o: Option<u64>| o.map_or("-".into(), |m| format!("{m} ms"));
        v.push(format!("LOAD→fetch {}, fetch→PLAYING {}, LOAD→PLAYING {}, LOAD→audio heard {}", ms(self.load_to_fetch_ms), ms(self.fetch_to_playing_ms), ms(self.load_to_playing_ms), ms(self.load_to_audio_ms)));
        if !self.setup.is_empty() {
            v.push(self.setup.clone());
        }
        v.push(format!("underruns {}, stalls {}", self.underruns, self.stalls));
        if !self.buffer_ms.is_empty() {
            let last: Vec<String> = self.buffer_ms.iter().rev().take(5).rev().map(|b| b.to_string()).collect();
            v.push(format!("served {:.2} s, device at {:.2} s ({}); buffer ms, last: {}", self.served_s, self.device_time_s, self.player_state, last.join(" ")));
        }
        if let Some((level, muted)) = self.volume {
            v.push(format!("device volume {:.0}%{}", level * 100.0, if muted { " (muted)" } else { "" }));
        }
        v
    }
}

pub(crate) enum Cmd {
    Stop,
    Pause,
    Resume,
    Volume(f64),
    Reload(Option<Meta>),
}

struct Handle {
    tx: mpsc::Sender<session::In>,
}

pub(crate) struct Inner {
    pub(crate) cfg: Config,
    on_event: Box<dyn Fn(Event) + Send + Sync>,
    pub(crate) hub: Arc<pcm::Hub>,
    pub(crate) feed: Arc<pcm::Feed>,
    devices: Mutex<Vec<CastDevice>>,
    browser: Mutex<Option<discover::Browser>>,
    browse_wanted: AtomicBool,
    session: Mutex<Option<Handle>>,
    chain: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub(crate) report: Mutex<Report>,
    state: Mutex<State>,
    closed: AtomicBool,
}

impl Inner {
    pub(crate) fn emit(&self, e: Event) {
        if let Event::State(s) = &e {
            *self.state.lock().unwrap() = s.clone();
        }
        (self.on_event)(e);
    }

    /// Addresses of the Cast devices seen (a group's members may fetch for it).
    pub(crate) fn known_ips(&self) -> Vec<IpAddr> {
        self.devices.lock().unwrap().iter().flat_map(|d| d.addrs.iter().copied()).collect()
    }
}

/// See the module comment. Cheap to clone.
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Inner>,
}

impl Controller {
    pub fn new(cfg: Config, on_event: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let feed = Arc::new(pcm::Feed::default());
        feed.set_keep_local(cfg.keep_local);
        Controller {
            inner: Arc::new(Inner {
                cfg,
                on_event: Box::new(on_event),
                hub: Arc::new(pcm::Hub::default()),
                feed,
                devices: Mutex::default(),
                browser: Mutex::default(),
                browse_wanted: AtomicBool::new(false),
                session: Mutex::default(),
                chain: Mutex::default(),
                report: Mutex::default(),
                state: Mutex::default(),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// The audio's way out (the command line tool publishes its tone here).
    pub fn hub(&self) -> Arc<pcm::Hub> {
        self.inner.hub.clone()
    }

    pub fn local_feed(&self) -> LocalFeed {
        LocalFeed { feed: self.inner.feed.clone() }
    }

    /// The session's PCM socket, for the bridge's capture (exists only while a session runs).
    pub fn socket(&self) -> PathBuf {
        self.inner.cfg.socket()
    }

    pub fn state(&self) -> State {
        self.inner.state.lock().unwrap().clone()
    }

    pub fn status_lines(&self) -> Vec<String> {
        self.inner.report.lock().unwrap().lines(&self.state())
    }

    /// Starts or stops browsing (`Event::Devices` on changes).
    pub fn browse(&self, on: bool) {
        let inner = &self.inner;
        if inner.browse_wanted.swap(on, Ordering::SeqCst) == on {
            return;
        }
        if !on {
            inner.browser.lock().unwrap().take();
            return;
        }
        if let Some(d) = inner.cfg.fake.clone() {
            *inner.devices.lock().unwrap() = vec![d.clone()];
            return inner.emit(Event::Devices(vec![d]));
        }
        let weak = Arc::downgrade(inner);
        match discover::Browser::start(move |list| {
            if let Some(inner) = weak.upgrade() {
                *inner.devices.lock().unwrap() = list.clone();
                inner.emit(Event::Devices(list));
            }
        }) {
            Ok(b) => {
                log("looking for Cast devices (mDNS _googlecast._tcp)");
                *inner.browser.lock().unwrap() = Some(b);
            }
            Err(e) => log(format!("can't look for Cast devices: {e}")),
        }
    }

    pub fn devices(&self) -> Vec<CastDevice> {
        self.inner.devices.lock().unwrap().clone()
    }

    /// Browses for `timeout` (blocking) and returns what answered; also keeps them for `connect`.
    pub fn look(&self, timeout: Duration) -> Result<Vec<CastDevice>, String> {
        if let Some(d) = self.inner.cfg.fake.clone() {
            *self.inner.devices.lock().unwrap() = vec![d.clone()];
            return Ok(vec![d]);
        }
        let list = discover::discover(timeout, None)?;
        *self.inner.devices.lock().unwrap() = list.clone();
        Ok(list)
    }

    /// Plays on `device` (the session there is ends first).
    pub fn connect(&self, device: CastDevice, meta: Meta) {
        self.start(Some(device), String::new(), meta);
    }

    /// Plays on the device called `name` (looked for by mDNS when not known yet).
    pub fn connect_named(&self, name: &str, meta: Meta) {
        let known = discover::pick(&self.devices(), name).cloned().or_else(|| self.inner.cfg.fake.clone());
        self.start(known, name.trim().to_string(), meta);
    }

    fn start(&self, device: Option<CastDevice>, name: String, meta: Meta) {
        let inner = &self.inner;
        if inner.closed.load(Ordering::SeqCst) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        if let Some(old) = inner.session.lock().unwrap().replace(Handle { tx: tx.clone() }) {
            let _ = old.tx.send(session::In::Cmd(Cmd::Stop));
        }
        let mut chain = inner.chain.lock().unwrap();
        let prev = chain.take();
        let inner2 = inner.clone();
        let t = std::thread::Builder::new().name("cast-session".into()).spawn(move || {
            if let Some(p) = prev {
                let _ = p.join();
            }
            let device = match device {
                Some(d) => d,
                None => {
                    inner2.emit(Event::State(State::Connecting { name: name.clone() }));
                    match discover::discover(inner2.cfg.discover_timeout, Some(&name)).ok().and_then(|l| {
                        let found = discover::pick(&l, &name).cloned();
                        *inner2.devices.lock().unwrap() = l;
                        found
                    }) {
                        Some(d) => d,
                        None => {
                            inner2.emit(Event::State(State::Off));
                            return inner2.emit(Event::Failed { name: name.clone(), why: Why::NotFound, detail: String::new() });
                        }
                    }
                }
            };
            session::run(&inner2, device, meta, &rx, tx);
        });
        match t {
            Ok(t) => *chain = Some(t),
            Err(e) => log(format!("could not start the session thread: {e}")),
        }
    }

    fn send(&self, c: Cmd) -> bool {
        self.inner.session.lock().unwrap().as_ref().is_some_and(|h| h.tx.send(session::In::Cmd(c)).is_ok())
    }

    /// Instant pause: the receiver pauses at once (its buffer keeps the next seconds) and the
    /// stream sends nothing meanwhile (no silence fill), so `resume` plays on in step.
    pub fn pause(&self) -> bool {
        let sent = self.send(Cmd::Pause);
        if sent {
            self.inner.feed.set_hold(true);
        }
        sent
    }
    pub fn resume(&self) -> bool {
        self.inner.feed.set_hold(false);
        self.send(Cmd::Resume)
    }
    /// Paused by `pause` (the tool's signal waits too).
    pub fn holding(&self) -> bool {
        self.inner.feed.holding()
    }
    /// The device's volume, 0-100.
    pub fn set_volume(&self, v: u8) -> bool {
        self.send(Cmd::Volume(f64::from(v.min(100)) / 100.0))
    }
    /// A new URL and a new `LOAD` (what a seek would do): the device's buffer starts afresh.
    pub fn reload(&self, meta: Option<Meta>) -> bool {
        self.send(Cmd::Reload(meta))
    }

    /// A seek, a skip or a song the user picked: the recent audio is dropped and a new `LOAD`
    /// follows, so the device plays a short silence (its pre-roll) and then the new place, in
    /// step, instead of the seconds it still had from before.
    pub fn restart(&self, meta: Option<Meta>) -> bool {
        self.inner.hub.forget();
        self.reload(meta)
    }

    /// Drops the recent audio only (a skip whose song is not known yet; `restart` follows).
    pub fn forget(&self) {
        self.inner.hub.forget();
    }

    /// Back to this computer: stops the receiver and ends the session.
    pub fn stop(&self) {
        if let Some(h) = self.inner.session.lock().unwrap().take() {
            let _ = h.tx.send(session::In::Cmd(Cmd::Stop));
        }
    }

    pub fn capture_ready(&self, ready: bool, error: Option<&str>) {
        if self.inner.feed.bridge_ready.swap(ready, Ordering::SeqCst) != ready || error.is_some() {
            log(format!("bridge capture {}{}", if ready { "ready" } else { "not ready" }, error.map(|e| format!(": {e}")).unwrap_or_default()));
        }
    }

    pub fn set_bridge(&self, present: bool) {
        self.inner.feed.no_bridge.store(!present, Ordering::SeqCst);
    }

    /// Quit: stops the receiver (waits up to ~1.5 s for the goodbye) and the server.
    pub fn shutdown(&self) {
        let inner = &self.inner;
        if inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        inner.browser.lock().unwrap().take();
        inner.feed.set_casting(false);
        self.stop();
        let t = inner.chain.lock().unwrap().take();
        if let Some(t) = t {
            let deadline = std::time::Instant::now() + Duration::from_millis(1500);
            while !t.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
        let _ = std::fs::remove_dir_all(&inner.cfg.dir);
    }

    /// The session thread ended (tests).
    pub fn idle(&self) -> bool {
        self.inner.chain.lock().unwrap().as_ref().is_none_or(|t| t.is_finished())
    }
}

#[cfg(test)]
mod tests;
