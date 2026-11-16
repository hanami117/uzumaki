//! AirPlay: sends what the app plays to AirPlay speakers on the local network, one or several
//! at once (multi-room, in step).
//!
//! The audio is the players' own output, already decoded: the bridge taps MusicKit's `<audio>` with
//! Web Audio and writes s16le stereo 44.1 kHz PCM to a Unix socket this crate binds, and the local
//! file player hands its samples to a [`LocalFeed`]. A session runs `cliairplay` (Music
//! Assistant's AirPlay sender, a separate GPL-3.0 program) for the chosen speaker and pumps that
//! PCM into its stdin; several speakers get one helper each, fed the same bytes. Nothing is written to disk; the audio only passes through memory. No keys,
//! no CDM, no encrypted stream is touched.
//!
//! A [`Controller`] is the whole API. It does nothing until asked: no thread and no network use
//! while nobody browses and no speaker is connected.
//!
//! - [`Controller::browse`]: mDNS browsing on or off (the picker, while it is open). Changes of
//!   the device list arrive as [`Event::Devices`].
//! - [`Controller::connect`] / [`Controller::connect_named`]: this speaker alone; the speakers
//!   playing stop first. [`Controller::add`] / [`Controller::add_named`] / [`Controller::remove`]:
//!   one speaker more or less in the group. Order for each speaker: start the helper → `[STATUS] connected` →
//!   bind the PCM socket and ask for the bridge's capture ([`Event::Capture`]) → `[STATUS] audio
//!   buffered_ms` → `ACTION=START` on the command pipe → `[STATUS] started`. The speaker plays
//!   ([`Cast::On`]) once it started *and* the audio source is ready: the bridge is connected to
//!   the socket and said its capture taps MusicKit's player ([`Controller::capture_ready`]), or
//!   the local player is in charge. Until then the state stays [`Cast::Connecting`]; a source
//!   that isn't ready within `Config::capture_timeout` ends the session ([`Why::NoAudio`]).
//! - [`Controller::flush`]: on a seek or a song the user picked, the audio still on its way is
//!   dropped (`ACTION=FLUSH`, then a new START), so the change is heard at once.
//! - A speaker that ends on its own reports why ([`Event::Failed`]); the others play on. When
//!   none is left the audio is back on this computer: the local outputs are quiet only while
//!   [`LocalFeed::mute_local`] is true and while the bridge's socket is connected.
//!
//! Multi-room, the way Music Assistant drives cliairplay: every helper follows one clock (a
//! `cliairplay --ptp-daemon` this crate starts with the first speaker, helpers run with
//! `--ptp-shared`). The audio is counted in frames from the moment the speakers last started
//! together; every speaker gets the same bytes from the frame it was added at. Speakers that
//! start together get one start instant (`START_UNIX_MS`); one that joins a group that plays gets
//! the instant its first frame is due (`START_JOIN=1`; if its helper can only start later, it cuts
//! as much audio and stays in step). A flush stops every writer before `ACTION=FLUSH` (the helper
//! drains its input), waits for each `[STATUS] flushed` and starts them again together.
//!
//! One session serves both players: while a local file is loaded (`LocalFeed::set_active(true)`)
//! the bridge's bytes are dropped and the local ones go to the helper. When nobody sends audio
//! (paused, between songs, no player yet), silence keeps the speaker's stream alive.
//!
//! Environment, for diagnostics and tests only:
//! - `UZUMAKI_CLIAIRPLAY`: the helper binary (else see `helper::find`).
//! - `UZUMAKI_AIRPLAY_DEBUG`: cliairplay `--debug` level; `UZUMAKI_AIRPLAY_EXTRA_ARGS`: more helper arguments.
//! - `UZUMAKI_AIRPLAY_LOCAL=1`: keep this computer's output on while casting (A/B checks).
//! - `UZUMAKI_AIRPLAY_PTP_DAEMON=0`: no shared clock (each helper its own, as before multi-room).
//! - `UZUMAKI_AIRPLAY_HOST` (+ `UZUMAKI_AIRPLAY_PORT`, default 7000; `UZUMAKI_AIRPLAY_TARGET`, the
//!   name, default `Test Speaker`): one made-up device at that address instead of mDNS, so tests
//!   use no network.

pub mod discover;
pub mod helper;

use discover::{Browser, Device, Kind, Output, Service};
use helper::Status;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// s16le stereo: the helper's input must stay aligned to whole frames.
const FRAME_BYTES: u64 = 4;
const BYTES_PER_SECOND: u64 = 44100 * FRAME_BYTES;
/// After a flush, audio that was already on its way (the worklet's chunk, the socket) is the
/// old position's: dropped for this long.
const FLUSH_DROP: Duration = Duration::from_millis(180);
/// The wall clock ran this far ahead of the process clock: the computer slept.
const SLEEP_JUMP: Duration = Duration::from_secs(8);

fn log(msg: impl AsRef<str>) {
    eprintln!("airplay: {}", msg.as_ref());
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, Debug)]
pub struct Config {
    pub helper: Option<PathBuf>,
    /// Where the socket and the command pipe live (a private folder).
    pub dir: PathBuf,
    /// Makes the DACP id, which is the identity of a stored pairing: keep it across runs.
    pub seed: u64,
    pub debug: Option<u8>,
    pub extra: Vec<String>,
    /// Keep this computer's output on while casting (`UZUMAKI_AIRPLAY_LOCAL=1`).
    pub keep_local: bool,
    /// How long the helper may take to connect.
    pub connect_timeout: Duration,
    /// The pauses before trying again when the speaker refuses the session at once (it plays for
    /// another device, which it lets go of a moment later): one more try after each.
    pub retry_backoff: Vec<Duration>,
    /// After a seek, the audio arriving for this long is the old position's and is dropped.
    pub flush_drop: Duration,
    /// How long `connect_named` looks for the speaker.
    pub discover_timeout: Duration,
    /// A speaker that doesn't answer our clock plays silence: after this long the session ends.
    pub stall_grace: Duration,
    /// Connected, but the audio source (the bridge's capture, or the local player) isn't ready
    /// for this long: the session ends (`Why::NoAudio`) instead of claiming to play.
    pub capture_timeout: Duration,
    /// Devices to show instead of browsing (tests: no network).
    pub fake: Vec<Device>,
    /// Run the shared PTP clock (`cliairplay --ptp-daemon`) and have every helper follow it, so
    /// several speakers play in step. Off: each helper runs its own clock (one speaker only).
    pub ptp_daemon: bool,
}

impl Config {
    /// The app's configuration: a private folder under the temporary directory, the helper where
    /// `helper::find` sees it, and the diagnostic switches from the environment.
    pub fn from_env(seed: u64) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let fake = var("UZUMAKI_AIRPLAY_HOST")
            .and_then(|h| h.trim().parse().ok())
            .map(|ip: std::net::IpAddr| {
                let port = var("UZUMAKI_AIRPLAY_PORT").and_then(|p| p.trim().parse().ok()).unwrap_or(7000);
                let name = var("UZUMAKI_AIRPLAY_TARGET").unwrap_or_else(|| "Test Speaker".into());
                let txt = vec![("features".to_string(), "0x200,0x0".to_string()), ("model".to_string(), "AudioAccessory5,1".to_string())];
                Device { name: name.clone(), ip, hostname: ip.to_string(), airplay: Some(Service { instance: name, port, txt }), raop: None }
            })
            .into_iter()
            .collect();
        Config {
            helper: helper::find(var("UZUMAKI_CLIAIRPLAY").map(PathBuf::from)),
            dir: std::env::temp_dir().join(format!("uzumaki-airplay-{}", std::process::id())),
            seed,
            debug: var("UZUMAKI_AIRPLAY_DEBUG").and_then(|v| v.trim().parse().ok()),
            extra: var("UZUMAKI_AIRPLAY_EXTRA_ARGS").map(|v| v.split_whitespace().map(String::from).collect()).unwrap_or_default(),
            keep_local: var("UZUMAKI_AIRPLAY_LOCAL").is_some_and(|v| v.trim() == "1"),
            connect_timeout: Duration::from_secs(15),
            retry_backoff: vec![Duration::from_millis(1500), Duration::from_millis(2500)],
            flush_drop: FLUSH_DROP,
            discover_timeout: Duration::from_secs(8),
            stall_grace: Duration::from_secs(6),
            capture_timeout: Duration::from_secs(8),
            fake,
            ptp_daemon: var("UZUMAKI_AIRPLAY_PTP_DAEMON").is_none_or(|v| v.trim() != "0"),
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("pcm.sock")
    }
}

/// Where the audio goes, as a whole (`Event::Speakers` has every speaker).
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Cast {
    /// This computer.
    #[default]
    Off,
    Connecting { id: String, name: String },
    /// The speaker plays. `latency_ms`: how long after the player a sound is heard (for a later
    /// lyrics and progress offset); None until the first start is confirmed.
    On { id: String, name: String, latency_ms: Option<u32> },
}

impl Cast {
    /// The device's id and name, connecting or playing.
    pub fn device(&self) -> Option<(&str, &str)> {
        match self {
            Cast::Off => None,
            Cast::Connecting { id, name } | Cast::On { id, name, .. } => Some((id, name)),
        }
    }
}

/// Why a session ended on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// `cliairplay` isn't installed with the app.
    NoHelper,
    /// No speaker of that name (or id) on the network.
    NotFound,
    /// Off, asleep, another network, or a firewall.
    Unreachable,
    /// It plays for another device and kept refusing us (after the retries).
    Busy,
    /// The speaker only lets people of its home play (no password, no PIN to type).
    Refused,
    /// The password was wrong.
    Password,
    /// The PIN was wrong or the pairing was cancelled on the device.
    Pairing,
    /// It was playing and the connection went away.
    Lost,
    /// The computer slept.
    Slept,
    /// The speaker never answered our clock (a firewall, usually).
    Clock,
    /// The player's audio could not be captured for the speaker (the bridge's tap never got ready).
    NoAudio,
}

impl Why {
    /// One sentence for the user.
    pub fn message(self, name: &str) -> String {
        match self {
            Why::NoHelper => "AirPlay isn't available: the cliairplay helper is missing from this install.".into(),
            Why::NotFound => format!("Couldn't find “{name}” on this network."),
            Why::Unreachable => format!("Couldn't connect to “{name}”. Check that it is on and on the same network."),
            Why::Busy => format!("“{name}” is busy with another device."),
            Why::Refused => format!("“{name}” only accepts people of its home. In the Home app, allow speaker access for “Anyone on the Same Network”."),
            Why::Password => format!("Wrong password for “{name}”."),
            Why::Pairing => format!("Couldn't pair with “{name}”. Check the code and try again."),
            Why::Lost => format!("The connection to “{name}” was lost."),
            Why::Slept => format!("AirPlay to “{name}” stopped while the computer slept."),
            Why::Clock => format!("“{name}” didn't answer. A firewall may be blocking AirPlay (UDP ports 319 and 320)."),
            Why::NoAudio => format!("Couldn't send the music to “{name}”: the player's audio could not be captured."),
        }
    }
}

/// What the speaker asks of the user before it plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Password,
    /// The four digits the device shows now.
    Pin,
}

/// One speaker of the group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Speaker {
    pub id: String,
    pub name: String,
    /// It plays what the user hears (else it is connecting).
    pub playing: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The picker's rows changed.
    Devices(Vec<Output>),
    /// The group as a whole: `Off` with no speaker, `On` (the first speaker that plays) while any
    /// plays, else `Connecting` (the first speaker).
    State(Cast),
    /// Every speaker of the group, in the order they were added (empty: this computer).
    Speakers(Vec<Speaker>),
    /// The bridge's capture must run (a speaker is connected) or stop.
    Capture(bool),
    /// A speaker stopped on its own. `still`: the speakers that go on playing; empty when the
    /// audio is back on this computer.
    Failed { name: String, why: Why, still: Vec<String> },
    /// The speaker wants a password or a PIN; `wrong`: the one just given was refused.
    /// Answer with `Controller::answer` or `Controller::disconnect`.
    Auth { id: String, name: String, need: Need, wrong: bool },
    /// A PIN pairing worked: keep `credentials` for this device (`Connect::auth`).
    Paired { id: String, credentials: String },
    /// The stored pairing of this device no longer works: forget it.
    Forget { id: String },
}

/// What `connect` needs besides the device.
#[derive(Clone, Debug, Default)]
pub struct Connect {
    /// The speaker's volume, 0-100.
    pub volume: u8,
    pub password: Option<String>,
    /// A stored pairing (`Event::Paired`).
    pub auth: Option<String>,
}

/// At most this much audio waits for one speaker's helper; past it the speaker has stopped
/// reading and is started again in step with the others.
const QUEUE_CAP: usize = 3 * BYTES_PER_SECOND as usize;
/// A helper's input is written in pieces this big, so a flush never waits for a long write.
const PIECE: usize = 4096;

/// One speaker's way in: its writer thread's queue.
struct Out {
    attempt: u64,
    tx: mpsc::Sender<(u64, Arc<[u8]>)>,
    /// The writer's generation when this was added: older chunks are dropped by the writer.
    wgen: u64,
    queued: Arc<AtomicUsize>,
    overflow: Arc<AtomicBool>,
}

/// The audio's way to every speaker that takes it. Each speaker gets the same bytes from the
/// moment it is added; `frames` counts the frames written since the group last started together
/// (a speaker added at frame `n` plays that frame at `anchor + n / 44.1` ms).
#[derive(Default)]
struct Fanout {
    /// A flush is under way: what arrives is the old position's.
    hold: bool,
    frames: u64,
    outs: Vec<Out>,
}

#[derive(Default)]
struct Shared {
    fan: Mutex<Fanout>,
    /// The helpers (and the clock daemon) running, for `shutdown` and the pid file.
    pids: Mutex<Vec<u32>>,
    /// A helper is connected to a speaker and takes audio (the local outputs go quiet).
    casting: AtomicBool,
    /// The local player has a song loaded: its audio (or silence) goes to the speaker, the bridge's doesn't.
    local_active: AtomicBool,
    /// The bridge said its capture taps the player (`Controller::capture_ready`). Reset for each session.
    bridge_ready: AtomicBool,
    /// The bridge is connected to the PCM socket.
    bridge_linked: AtomicBool,
    /// The app runs no bridge (local only mode): there is nothing to capture (`Controller::set_bridge`).
    no_bridge: AtomicBool,
    keep_local: bool,
    /// The local player's way in, while a session runs.
    local_tx: Mutex<Option<mpsc::SyncSender<Vec<i16>>>>,
    /// Milliseconds since `epoch`: the last write to the helpers, and until when incoming audio
    /// is dropped after a flush.
    last_write: AtomicU64,
    drop_until: AtomicU64,
    /// Wall-clock ms of the first audio written since the group started together (0: none yet).
    first_audio: AtomicU64,
    epoch: Option<Instant>,
}

impl Shared {
    fn now_ms(&self) -> u64 {
        self.epoch.map_or(0, |e| e.elapsed().as_millis() as u64)
    }

    fn dropping(&self) -> bool {
        self.now_ms() < self.drop_until.load(Ordering::Relaxed)
    }

    /// What the speaker gets is what the user hears: the local player is in charge, or the
    /// bridge is connected and its capture taps MusicKit's player.
    fn source_ready(&self) -> bool {
        self.local_active.load(Ordering::SeqCst) || self.no_bridge.load(Ordering::SeqCst) || (self.bridge_ready.load(Ordering::SeqCst) && self.bridge_linked.load(Ordering::SeqCst))
    }

    /// Whole frames to every speaker that takes audio now. Never waits for a speaker: each has
    /// its own writer. Dropped while a flush is under way or nobody takes audio.
    fn write(&self, bytes: &[u8]) {
        let mut fan = self.fan.lock().unwrap();
        if fan.hold || fan.outs.is_empty() || bytes.is_empty() {
            return;
        }
        let data: Arc<[u8]> = Arc::from(bytes);
        let len = bytes.len();
        fan.outs.retain(|o| {
            if o.queued.load(Ordering::SeqCst) + len > QUEUE_CAP {
                o.overflow.store(true, Ordering::SeqCst);
                return false;
            }
            o.queued.fetch_add(len, Ordering::SeqCst);
            o.tx.send((o.wgen, data.clone())).is_ok()
        });
        fan.frames += len as u64 / FRAME_BYTES;
        drop(fan);
        self.last_write.store(self.now_ms(), Ordering::Relaxed);
        let _ = self.first_audio.compare_exchange(0, unix_ms(), Ordering::Relaxed, Ordering::Relaxed);
    }

    fn save_pids(&self, cfg: &Config) {
        let pids = self.pids.lock().unwrap();
        let path = pid_file(cfg);
        if pids.is_empty() {
            let _ = std::fs::remove_file(path);
        } else {
            let _ = std::fs::write(path, pids.iter().map(|p| format!("{p}\n")).collect::<String>());
        }
    }
}

/// Size of the local player's queue: 4096-frame chunks (~93 ms each), as the bridge's.
const LOCAL_QUEUE: usize = 32;
/// Nobody has sent audio for this long (paused, between songs, loading): silence.
const SILENCE_GRACE: Duration = Duration::from_millis(250);
/// How far ahead of the clock the silence is written.
const SILENCE_LEAD: Duration = Duration::from_millis(100);

/// What the app's local-file player uses to send its audio to the speaker. Cheap to clone.
#[derive(Clone)]
pub struct LocalFeed {
    shared: Arc<Shared>,
}

impl LocalFeed {
    /// True while a speaker takes audio: only then is `push` worth calling.
    pub fn casting(&self) -> bool {
        self.shared.casting.load(Ordering::Relaxed)
    }

    /// True while this computer's output must stay silent (casting, unless `UZUMAKI_AIRPLAY_LOCAL=1`).
    pub fn mute_local(&self) -> bool {
        self.casting() && !self.shared.keep_local
    }

    /// Interleaved s16 stereo 44.1 kHz frames. Never waits for the speaker (called on the audio
    /// thread): when the queue is full the chunk is dropped.
    pub fn push(&self, chunk: Vec<i16>) {
        if let Some(tx) = self.shared.local_tx.lock().unwrap().as_ref() {
            let _ = tx.try_send(chunk);
        }
    }

    /// The local player has a song loaded (true) or handed over to the bridge (false).
    pub fn set_active(&self, on: bool) {
        if self.shared.local_active.swap(on, Ordering::SeqCst) != on && self.casting() {
            log(if on { "local player in charge: the speaker gets the local file" } else { "bridge in charge: the speaker gets Apple Music" });
        }
    }
}

enum Target {
    Known(Box<(Output, Device)>),
    Named(String),
}

enum Cmd {
    Stop,
    Add(Target, Connect),
    Remove(String),
    /// One speaker's volume (`Some(id)`), or every speaker's.
    Volume(Option<String>, u8),
    Flush,
}

enum Ev {
    Line(u64, Status),
    CmdReady(u64, std::fs::File),
    Closed(u64),
    Found(u64, Option<Box<(Output, Device)>>),
    Cmd(Cmd),
}

struct GroupHandle {
    tx: mpsc::Sender<Ev>,
    done: Arc<AtomicBool>,
    /// The group thread is ending: a new speaker needs a new group.
    ending: Arc<AtomicBool>,
}

struct Pairing {
    id: String,
    name: String,
    stdin: Option<ChildStdin>,
    child: Arc<Mutex<Option<Child>>>,
    /// What to connect with once the pairing worked.
    then: Box<(Output, Device)>,
}

/// A password asked for (`Event::Auth` with `Need::Password`): the device to try again.
struct Asked {
    target: Box<(Output, Device)>,
}

struct Inner {
    cfg: Config,
    on_event: Box<dyn Fn(Event) + Send + Sync>,
    shared: Arc<Shared>,
    devices: Mutex<Vec<(Output, Device)>>,
    browser: Mutex<Option<Browser>>,
    browse_wanted: AtomicBool,
    group: Mutex<Option<GroupHandle>>,
    /// The last group's thread: the next one waits for it, so two never share the socket.
    chain: Mutex<Option<std::thread::JoinHandle<()>>>,
    pairing: Mutex<Option<Pairing>>,
    asked: Mutex<Option<Asked>>,
    volume: AtomicU8,
    /// The group's speakers, as last reported (`Event::Speakers`).
    speakers: Mutex<Vec<Speaker>>,
    /// Ids for speakers and helper runs.
    next: AtomicU64,
    /// `shutdown` ran: nothing starts any more.
    closed: AtomicBool,
}

/// See the module comment. Cheap to clone.
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Inner>,
}

impl Controller {
    pub fn new(cfg: Config, on_event: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared { keep_local: cfg.keep_local, epoch: Some(Instant::now()), ..Shared::default() });
        Controller {
            inner: Arc::new(Inner {
                cfg,
                on_event: Box::new(on_event),
                shared,
                devices: Mutex::default(),
                browser: Mutex::default(),
                browse_wanted: AtomicBool::new(false),
                group: Mutex::default(),
                chain: Mutex::default(),
                pairing: Mutex::default(),
                asked: Mutex::default(),
                volume: AtomicU8::new(30),
                speakers: Mutex::default(),
                next: AtomicU64::new(1),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// The PCM socket's path, for the bridge (`UZUMAKI_AIRPLAY_PCM_SOCKET`). It exists only while
    /// a speaker is connected.
    pub fn socket(&self) -> PathBuf {
        self.inner.cfg.socket()
    }

    /// Whether the helper program is there (the picker says so when it isn't).
    pub fn available(&self) -> bool {
        self.inner.cfg.helper.is_some()
    }

    pub fn local_feed(&self) -> LocalFeed {
        LocalFeed { shared: self.inner.shared.clone() }
    }

    /// Starts or stops browsing for speakers. Starting clears the list; it fills as they answer.
    pub fn browse(&self, on: bool) {
        let inner = &self.inner;
        if inner.browse_wanted.swap(on, Ordering::SeqCst) == on {
            return;
        }
        if !on {
            if inner.browser.lock().unwrap().take().is_some() {
                log("stopped looking for speakers");
            }
            return;
        }
        if !inner.cfg.fake.is_empty() {
            inner.set_devices(inner.cfg.fake.clone(), &[]);
            return;
        }
        inner.set_devices(Vec::new(), &[]);
        // Off the caller's thread: opening the mDNS sockets may take a moment.
        let inner = inner.clone();
        std::thread::spawn(move || {
            let own = discover::own_addresses();
            let weak = Arc::downgrade(&inner);
            let browser = Browser::start(move |devices| {
                if let Some(inner) = weak.upgrade()
                    && inner.browse_wanted.load(Ordering::SeqCst)
                {
                    inner.set_devices(devices, &own);
                }
            });
            match browser {
                Ok(b) => {
                    log("looking for speakers (mDNS _airplay._tcp, _raop._tcp)");
                    let mut slot = inner.browser.lock().unwrap();
                    // Closed again meanwhile: dropping it stops it.
                    if inner.browse_wanted.load(Ordering::SeqCst) {
                        *slot = Some(b);
                    }
                }
                Err(e) => log(format!("can't look for speakers: {e}")),
            }
        });
    }

    /// The rows last reported.
    pub fn devices(&self) -> Vec<Output> {
        self.inner.devices.lock().unwrap().iter().map(|(o, _)| o.clone()).collect()
    }

    fn known(&self, id: &str) -> Option<(Output, Device)> {
        self.inner.devices.lock().unwrap().iter().find(|(o, _)| o.id == id).cloned()
    }

    fn by_name(&self, name: &str) -> Target {
        let known = self.inner.devices.lock().unwrap().iter().find(|(o, _)| o.name.eq_ignore_ascii_case(name.trim())).cloned();
        match known {
            Some(t) => Target::Known(Box::new(t)),
            None => Target::Named(name.trim().to_string()),
        }
    }

    /// Sends the audio to the device with this id (from `Event::Devices`) and to no other. False
    /// when the id isn't known (any more).
    pub fn connect(&self, id: &str, opts: Connect) -> bool {
        match self.known(id) {
            Some(t) => {
                self.inner.start(Target::Known(Box::new(t)), opts, true);
                true
            }
            None => false,
        }
    }

    /// Looks for the speaker called `name` (as in the picker, any case) and sends the audio there,
    /// and to no other.
    pub fn connect_named(&self, name: &str, opts: Connect) {
        self.inner.start(self.by_name(name), opts, true);
    }

    /// Adds the device with this id to the speakers that play (multi-room: they play in step).
    /// With none playing, the same as `connect`. False when the id isn't known (any more).
    pub fn add(&self, id: &str, opts: Connect) -> bool {
        match self.known(id) {
            Some(t) => {
                self.inner.start(Target::Known(Box::new(t)), opts, false);
                true
            }
            None => false,
        }
    }

    /// `add` by name, as `connect_named`.
    pub fn add_named(&self, name: &str, opts: Connect) {
        self.inner.start(self.by_name(name), opts, false);
    }

    /// Takes one speaker out of the group; the others play on. The last one out: back to this computer.
    pub fn remove(&self, id: &str) {
        if let Some(g) = self.inner.group.lock().unwrap().as_ref() {
            let _ = g.tx.send(Ev::Cmd(Cmd::Remove(id.to_string())));
        }
    }

    /// Back to this computer: ends every speaker, a pairing in progress or a question.
    pub fn disconnect(&self) {
        self.inner.cancel_pairing();
        *self.inner.asked.lock().unwrap() = None;
        if let Some(g) = self.inner.group.lock().unwrap().take() {
            let _ = g.tx.send(Ev::Cmd(Cmd::Stop));
        }
    }

    /// The answer to `Event::Auth`: the password, or the PIN the device shows.
    pub fn answer(&self, text: &str) {
        let text = text.trim();
        let asked = self.inner.asked.lock().unwrap().take();
        if let Some(a) = asked {
            let opts = Connect { volume: self.volume(), password: Some(text.to_string()), auth: None };
            return self.inner.start(Target::Known(a.target), opts, false);
        }
        if let Some(p) = self.inner.pairing.lock().unwrap().as_mut()
            && let Some(mut stdin) = p.stdin.take()
        {
            log(format!("pairing with \"{}\": sending the code", p.name));
            let _ = writeln!(stdin, "{text}");
            // Dropped here: the helper reads one line.
        }
    }

    /// Every speaker's own volume, 0-100. Kept for the next session too.
    pub fn set_volume(&self, volume: u8) {
        let v = volume.min(100);
        self.inner.volume.store(v, Ordering::Relaxed);
        if let Some(g) = self.inner.group.lock().unwrap().as_ref() {
            let _ = g.tx.send(Ev::Cmd(Cmd::Volume(None, v)));
        }
    }

    /// One speaker's own volume, 0-100 (the others keep theirs).
    pub fn set_speaker_volume(&self, id: &str, volume: u8) {
        if let Some(g) = self.inner.group.lock().unwrap().as_ref() {
            let _ = g.tx.send(Ev::Cmd(Cmd::Volume(Some(id.to_string()), volume.min(100))));
        }
    }

    pub fn volume(&self) -> u8 {
        self.inner.volume.load(Ordering::Relaxed)
    }

    /// The group's speakers, as last reported (`Event::Speakers`).
    pub fn speakers(&self) -> Vec<Speaker> {
        self.inner.speakers.lock().unwrap().clone()
    }

    /// The bridge's capture taps the player (true), or doesn't (any more): the page is loading, an
    /// `<audio>` couldn't be tapped, the audio context is suspended (`error` says which). An app
    /// with no bridge calls `set_bridge(false)` instead. The state shows `Cast::On` only while this
    /// holds (or the local player is in charge).
    pub fn capture_ready(&self, ready: bool, error: Option<&str>) {
        if self.inner.shared.bridge_ready.swap(ready, Ordering::SeqCst) != ready || error.is_some() {
            log(format!("bridge capture {}{}", if ready { "ready" } else { "not ready" }, error.map(|e| format!(": {e}")).unwrap_or_default()));
        }
    }

    /// Whether the app runs a bridge (default: it does). Without one (local only mode) there is
    /// nothing to capture, and the speaker plays the local player's audio or silence.
    pub fn set_bridge(&self, present: bool) {
        self.inner.shared.no_bridge.store(!present, Ordering::SeqCst);
    }

    /// The user seeked or picked another song: drop the audio still on its way to the speakers.
    /// Nothing happens while no speaker plays.
    pub fn flush(&self) {
        if !self.inner.shared.casting.load(Ordering::Relaxed) {
            return;
        }
        if let Some(g) = self.inner.group.lock().unwrap().as_ref() {
            let _ = g.tx.send(Ev::Cmd(Cmd::Flush));
        }
    }

    /// Ends everything (quit): tells the speakers to stop, ends the helpers, removes the socket.
    /// Waits up to about two seconds for the helpers to say goodbye to the speakers.
    pub fn shutdown(&self) {
        let inner = &self.inner;
        if inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.browse(false);
        inner.cancel_pairing();
        inner.shared.casting.store(false, Ordering::SeqCst); // the local outputs play again at once
        let group = inner.group.lock().unwrap().take();
        if let Some(g) = group {
            let _ = g.tx.send(Ev::Cmd(Cmd::Stop));
            let deadline = Instant::now() + Duration::from_millis(2200);
            while !g.done.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        // Whatever is left (a group that didn't end in time): no helper may outlive us.
        for pid in std::mem::take(&mut *inner.shared.pids.lock().unwrap()) {
            // SAFETY: a plain signal to a child of ours.
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        let _ = std::fs::remove_file(inner.cfg.socket());
        let _ = std::fs::remove_file(pid_file(&inner.cfg));
        let _ = std::fs::remove_dir_all(&inner.cfg.dir);
    }
}

impl Inner {
    fn emit(&self, e: Event) {
        (self.on_event)(e);
    }

    fn set_devices(&self, devices: Vec<Device>, own: &[std::net::IpAddr]) {
        let rows = discover::outputs(&devices, own);
        let list: Vec<Output> = rows.iter().map(|(o, _)| o.clone()).collect();
        let mut slot = self.devices.lock().unwrap();
        let same = slot.iter().map(|(o, _)| o).eq(list.iter());
        *slot = rows;
        drop(slot);
        if !same {
            self.emit(Event::Devices(list));
        }
    }

    fn cancel_pairing(&self) {
        if let Some(p) = self.pairing.lock().unwrap().take() {
            log(format!("pairing with \"{}\" cancelled", p.name));
            if let Some(mut c) = p.child.lock().unwrap().take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    /// Sends the audio to `target`: alone (`exclusive`: the group there is ends first), or along
    /// with the speakers that already play.
    fn start(self: &Arc<Self>, target: Target, opts: Connect, exclusive: bool) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        if exclusive {
            self.cancel_pairing();
            *self.asked.lock().unwrap() = None;
            self.volume.store(opts.volume.min(100), Ordering::Relaxed);
        }
        let mut slot = self.group.lock().unwrap();
        if !exclusive
            && let Some(g) = slot.as_ref()
            && !g.ending.load(Ordering::SeqCst)
        {
            let _ = g.tx.send(Ev::Cmd(Cmd::Add(target, opts)));
            return;
        }
        let (tx, rx) = mpsc::channel::<Ev>();
        let _ = tx.send(Ev::Cmd(Cmd::Add(target, opts)));
        let done = Arc::new(AtomicBool::new(false));
        let ending = Arc::new(AtomicBool::new(false));
        let old = slot.replace(GroupHandle { tx: tx.clone(), done: done.clone(), ending: ending.clone() });
        drop(slot);
        if let Some(old) = old {
            let _ = old.tx.send(Ev::Cmd(Cmd::Stop));
        }
        let mut chain = self.chain.lock().unwrap();
        let prev = chain.take();
        let inner = self.clone();
        let spawned = std::thread::Builder::new().name("airplay".into()).spawn(move || {
            if let Some(p) = prev {
                let _ = p.join();
            }
            Group::new(&inner, tx, ending).run(&rx);
            done.store(true, Ordering::SeqCst);
        });
        match spawned {
            Ok(t) => *chain = Some(t),
            Err(e) => log(format!("could not start the session thread: {e}")),
        }
    }

    /// HomeKit pairing: the device shows a PIN, `answer` sends it, the credentials come back as
    /// `Event::Paired` and the speaker joins with them.
    fn pair(self: &Arc<Self>, target: Box<(Output, Device)>) {
        let (out, device) = (&target.0, &target.1);
        let Some(helper_path) = self.cfg.helper.clone() else { return };
        let dacp = helper::Options::seeded(self.cfg.seed, 0).dacp;
        let args = helper::pair_args(device, &dacp);
        log(format!("pairing with \"{}\": {} {}", out.name, helper_path.display(), args.join(" ")));
        let mut child = match Command::new(&helper_path).args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                log(format!("could not start cliairplay: {e}"));
                return self.emit(Event::Failed { name: out.name.clone(), why: Why::Pairing, still: Vec::new() });
            }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(Some(child)));
        let (id, name) = (out.id.clone(), out.name.clone());
        *self.pairing.lock().unwrap() = Some(Pairing { id: id.clone(), name: name.clone(), stdin, child: child.clone(), then: target });
        // The prompt and the log lines.
        let inner = self.clone();
        let (pid, pname) = (id.clone(), name.clone());
        std::thread::spawn(move || {
            let Some(stderr) = stderr else { return };
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("cliairplay: {line}");
                if line.contains("Enter the PIN") {
                    inner.emit(Event::Auth { id: pid.clone(), name: pname.clone(), need: Need::Pin, wrong: false });
                }
            }
        });
        // The result.
        let inner = self.clone();
        std::thread::spawn(move || {
            let mut credentials = None;
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(c) = helper::parse_credentials(&line) {
                        credentials = Some(c.to_string());
                    }
                }
            }
            if let Some(mut c) = child.lock().unwrap().take() {
                let _ = c.wait();
            }
            // Cancelled meanwhile (the slot is empty or another device's): say nothing.
            let mine = {
                let mut slot = inner.pairing.lock().unwrap();
                if slot.as_ref().is_some_and(|p| p.id == id) { slot.take() } else { None }
            };
            let Some(p) = mine else { return };
            match credentials {
                Some(c) => {
                    log(format!("paired with \"{name}\""));
                    inner.emit(Event::Paired { id: id.clone(), credentials: c.clone() });
                    let opts = Connect { volume: inner.volume.load(Ordering::Relaxed), password: None, auth: Some(c) };
                    inner.start(Target::Known(p.then), opts, false);
                }
                None => {
                    log(format!("pairing with \"{name}\" failed"));
                    let still = inner.group_names();
                    inner.emit(Event::Failed { name, why: Why::Pairing, still });
                }
            }
        });
    }

    /// The names of the group's speakers (for a pairing's failure message).
    fn group_names(&self) -> Vec<String> {
        self.speakers.lock().unwrap().iter().map(|s| s.name.clone()).collect()
    }
}

/// How a speaker's helper ended.
enum End {
    /// The user (or a newer session) stopped it.
    User,
    Failed(Why),
    /// The speaker refused the session before it was set up (`connect_failed`): busy with another
    /// sender (tried again after a pause), or really out of reach.
    ConnectFailed { http: u64 },
    Auth { need: Need, wrong: bool },
    /// The stored pairing was refused.
    StalePairing,
}

/// A speaker's clock, as its helper reports it (`[STATUS] clock_ready`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clock {
    /// No word yet.
    Unknown,
    /// Measuring (cold, probing).
    Waiting,
    /// Locked, or NTP timing (nothing to wait for).
    Ready,
}

/// A joining speaker waits for its clock this long at most before it is fed.
const CLOCK_WAIT: Duration = Duration::from_secs(5);
/// A helper that never reports its clock (an old one, or the test stand-in) is taken as ready after this.
const CLOCK_SILENT: Duration = Duration::from_millis(500);
/// The speakers starting together wait this long at most for each other's first audio.
const GATHER: Duration = Duration::from_millis(1000);
/// A flush waits this long at most for every speaker's ack.
const FLUSH_WAIT: Duration = Duration::from_millis(1500);
/// The lead of a common start: every speaker's helper must be able to make it.
const GROUP_LEAD_MS: u64 = 400;
/// Starts this far apart are out of step: the group starts again together.
const ALIGN_SLACK_MS: u64 = 20;

/// The instant frame `first` is heard when frame 0 is heard at `anchor`.
fn instant_of(anchor: u64, first: u64) -> u64 {
    anchor + (first * 1000 + 22050) / 44100
}

/// One speaker of the group and its helper.
struct Member {
    key: u64,
    id: String,
    name: String,
    target: Option<Box<(Output, Device)>>,
    opts: Connect,
    /// The helper running now (0: none, waiting to try again or for the name to be found).
    attempt: u64,
    tries: usize,
    retry_at: Option<Instant>,
    child: Option<Child>,
    fifo: PathBuf,
    cmd: Option<std::fs::File>,
    /// Commands written once the pipe is open.
    pending: String,
    /// The writer of this helper's input.
    tx: Option<mpsc::Sender<(u64, Arc<[u8]>)>>,
    wgen: Arc<AtomicU64>,
    busy: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
    overflow: Arc<AtomicBool>,
    /// The command pipe opener runs while this is true.
    alive: Arc<AtomicBool>,
    began: Instant,
    connected: Option<Instant>,
    clock: Clock,
    stalled: Option<Instant>,
    /// It takes the group's audio, from frame `first` on.
    feeding: bool,
    first: u64,
    buffered: bool,
    /// The START sent (requested instant; 0: as soon as possible) and whether it joins a group
    /// that already plays.
    start_sent: Option<(u64, bool)>,
    started: bool,
    start_tries: u32,
    flushing: bool,
    head_ms: u64,
    error: Option<String>,
    error_http: u64,
    failed: Option<Why>,
    stopping: bool,
    kill_at: Option<Instant>,
    volume: u8,
}

impl Member {
    fn new(key: u64, id: String, name: String, target: Option<Box<(Output, Device)>>, opts: Connect) -> Self {
        Member {
            key,
            id,
            name,
            target,
            volume: opts.volume.min(100),
            opts,
            attempt: 0,
            tries: 0,
            retry_at: None,
            child: None,
            fifo: PathBuf::new(),
            cmd: None,
            pending: String::new(),
            tx: None,
            wgen: Arc::default(),
            busy: Arc::default(),
            queued: Arc::default(),
            overflow: Arc::default(),
            alive: Arc::default(),
            began: Instant::now(),
            connected: None,
            clock: Clock::Unknown,
            stalled: None,
            feeding: false,
            first: 0,
            buffered: false,
            start_sent: None,
            started: false,
            start_tries: 0,
            flushing: false,
            head_ms: 0,
            error: None,
            error_http: 0,
            failed: None,
            stopping: false,
            kill_at: None,
        }
    }

    /// Writes to the command pipe now, or once it opens.
    fn command(&mut self, lines: &str) -> bool {
        match self.cmd.as_mut() {
            Some(f) => f.write_all(lines.as_bytes()).and_then(|_| f.flush()).is_ok(),
            None if self.attempt != 0 => {
                self.pending.push_str(lines);
                true
            }
            None => false,
        }
    }

    fn kill(&mut self, why: Option<Why>) {
        if self.failed.is_none() {
            self.failed = why;
        }
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
        }
    }

    /// Its clock is ready to join a group that plays (or there is nothing to wait for).
    fn clock_ok(&self) -> bool {
        let since = self.connected.map_or(Duration::ZERO, |t| t.elapsed());
        self.clock == Clock::Ready || (self.clock == Clock::Unknown && since > CLOCK_SILENT) || since > CLOCK_WAIT
    }

    /// Stops taking audio: the writer drops what it still has, and nothing more is queued.
    fn unfeed(&mut self, fan: &mut Fanout) {
        self.wgen.fetch_add(1, Ordering::SeqCst);
        fan.outs.retain(|o| o.attempt != self.attempt);
        self.feeding = false;
    }

    /// Waits (briefly) for the writer to finish the piece it is writing, so a FLUSH that follows
    /// finds nothing of the old audio on its way.
    fn writer_idle(&self) {
        let until = Instant::now() + Duration::from_millis(300);
        while self.busy.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Takes `rx`'s chunks into the helper's input until the queue closes or the helper stops
/// reading. Chunks of an older generation (before a flush) are dropped.
fn writer(mut stdin: ChildStdin, rx: mpsc::Receiver<(u64, Arc<[u8]>)>, wgen: Arc<AtomicU64>, busy: Arc<AtomicBool>, queued: Arc<AtomicUsize>) {
    for (g, data) in rx {
        busy.store(true, Ordering::SeqCst);
        let mut ok = true;
        for piece in data.chunks(PIECE) {
            if wgen.load(Ordering::SeqCst) != g {
                break;
            }
            if stdin.write_all(piece).is_err() {
                ok = false;
                break;
            }
        }
        queued.fetch_sub(data.len(), Ordering::SeqCst);
        busy.store(false, Ordering::SeqCst);
        if !ok {
            break;
        }
    }
}

/// The speakers that play together: one helper each, the same audio to all, one clock (the PTP
/// daemon), the same start instant. Lives on its own thread while it has speakers.
struct Group<'a> {
    inner: &'a Arc<Inner>,
    tx: mpsc::Sender<Ev>,
    ending: Arc<AtomicBool>,
    members: Vec<Member>,
    /// The PCM socket and the local player's pump run (from the first connection on).
    pump: Option<Pump>,
    source_alive: Arc<AtomicBool>,
    daemon: Option<Child>,
    prepared: bool,
    /// The instant frame 0 of the fan-out is heard, once a start is confirmed.
    anchor: Option<u64>,
    /// The common start requested (several speakers starting together), to check the acks.
    epoch_start: Option<u64>,
    /// Waiting for the speakers starting together to have audio, since.
    gather: Option<Instant>,
    /// A flush under way, since.
    flushing: Option<Instant>,
    realigns: u32,
    latency: Option<u32>,
    /// Connected, but the audio source isn't ready since then.
    waiting: Option<Instant>,
    /// The whole group failed (slept, no audio): one message for all.
    failure: Option<(Why, String)>,
    shown: (Cast, Vec<Speaker>),
    clocks: (SystemTime, Instant),
}

impl<'a> Group<'a> {
    fn new(inner: &'a Arc<Inner>, tx: mpsc::Sender<Ev>, ending: Arc<AtomicBool>) -> Self {
        Group {
            inner,
            tx,
            ending,
            members: Vec::new(),
            pump: None,
            source_alive: Arc::new(AtomicBool::new(false)),
            daemon: None,
            prepared: false,
            anchor: None,
            epoch_start: None,
            gather: None,
            flushing: None,
            realigns: 0,
            latency: None,
            waiting: None,
            failure: None,
            shown: (Cast::Off, Vec::new()),
            clocks: (SystemTime::now(), Instant::now()),
        }
    }

    fn cfg(&self) -> &Config {
        &self.inner.cfg
    }

    fn shared(&self) -> &Arc<Shared> {
        &self.inner.shared
    }

    fn run(mut self, rx: &mpsc::Receiver<Ev>) {
        loop {
            if self.members.is_empty() {
                // Nobody left: end, unless a speaker was added meanwhile (decided under the lock
                // `Inner::start` takes, so no addition is lost).
                let mut slot = self.inner.group.lock().unwrap();
                let pending: Vec<Ev> = rx.try_iter().collect();
                if pending.is_empty() {
                    self.ending.store(true, Ordering::SeqCst);
                    if slot.as_ref().is_some_and(|g| Arc::ptr_eq(&g.ending, &self.ending)) {
                        slot.take();
                    }
                    break;
                }
                drop(slot);
                for ev in pending {
                    self.handle(ev);
                }
                self.tick();
                continue;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(ev) => self.handle(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }
        self.close();
    }

    fn by_attempt(&mut self, attempt: u64) -> Option<usize> {
        self.members.iter().position(|m| m.attempt == attempt && attempt != 0)
    }

    fn active(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| !m.stopping && m.failed.is_none())
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Cmd(Cmd::Stop) => {
                for i in 0..self.members.len() {
                    self.stop_member(i);
                }
            }
            Ev::Cmd(Cmd::Add(target, opts)) => self.add(target, opts),
            Ev::Cmd(Cmd::Remove(id)) => {
                if let Some(i) = self.members.iter().position(|m| m.id == id && !m.stopping) {
                    self.stop_member(i);
                }
            }
            Ev::Cmd(Cmd::Volume(id, v)) => {
                for m in self.members.iter_mut().filter(|m| id.as_ref().is_none_or(|id| *id == m.id)) {
                    m.volume = v;
                    if m.connected.is_some() && m.command(&format!("VOLUME={v}\n")) {
                        log(format!("\"{}\": volume {v}", m.name));
                    }
                }
            }
            Ev::Cmd(Cmd::Flush) => self.flush(false),
            Ev::Found(key, found) => {
                let Some(i) = self.members.iter().position(|m| m.key == key) else { return };
                if self.members[i].stopping {
                    return self.remove_at(i);
                }
                match found.and_then(|d| discover::outputs(std::slice::from_ref(&d.1), &[]).pop()) {
                    Some(t) => {
                        log(format!("found {}", discover::describe(&t.1)));
                        // Already in the group under its id (added twice by name): once is enough.
                        if self.members.iter().any(|m| m.key != key && m.id == t.0.id && !m.stopping) {
                            return self.remove_at(i);
                        }
                        let m = &mut self.members[i];
                        m.id = t.0.id.clone();
                        m.name = t.0.name.clone();
                        m.target = Some(Box::new(t));
                        self.spawn(i);
                    }
                    None => {
                        self.members[i].failed = Some(Why::NotFound);
                        self.finish(i, None);
                    }
                }
            }
            Ev::CmdReady(att, f) => {
                let Some(i) = self.by_attempt(att) else { return };
                let m = &mut self.members[i];
                m.cmd = Some(f);
                let pending = std::mem::take(&mut m.pending);
                // A volume chosen while connecting.
                let initial = m.opts.volume.min(100);
                let mut lines = String::new();
                if m.volume != initial {
                    lines.push_str(&format!("VOLUME={}\n", m.volume));
                }
                lines.push_str(&pending);
                if !lines.is_empty() && !m.command(&lines) {
                    log(format!("\"{}\": could not write to the command pipe", m.name));
                }
            }
            Ev::Line(att, status) => {
                let Some(i) = self.by_attempt(att) else { return };
                self.line(i, status);
            }
            Ev::Closed(att) => {
                let Some(i) = self.by_attempt(att) else { return };
                self.closed(i);
            }
        }
    }

    fn add(&mut self, target: Target, opts: Connect) {
        let key = self.inner.next.fetch_add(1, Ordering::SeqCst);
        match target {
            Target::Known(t) => {
                if self.members.iter().any(|m| m.id == t.0.id && !m.stopping) {
                    return; // already there (or on its way)
                }
                let (id, name) = (t.0.id.clone(), t.0.name.clone());
                self.members.push(Member::new(key, id, name, Some(t), opts));
                let i = self.members.len() - 1;
                self.spawn(i);
            }
            Target::Named(n) => {
                if self.members.iter().any(|m| m.name.eq_ignore_ascii_case(&n) && !m.stopping) {
                    return;
                }
                self.members.push(Member::new(key, n.to_lowercase(), n.clone(), None, opts));
                let fake = self.cfg().fake.clone();
                let (tx, timeout) = (self.tx.clone(), self.cfg().discover_timeout);
                let found = move || -> Option<Box<(Output, Device)>> {
                    let device = if fake.is_empty() {
                        log(format!("looking for \"{n}\" on the network (mDNS, up to {} s)", timeout.as_secs()));
                        match discover::discover(timeout, Some(&n)) {
                            Ok(devices) => discover::outputs(&devices, &discover::own_addresses()).into_iter().map(|(_, d)| d).find(|d| d.name.eq_ignore_ascii_case(&n)),
                            Err(e) => {
                                log(format!("discovery failed: {e}"));
                                None
                            }
                        }
                    } else {
                        fake.iter().find(|d| d.name.eq_ignore_ascii_case(&n)).cloned()
                    };
                    device.and_then(|d| discover::outputs(std::slice::from_ref(&d), &[]).pop()).map(Box::new)
                };
                if self.cfg().fake.is_empty() {
                    std::thread::spawn(move || {
                        let _ = tx.send(Ev::Found(key, found()));
                    });
                } else {
                    let _ = self.tx.send(Ev::Found(key, found()));
                }
            }
        }
    }

    /// The first helper of the group: the private folder, a helper left by a killed run, the
    /// shared clock.
    fn prepare(&mut self) -> bool {
        if self.prepared {
            return true;
        }
        let cfg = self.cfg().clone();
        if let Err(e) = std::fs::create_dir_all(&cfg.dir) {
            log(format!("{}: {e}", cfg.dir.display()));
            return false;
        }
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&cfg.dir, std::fs::Permissions::from_mode(0o700));
        }
        end_stale_helpers(&cfg);
        if cfg.ptp_daemon
            && let Some(helper_path) = cfg.helper.as_ref()
        {
            self.daemon = start_daemon(helper_path, &cfg, self.shared());
        }
        self.prepared = true;
        true
    }

    /// Starts member `i`'s helper (a first try, or another one).
    fn spawn(&mut self, i: usize) {
        let Some(helper_path) = self.cfg().helper.clone() else {
            self.members[i].failed = Some(Why::NoHelper);
            return self.finish(i, None);
        };
        if !self.prepare() {
            self.members[i].failed = Some(Why::Unreachable);
            return self.finish(i, None);
        }
        let attempt = self.inner.next.fetch_add(1, Ordering::SeqCst);
        let cfg = self.cfg().clone();
        let shared = self.shared().clone();
        let fifo = cfg.dir.join(format!("cmd-{attempt}.fifo"));
        let _ = std::fs::remove_file(&fifo);
        if let Err(e) = mkfifo(&fifo) {
            log(format!("command pipe {}: {e}", fifo.display()));
            self.members[i].failed = Some(Why::Unreachable);
            return self.finish(i, None);
        }
        let m = &mut self.members[i];
        let Some(target) = m.target.clone() else { return };
        let (out, device) = (&target.0, &target.1);
        let options = helper::Options {
            debug: cfg.debug,
            extra: cfg.extra.clone(),
            password: m.opts.password.clone(),
            auth: m.opts.auth.clone(),
            ptp_shared: cfg.ptp_daemon,
            ..helper::Options::seeded(cfg.seed, m.opts.volume.min(100))
        };
        let args = helper::args(device, &fifo, &options);
        // The password and the pairing stay out of the log.
        let shown: Vec<String> = {
            let mut hide = false;
            args.iter()
                .map(|a| {
                    let v = if hide { "<hidden>".to_string() } else { a.clone() };
                    hide = a == "--password" || a == "--auth";
                    v
                })
                .collect()
        };
        log(format!("connecting to \"{}\": {} {}", out.name, helper_path.display(), shown.join(" ")));
        let mut child = match spawn_helper(&helper_path, &args) {
            Ok(c) => c,
            Err(e) => {
                log(format!("could not start cliairplay: {e}"));
                let _ = std::fs::remove_file(&fifo);
                m.failed = Some(Why::NoHelper);
                return self.finish(i, None);
            }
        };
        // A fresh run: everything of the last one is forgotten.
        let opts = m.opts.clone();
        let (key, id, name, tries, volume) = (m.key, m.id.clone(), m.name.clone(), m.tries, m.volume);
        *m = Member { attempt, tries, fifo, began: Instant::now(), volume, ..Member::new(key, id, name, Some(target), opts) };
        shared.pids.lock().unwrap().push(child.id());
        shared.save_pids(&cfg);
        if let Some(stdin) = child.stdin.take() {
            let (wtx, wrx) = mpsc::channel();
            let (g, b, q) = (m.wgen.clone(), m.busy.clone(), m.queued.clone());
            let _ = std::thread::Builder::new().name("airplay-in".into()).spawn(move || writer(stdin, wrx, g, b, q));
            m.tx = Some(wtx);
        }
        let readers = Arc::new(AtomicU64::new(2));
        let streams = [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)];
        let multi = self.members.len() > 1;
        for stream in streams.into_iter().flatten() {
            let tx = self.tx.clone();
            let readers = readers.clone();
            let tag = if multi { format!("cliairplay[{}]", self.members[i].name) } else { "cliairplay".to_string() };
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    eprintln!("{tag}: {line}");
                    if let Some(s) = helper::parse_status(&line) {
                        let _ = tx.send(Ev::Line(attempt, s));
                    }
                }
                if readers.fetch_sub(1, Ordering::SeqCst) == 1 {
                    let _ = tx.send(Ev::Closed(attempt));
                }
            });
        }
        self.members[i].child = Some(child);
    }

    /// The PCM socket and the local player's pump, at the first connection.
    fn start_source(&mut self) {
        if self.pump.is_some() || self.source_alive.load(Ordering::SeqCst) {
            return;
        }
        let shared = self.shared().clone();
        // This group's capture says when it's ready (an earlier group's word doesn't count).
        shared.bridge_ready.store(false, Ordering::SeqCst);
        shared.first_audio.store(0, Ordering::Relaxed);
        self.source_alive.store(true, Ordering::SeqCst);
        let (ltx, lrx) = mpsc::sync_channel::<Vec<i16>>(LOCAL_QUEUE);
        *shared.local_tx.lock().unwrap() = Some(ltx);
        let (s, a) = (shared.clone(), self.source_alive.clone());
        let _ = std::thread::Builder::new().name("airplay-local".into()).spawn(move || local_pump(lrx, s, a));
        let socket = self.cfg().socket();
        match Pump::start(&socket, shared.clone()) {
            Ok(p) => self.pump = Some(p),
            Err(e) => log(format!("PCM socket {}: {e}", socket.display())),
        }
        shared.casting.store(true, Ordering::SeqCst);
        self.inner.emit(Event::Capture(true));
    }

    fn line(&mut self, i: usize, status: Status) {
        let name = self.members[i].name.clone();
        match status {
            Status::Route(r) => log(format!("\"{name}\": route: {r}")),
            Status::Connected => {
                let m = &mut self.members[i];
                m.connected = Some(Instant::now());
                log(format!("connected to \"{name}\" in {} ms", m.began.elapsed().as_millis()));
                let (fifo, alive) = (m.fifo.clone(), m.alive.clone());
                alive.store(true, Ordering::SeqCst);
                if !m.stopping {
                    if self.pump.is_none() {
                        log("opening the PCM socket for the bridge");
                    }
                    self.start_source();
                }
                open_cmd_pipe(fifo, self.tx.clone(), alive, self.members[i].attempt);
            }
            Status::Buffered { ms } => {
                log(format!("\"{name}\": audio buffered ({ms} ms)"));
                self.members[i].buffered = true;
            }
            Status::Started { requested_ms, at_ms } => self.started(i, requested_ms, at_ms),
            Status::Flushed { head_ms } => {
                log(format!("\"{name}\": flushed"));
                let m = &mut self.members[i];
                m.flushing = false;
                m.head_ms = head_ms;
            }
            Status::Latency { warm_lead_ms } => log(format!("\"{name}\" keeps {warm_lead_ms} ms of audio queued")),
            Status::ClockReady { mode, state } => {
                log(format!("\"{name}\": clock {mode}: {state}"));
                let m = &mut self.members[i];
                m.stalled = (state == "stalled").then(Instant::now);
                m.clock = if state == "ready" || mode == "ntp" { Clock::Ready } else { Clock::Waiting };
            }
            Status::Error { code, http, detail } => {
                log(format!("\"{name}\": ERROR {code} (http {http}): {detail}"));
                log(format!("  → {}", helper::explain(&code)));
                let m = &mut self.members[i];
                match code.as_str() {
                    // The stream goes on as it was.
                    "flush_failed" => m.flushing = false,
                    "start_failed" => {
                        m.start_tries += 1;
                        m.start_sent = None;
                        if m.start_tries >= 3 {
                            m.kill(Some(Why::Lost));
                        }
                    }
                    "auth_required" | "auth_failed" | "connect_failed" => {
                        m.error = Some(code);
                        m.error_http = http;
                    }
                    _ => {}
                }
            }
            Status::Eof => log(format!("\"{name}\": helper input ended")),
            Status::IdleTimeout => log(format!("\"{name}\": helper idle timeout")),
            Status::Stopped => log(format!("\"{name}\": helper stopped")),
        }
    }

    fn started(&mut self, i: usize, requested_ms: u64, at_ms: u64) {
        let first_audio = self.shared().first_audio.load(Ordering::Relaxed);
        let m = &mut self.members[i];
        let joined = m.start_sent.is_some_and(|(_, j)| j);
        m.started = true;
        m.start_tries = 0;
        let anchor = at_ms.saturating_sub(instant_of(0, m.first));
        let name = m.name.clone();
        if joined {
            let late = at_ms.saturating_sub(requested_ms);
            log(format!(
                "\"{name}\" joins at unix ms {at_ms} ({} ms from now){}",
                at_ms as i64 - unix_ms() as i64,
                if late > ALIGN_SLACK_MS { format!("; {late} ms later than asked, its audio is cut by as much to stay in step") } else { String::new() }
            ));
            return;
        }
        if first_audio > 0 && anchor > first_audio && self.latency.is_none() {
            self.latency = Some((anchor - first_audio).min(60_000) as u32);
        }
        log(format!(
            "\"{name}\" plays from unix ms {at_ms} ({} ms from now; requested {requested_ms}); a sound is heard about {} ms after the player makes it",
            at_ms as i64 - unix_ms() as i64,
            self.latency.map_or("?".into(), |l| l.to_string())
        ));
        match self.anchor {
            None => self.anchor = Some(anchor),
            // Speakers started together that didn't get the instant asked for are out of step:
            // all start again together, further ahead.
            Some(a) if a.abs_diff(anchor) > ALIGN_SLACK_MS => {
                if self.realigns < 2 {
                    self.realigns += 1;
                    log(format!("\"{name}\" starts {} ms off the others: starting the group again together", a.abs_diff(anchor)));
                    self.flush(true);
                } else {
                    log(format!("\"{name}\" starts {} ms off the others (still, after {} tries)", a.abs_diff(anchor), self.realigns));
                }
            }
            Some(_) => {}
        }
    }

    /// The user seeked or picked another song, or the group must start again together: every
    /// speaker drops what it has and they start again from the same frame.
    fn flush(&mut self, realign: bool) {
        if !realign {
            self.realigns = 0;
        }
        let shared = self.shared().clone();
        // Audio already on its way is the old position's.
        shared.drop_until.store(shared.now_ms() + self.cfg().flush_drop.as_millis() as u64, Ordering::Relaxed);
        let mut fan = shared.fan.lock().unwrap();
        let mut any = false;
        for m in self.members.iter_mut().filter(|m| !m.stopping && (m.feeding || m.start_sent.is_some())) {
            m.unfeed(&mut fan);
            m.writer_idle();
            if m.command("ACTION=FLUSH\n") {
                m.flushing = true;
                any = true;
            }
            m.buffered = false;
            m.start_sent = None;
            m.head_ms = 0;
        }
        if !any {
            return;
        }
        log("flush (seek or new song): the speakers start again from the new audio");
        fan.hold = true;
        fan.frames = 0;
        drop(fan);
        shared.first_audio.store(0, Ordering::Relaxed);
        self.anchor = None;
        self.epoch_start = None;
        self.gather = None;
        self.flushing = Some(Instant::now());
    }

    /// Adds member `i` to the fan-out: it gets every frame from now on.
    fn feed(&mut self, i: usize) {
        let shared = self.shared().clone();
        let mut fan = shared.fan.lock().unwrap();
        let m = &mut self.members[i];
        let Some(tx) = m.tx.clone() else { return };
        m.overflow.store(false, Ordering::SeqCst);
        fan.outs.push(Out { attempt: m.attempt, tx, wgen: m.wgen.load(Ordering::SeqCst), queued: m.queued.clone(), overflow: m.overflow.clone() });
        m.feeding = true;
        m.first = fan.frames;
        m.buffered = false;
    }

    fn tick(&mut self) {
        let cfg = self.cfg().clone();
        let shared = self.shared().clone();
        // The clocks: a sleep shows as the wall clock running ahead of the process clock.
        let (w, mo) = (SystemTime::now(), Instant::now());
        let jump = w.duration_since(self.clocks.0).unwrap_or_default().saturating_sub(mo.duration_since(self.clocks.1));
        self.clocks = (w, mo);
        if jump > SLEEP_JUMP && self.active().next().is_some() {
            log(format!("the computer slept ({} s): ending the session", jump.as_secs()));
            self.fail_all(Why::Slept);
        }
        // The flush is over once every speaker took it.
        if let Some(t) = self.flushing {
            let acked = self.members.iter().all(|m| !m.flushing || m.stopping);
            if (acked || t.elapsed() > FLUSH_WAIT) && !shared.dropping() {
                if !acked {
                    log("a speaker didn't confirm the flush in time");
                }
                for m in &mut self.members {
                    m.flushing = false;
                }
                shared.fan.lock().unwrap().hold = false;
                self.flushing = None;
            }
        }
        let due: Vec<u64> = self.members.iter().filter(|m| m.retry_at.is_some_and(|t| Instant::now() >= t)).map(|m| m.key).collect();
        for key in due {
            if let Some(i) = self.members.iter().position(|m| m.key == key) {
                self.members[i].retry_at = None;
                self.spawn(i);
            }
        }
        for m in &mut self.members {
            if m.attempt == 0 || m.stopping || m.failed.is_some() {
                if m.kill_at.is_some_and(|t| Instant::now() >= t) {
                    m.kill_at = None;
                    m.kill(None);
                }
                continue;
            }
            if m.connected.is_none() && m.began.elapsed() > cfg.connect_timeout {
                log(format!("no answer from \"{}\" after {} s", m.name, cfg.connect_timeout.as_secs()));
                m.kill(Some(Why::Unreachable));
                continue;
            }
            if m.stalled.is_some_and(|t| t.elapsed() > cfg.stall_grace) {
                m.stalled = None;
                log(format!("\"{}\": {}", m.name, helper::STALLED_HINT));
                m.kill(Some(Why::Clock));
                continue;
            }
            if m.overflow.swap(false, Ordering::SeqCst) && m.feeding {
                log(format!("\"{}\" stopped taking audio: starting it again in step with the others", m.name));
                let mut fan = shared.fan.lock().unwrap();
                m.unfeed(&mut fan);
                drop(fan);
                m.writer_idle();
                if m.command("ACTION=FLUSH\n") {
                    m.flushing = true;
                }
                m.buffered = false;
                m.start_sent = None;
            }
        }
        if self.flushing.is_none() {
            self.feed_and_start();
        }
        self.source_check();
        self.show();
    }

    /// Who takes audio, and the START of each speaker that has its first audio.
    fn feed_and_start(&mut self) {
        let any_start = self.members.iter().any(|m| m.start_sent.is_some() && !m.stopping);
        for i in 0..self.members.len() {
            let m = &self.members[i];
            if m.connected.is_none() || m.feeding || m.flushing || m.stopping || m.failed.is_some() {
                continue;
            }
            // Before the group plays: everyone starts together. After: a newcomer joins once its
            // clock is ready (else its helper would have to cut more of its audio).
            if (self.anchor.is_none() && !any_start) || m.clock_ok() {
                self.feed(i);
            }
        }
        // Starts.
        let ready = |m: &Member| m.feeding && m.buffered && m.start_sent.is_none() && m.cmd.is_some() && !m.stopping && m.failed.is_none();
        if let Some(anchor) = self.anchor {
            for m in self.members.iter_mut().filter(|m| ready(m)) {
                let at = instant_of(anchor, m.first);
                log(format!("\"{}\" joins the group: its first frame ({}) at unix ms {at}", m.name, m.first));
                if m.command(&format!("START_JOIN=1\nSTART_UNIX_MS={at}\nACTION=START\n")) {
                    m.start_sent = Some((at, true));
                }
            }
            return;
        }
        if any_start {
            return; // the first start's confirmation is awaited: it sets the anchor
        }
        let waiting: Vec<usize> = (0..self.members.len()).filter(|&i| self.members[i].feeding && self.members[i].start_sent.is_none() && !self.members[i].stopping).collect();
        if waiting.is_empty() {
            self.gather = None;
            return;
        }
        let since = *self.gather.get_or_insert_with(Instant::now);
        let all = waiting.iter().all(|&i| ready(&self.members[i]));
        if !all && since.elapsed() < GATHER {
            return;
        }
        let go: Vec<usize> = waiting.into_iter().filter(|&i| ready(&self.members[i])).collect();
        if go.is_empty() {
            return;
        }
        self.gather = None;
        if go.len() == 1 {
            // One speaker: as soon as it can (its helper picks the instant, past its queued audio).
            let m = &mut self.members[go[0]];
            if m.command("START_UNIX_MS=0\nACTION=START\n") {
                m.start_sent = Some((0, false));
            } else {
                log("could not send START");
            }
            return;
        }
        // Several: one instant every helper can make, past every speaker's queued audio.
        let lead = GROUP_LEAD_MS * u64::from(1 + self.realigns);
        let head = go.iter().map(|&i| self.members[i].head_ms).max().unwrap_or(0);
        let t0 = (unix_ms() + lead).max(if head > 0 { head + 300 } else { 0 });
        self.epoch_start = Some(t0);
        let names: Vec<String> = go.iter().map(|&i| self.members[i].name.clone()).collect();
        log(format!("{} start together at unix ms {t0}", names.join(", ")));
        for i in go {
            let m = &mut self.members[i];
            let at = instant_of(t0, m.first);
            if m.command(&format!("START_UNIX_MS={at}\nACTION=START\n")) {
                m.start_sent = Some((at, false));
            }
        }
    }

    /// "Playing on <speaker>" only while the speakers get what the user hears.
    fn source_check(&mut self) {
        let connected = self.active().any(|m| m.connected.is_some());
        if !connected {
            self.waiting = None;
            return;
        }
        if self.shared().source_ready() {
            self.waiting = None;
            return;
        }
        let since = *self.waiting.get_or_insert_with(Instant::now);
        if since.elapsed() > self.cfg().capture_timeout && self.failure.is_none() {
            let shared = self.shared();
            log(format!(
                "the player's audio isn't reaching the speakers after {} s (bridge capture {}, bridge {}connected to the PCM socket)",
                self.cfg().capture_timeout.as_secs(),
                if shared.bridge_ready.load(Ordering::SeqCst) { "ready" } else { "not ready" },
                if shared.bridge_linked.load(Ordering::SeqCst) { "" } else { "not " }
            ));
            self.fail_all(Why::NoAudio);
        }
    }

    /// The whole group ends with one message (the computer slept, no audio).
    fn fail_all(&mut self, why: Why) {
        if self.failure.is_some() {
            return;
        }
        let names: Vec<String> = self.active().map(|m| m.name.clone()).collect();
        self.failure = Some((why, names_text(&names)));
        self.shared().casting.store(false, Ordering::SeqCst);
        for m in &mut self.members {
            m.kill(Some(why));
            if m.attempt == 0 {
                m.stopping = true;
            }
        }
        self.members.retain(|m| m.attempt != 0);
    }

    /// The speakers and the summary, sent when they change.
    fn show(&mut self) {
        let ready = self.shared().source_ready();
        let speakers: Vec<Speaker> = self.active().map(|m| Speaker { id: m.id.clone(), name: m.name.clone(), playing: m.started && ready }).collect();
        let cast = match speakers.iter().find(|s| s.playing) {
            Some(s) => Cast::On { id: s.id.clone(), name: s.name.clone(), latency_ms: self.latency },
            None => match speakers.first() {
                Some(s) => Cast::Connecting { id: s.id.clone(), name: s.name.clone() },
                None => Cast::Off,
            },
        };
        if speakers != self.shown.1 {
            self.shown.1 = speakers.clone();
            *self.inner.speakers.lock().unwrap() = speakers.clone();
            self.inner.emit(Event::Speakers(speakers));
        }
        if cast != self.shown.0 {
            if matches!(self.shown.0, Cast::On { .. }) && matches!(cast, Cast::Connecting { .. }) {
                log("the audio source is gone (page reloading?): connecting again");
            }
            // Off is said once the group has closed (after the capture stops).
            if cast != Cast::Off {
                self.shown.0 = cast.clone();
                self.inner.emit(Event::State(cast));
            }
        }
    }

    /// Ends member `i` (the user's choice): the speaker is told to stop.
    fn stop_member(&mut self, i: usize) {
        let shared = self.shared().clone();
        let m = &mut self.members[i];
        if m.stopping {
            return;
        }
        m.stopping = true;
        log(format!("stopping \"{}\"", m.name));
        let mut fan = shared.fan.lock().unwrap();
        m.unfeed(&mut fan);
        drop(fan);
        if m.attempt == 0 {
            // Waiting to try again, or for its name to be found: nothing runs.
            if m.target.is_some() || m.retry_at.is_some() {
                self.remove_at(i);
            }
        } else if m.started && m.command("ACTION=STOP\n") {
            m.kill_at = Some(Instant::now() + Duration::from_millis(1500));
        } else {
            // Nothing plays yet: no goodbye to wait for.
            m.kill(None);
        }
        if self.active().next().is_none() {
            shared.casting.store(false, Ordering::SeqCst); // this computer plays again at once
        }
    }

    fn remove_at(&mut self, i: usize) {
        if i < self.members.len() {
            self.members.remove(i);
        }
        self.show();
    }

    /// Member `i`'s helper ended: what that means for it.
    fn closed(&mut self, i: usize) {
        let cfg = self.cfg().clone();
        let shared = self.shared().clone();
        let m = &mut self.members[i];
        m.alive.store(false, Ordering::SeqCst);
        {
            let mut fan = shared.fan.lock().unwrap();
            m.unfeed(&mut fan);
        }
        m.tx = None;
        m.cmd = None;
        let status = m.child.take().map(|mut c| {
            let pid = c.id();
            shared.pids.lock().unwrap().retain(|p| *p != pid);
            c.wait()
        });
        shared.save_pids(&cfg);
        let _ = std::fs::remove_file(&m.fifo);
        log(format!(
            "session with \"{}\" ended ({}) after {} s",
            m.name,
            match status {
                Some(Ok(s)) => s.to_string(),
                Some(Err(e)) => e.to_string(),
                None => "?".into(),
            },
            m.began.elapsed().as_secs()
        ));
        let end = self.end_of(i);
        self.finish(i, Some(end));
    }

    /// What the end of member `i`'s helper means (its error lines, whether it played).
    fn end_of(&self, i: usize) -> End {
        let m = &self.members[i];
        if m.stopping {
            return End::User;
        }
        if let Some(why) = m.failed {
            return End::Failed(why);
        }
        let Some(target) = m.target.as_ref() else { return End::Failed(Why::Unreachable) };
        let (out, device) = (&target.0, &target.1);
        // A screen to show a PIN on: a HomePod has none, so its refusal is a setting in the Home app.
        let can_pin = !matches!(out.kind, Kind::HomePod | Kind::HomePodMini) && device.ip.is_ipv4() && device.airplay.is_some();
        let password = m.opts.password.is_some();
        match m.error.as_deref() {
            Some("auth_required") if device.wants_password() && !password => End::Auth { need: Need::Password, wrong: false },
            Some("auth_required" | "auth_failed") if password => End::Auth { need: Need::Password, wrong: true },
            Some("auth_failed") if m.opts.auth.is_some() && can_pin => End::StalePairing,
            Some("auth_required") if can_pin && m.opts.auth.is_none() => End::Auth { need: Need::Pin, wrong: false },
            Some("auth_required" | "auth_failed") => End::Failed(Why::Refused),
            Some("connect_failed") if m.connected.is_none() => End::ConnectFailed { http: m.error_http },
            Some(_) => End::Failed(Why::Unreachable),
            None if m.started => End::Failed(Why::Lost),
            None => End::Failed(Why::Unreachable),
        }
    }

    /// Member `i` is done with its helper (`end`; None: it never ran one): tried again, asked
    /// for a password or a PIN, or out of the group with a word to the user.
    fn finish(&mut self, i: usize, end: Option<End>) {
        let end = end.unwrap_or_else(|| End::Failed(self.members[i].failed.unwrap_or(Why::Unreachable)));
        let m = &mut self.members[i];
        let target = m.target.clone();
        match end {
            // A speaker playing for another device (an iPhone, say) may refuse the first session
            // and take one a moment later, as it lets the other sender go: tried again after a
            // short pause, the way Apple's senders take a speaker over.
            End::ConnectFailed { http } if m.tries < self.inner.cfg.retry_backoff.len() && self.failure.is_none() => {
                let pause = self.inner.cfg.retry_backoff[m.tries];
                m.tries += 1;
                let busy = target.as_ref().is_some_and(|t| t.1.busy());
                log(format!(
                    "\"{}\" refused the session{}; trying again in {} ms (try {} of {})",
                    m.name,
                    if busy || http == 453 { " (it plays for another device)" } else { "" },
                    pause.as_millis(),
                    m.tries + 1,
                    self.inner.cfg.retry_backoff.len() + 1
                ));
                m.attempt = 0;
                m.retry_at = Some(Instant::now() + pause);
            }
            End::User => self.remove_at(i),
            End::Failed(why) => self.fail_member(i, why),
            // 453 Not Enough Bandwidth is RTSP's "busy"; else the speaker said so in its TXT flags.
            End::ConnectFailed { http } => {
                let busy = http == 453 || target.as_ref().is_some_and(|t| t.1.busy());
                self.fail_member(i, if busy { Why::Busy } else { Why::Unreachable });
            }
            End::Auth { need, wrong } => {
                let (id, name) = (m.id.clone(), m.name.clone());
                self.remove_at(i);
                let Some(target) = target else { return };
                if need == Need::Password {
                    *self.inner.asked.lock().unwrap() = Some(Asked { target });
                    self.inner.emit(Event::Auth { id, name, need, wrong });
                } else {
                    self.inner.pair(target);
                }
            }
            End::StalePairing => {
                let id = m.id.clone();
                self.remove_at(i);
                self.inner.emit(Event::Forget { id });
                if let Some(target) = target {
                    self.inner.pair(target);
                }
            }
        }
    }

    fn fail_member(&mut self, i: usize, why: Why) {
        let name = self.members[i].name.clone();
        self.members.remove(i);
        if self.failure.is_some() {
            return self.show(); // the group's own message comes at its end
        }
        let still: Vec<String> = self.active().map(|m| m.name.clone()).collect();
        log(format!("\"{name}\": {}", why.message(&name)));
        if still.is_empty() {
            self.shared().casting.store(false, Ordering::SeqCst);
        }
        self.inner.emit(Event::Failed { name, why, still });
        self.show();
    }

    /// The group is over: the source stops, the clock daemon ends, the audio is back here.
    fn close(mut self) {
        let shared = self.shared().clone();
        self.source_alive.store(false, Ordering::SeqCst);
        shared.casting.store(false, Ordering::SeqCst);
        shared.bridge_ready.store(false, Ordering::SeqCst);
        *shared.local_tx.lock().unwrap() = None;
        {
            let mut fan = shared.fan.lock().unwrap();
            *fan = Fanout::default();
        }
        let started = self.pump.is_some();
        if let Some(p) = self.pump.take() {
            p.stop();
        }
        if let Some(mut d) = self.daemon.take() {
            stop_daemon(&mut d, &shared);
        }
        shared.save_pids(&self.inner.cfg);
        if started {
            self.inner.emit(Event::Capture(false));
        }
        if let Some((why, name)) = self.failure.take() {
            log(why.message(&name));
            self.inner.emit(Event::Failed { name, why, still: Vec::new() });
        }
        if !self.shown.1.is_empty() {
            self.inner.speakers.lock().unwrap().clear();
            self.inner.emit(Event::Speakers(Vec::new()));
        }
        self.inner.emit(Event::State(Cast::Off));
    }
}

/// "Bedroom", "Bedroom and Kitchen", "Bedroom, Kitchen and Office".
pub fn names_text(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn pid_file(cfg: &Config) -> PathBuf {
    cfg.dir.with_file_name("uzumaki-airplay-helper.pid")
}

/// Helpers left by a copy of the app that was killed (they end on their own after two minutes; a
/// new session shouldn't wait for that): ended now, if those processes still are cliairplay.
fn end_stale_helpers(cfg: &Config) {
    let path = pid_file(cfg);
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let _ = std::fs::remove_file(&path);
    for pid in text.lines().filter_map(|l| l.trim().parse::<i32>().ok()).filter(|&p| p > 1) {
        let name = Command::new("ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        if name.is_some_and(|n| n.ends_with("cliairplay")) {
            log(format!("ending a helper left by an earlier run (pid {pid})"));
            // SAFETY: a plain signal to a process id we just checked.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
}

/// The shared PTP clock (`cliairplay --ptp-daemon`) every speaker's helper follows, so a group
/// plays on one clock. None when one already runs (another copy, another program: one per
/// computer, the helpers find it) or it couldn't start (each helper then runs its own clock).
fn start_daemon(helper_path: &Path, cfg: &Config, shared: &Shared) -> Option<Child> {
    if helper::daemon_live(Duration::from_millis(150)) {
        log("a shared PTP clock already runs on this computer: the speakers follow it");
        return None;
    }
    let args = helper::daemon_args(&helper::Options::seeded(cfg.seed, 0).dacp);
    log(format!("starting the shared PTP clock: {} {}", helper_path.display(), args.join(" ")));
    let mut child = match spawn_helper(helper_path, &args) {
        Ok(c) => c,
        Err(e) => {
            log(format!("could not start the shared PTP clock: {e}"));
            return None;
        }
    };
    drop(child.stdin.take());
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)].into_iter().flatten() {
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                eprintln!("cliairplay-ptp: {line}");
            }
        });
    }
    shared.pids.lock().unwrap().push(child.id());
    shared.save_pids(cfg);
    // Up before the first speaker's helper looks for it (else that one runs its own clock).
    let until = Instant::now() + Duration::from_millis(1000);
    loop {
        if helper::daemon_live(Duration::from_millis(100)) {
            log("the shared PTP clock is up");
            break;
        }
        if Instant::now() >= until || child.try_wait().ok().flatten().is_some() {
            log("the shared PTP clock didn't answer: each speaker's helper runs its own");
            break;
        }
    }
    Some(child)
}

fn stop_daemon(child: &mut Child, shared: &Shared) {
    let pid = child.id();
    // SAFETY: a plain signal to our child.
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    let until = Instant::now() + Duration::from_millis(800);
    while child.try_wait().ok().flatten().is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    shared.pids.lock().unwrap().retain(|p| *p != pid);
    log("stopped the shared PTP clock");
}

fn spawn_helper(helper_path: &Path, args: &[String]) -> std::io::Result<Child> {
    let mut cmd = Command::new(helper_path);
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Linux: the helper gets SIGTERM when the app dies, however it dies. (Elsewhere its input
    // closes then, it stops sending at once and exits by its own idle timer.)
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: prctl is async-signal-safe; it only sets a flag on the new process.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
    }
    cmd.spawn()
}

fn mkfifo(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| std::io::Error::other("bad path"))?;
    // SAFETY: a valid NUL-terminated path.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Opens the command pipe for writing once the helper reads it (it opens it after connecting).
/// Non-blocking attempts, so a helper that dies first doesn't leave this thread stuck.
fn open_cmd_pipe(path: PathBuf, tx: mpsc::Sender<Ev>, alive: Arc<AtomicBool>, attempt: u64) {
    std::thread::spawn(move || {
        use std::os::unix::fs::OpenOptionsExt;
        let started = Instant::now();
        let mut warned = false;
        while alive.load(Ordering::SeqCst) {
            match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&path) {
                Ok(f) => {
                    use std::os::fd::AsRawFd;
                    // SAFETY: an open descriptor; back to blocking writes.
                    unsafe {
                        let fl = libc::fcntl(f.as_raw_fd(), libc::F_GETFL);
                        libc::fcntl(f.as_raw_fd(), libc::F_SETFL, fl & !libc::O_NONBLOCK);
                    }
                    let _ = tx.send(Ev::CmdReady(attempt, f));
                    return;
                }
                Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                    if !warned && started.elapsed() > Duration::from_secs(10) {
                        warned = true;
                        log("the helper has not opened its command pipe after 10 s");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    log(format!("command pipe: {e}"));
                    return;
                }
            }
        }
    });
}

/// Cuts a byte stream into whole frames: what a read returns may end inside a frame, and the
/// helper's input must only ever get whole ones (the two players take turns on it).
#[derive(Default)]
pub struct Frames {
    carry: Vec<u8>,
}

impl Frames {
    /// The whole frames in `carry + bytes`; the rest waits for the next call.
    pub fn whole<'a>(&'a mut self, bytes: &[u8]) -> &'a [u8] {
        self.carry.extend_from_slice(bytes);
        let n = self.carry.len() - self.carry.len() % FRAME_BYTES as usize;
        &self.carry[..n]
    }

    /// Drops what `whole` returned (call after using it).
    pub fn consume(&mut self) {
        let n = self.carry.len() - self.carry.len() % FRAME_BYTES as usize;
        self.carry.drain(..n);
    }
}

/// The PCM socket: accepts the bridge (one connection at a time, again after it reconnects) and
/// copies its whole frames to the speakers, unless the local player is in charge or a
/// flush is dropping what was on its way.
struct Pump {
    path: PathBuf,
    quit: Arc<AtomicBool>,
    current: Arc<Mutex<Option<UnixStream>>>,
    thread: std::thread::JoinHandle<()>,
}

impl Pump {
    fn start(path: &Path, shared: Arc<Shared>) -> std::io::Result<Self> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        let quit = Arc::new(AtomicBool::new(false));
        let current: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
        let (q, cur) = (quit.clone(), current.clone());
        let thread = std::thread::Builder::new().name("airplay-pcm".into()).spawn(move || {
            let mut buf = vec![0u8; 16384];
            for conn in listener.incoming() {
                if q.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut s) = conn else { continue };
                *cur.lock().unwrap() = s.try_clone().ok();
                shared.bridge_linked.store(true, Ordering::SeqCst);
                log("the bridge is connected to the PCM socket: Apple Music's audio goes to the speaker");
                let mut frames = Frames::default(); // a new connection starts on a frame
                let (mut this, mut next_report) = (0u64, BYTES_PER_SECOND * 60);
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let whole = frames.whole(&buf[..n]);
                            if !whole.is_empty() && !shared.local_active.load(Ordering::SeqCst) && !shared.dropping() {
                                shared.write(whole);
                                this += whole.len() as u64;
                                if this >= next_report {
                                    log(format!("forwarded {} s of Apple Music audio", this / BYTES_PER_SECOND));
                                    next_report += BYTES_PER_SECOND * 60;
                                }
                            }
                            frames.consume();
                        }
                    }
                }
                *cur.lock().unwrap() = None;
                shared.bridge_linked.store(false, Ordering::SeqCst);
                log(format!("the bridge's PCM stream ended after {:.1} s of audio", this as f64 / BYTES_PER_SECOND as f64));
                if q.load(Ordering::SeqCst) {
                    break;
                }
            }
        })?;
        Ok(Pump { path: path.to_path_buf(), quit, current, thread })
    }

    fn stop(self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(s) = self.current.lock().unwrap().as_ref() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        let _ = UnixStream::connect(&self.path); // wakes accept()
        let _ = self.thread.join();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// How much silence to write now so the speaker's input runs `SILENCE_LEAD` ahead of the clock,
/// once nobody has sent audio for `SILENCE_GRACE` (else nothing). In frames.
pub fn silence_due(now: Instant, last_audio: Instant, audio_until: Instant) -> usize {
    if now.duration_since(last_audio) < SILENCE_GRACE {
        return 0;
    }
    let want = now + SILENCE_LEAD;
    if audio_until >= want {
        return 0;
    }
    // At most half a second at once (after a stall the clock is not worth catching up).
    let gap = want.duration_since(audio_until.max(now)).min(Duration::from_millis(500));
    (gap.as_secs_f64() * 44100.0).round() as usize
}

/// The local player's chunks → the helper while the local player is in charge, and silence
/// whenever nobody (the bridge included) has sent audio for a moment: paused, loading, between
/// songs, or no player yet. Lives as long as its session.
fn local_pump(rx: mpsc::Receiver<Vec<i16>>, shared: Arc<Shared>, alive: Arc<AtomicBool>) {
    let epoch = shared.epoch.unwrap_or_else(Instant::now);
    let mut audio_until = Instant::now();
    let (mut sent, mut next_report) = (0u64, 44100u64 * 60);
    while alive.load(Ordering::SeqCst) {
        let chunk = match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let now = Instant::now();
        let local = shared.local_active.load(Ordering::SeqCst);
        let bytes = match chunk {
            Some(c) if c.len() >= 2 && local && !shared.dropping() => {
                let c = &c[..c.len() - c.len() % 2]; // whole stereo frames
                audio_until = audio_until.max(now) + Duration::from_secs_f64((c.len() / 2) as f64 / 44100.0);
                sent += (c.len() / 2) as u64;
                if sent >= next_report {
                    log(format!("forwarded {} s of local audio", sent / 44100));
                    next_report += 44100 * 60;
                }
                to_bytes(c)
            }
            Some(_) => continue, // the bridge is in charge, or a flush drops it
            None => {
                let last = epoch + Duration::from_millis(shared.last_write.load(Ordering::Relaxed));
                match silence_due(now, last.min(now), audio_until) {
                    0 => continue,
                    frames => {
                        audio_until = audio_until.max(now) + Duration::from_secs_f64(frames as f64 / 44100.0);
                        // Silence doesn't count as audio: `last_write` stays where it was.
                        let quiet = vec![0u8; frames * FRAME_BYTES as usize];
                        let keep = shared.last_write.load(Ordering::Relaxed);
                        shared.write(&quiet);
                        shared.last_write.store(keep, Ordering::Relaxed);
                        continue;
                    }
                }
            }
        };
        shared.write(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_only_whole_ones_reach_the_helper() {
        let mut f = Frames::default();
        assert_eq!(f.whole(&[1, 2, 3]), &[] as &[u8]);
        f.consume();
        assert_eq!(f.whole(&[4, 5, 6, 7, 8, 9]), &[1, 2, 3, 4, 5, 6, 7, 8]);
        f.consume();
        assert_eq!(f.whole(&[10, 11, 12]), &[9, 10, 11, 12]);
        f.consume();
        assert_eq!(f.whole(&[]), &[] as &[u8]);
    }

    #[test]
    fn silence_only_after_the_grace_and_up_to_the_lead() {
        let t0 = Instant::now();
        // Audio flowing: nothing.
        assert_eq!(silence_due(t0 + Duration::from_millis(100), t0, t0 + Duration::from_millis(200)), 0);
        // Stopped for longer than the grace, input ran dry: up to 100 ms ahead.
        let now = t0 + Duration::from_millis(400);
        assert_eq!(silence_due(now, t0, t0), 4410);
        // Already 100 ms ahead: nothing more.
        assert_eq!(silence_due(now, t0, now + SILENCE_LEAD), 0);
        // 60 ms ahead: tops up 40 ms.
        assert_eq!(silence_due(now, t0, now + Duration::from_millis(60)), 1764);
    }

    #[test]
    fn every_reason_has_a_sentence_with_the_name() {
        for why in [Why::NotFound, Why::Unreachable, Why::Busy, Why::Refused, Why::Password, Why::Pairing, Why::Lost, Why::Slept, Why::Clock] {
            assert!(why.message("Bedroom").contains("Bedroom"), "{why:?}");
        }
        assert!(Why::NoHelper.message("x").contains("cliairplay"));
    }

    #[test]
    fn an_idle_controller_runs_nothing() {
        let c = Controller::new(Config { helper: None, ..Config::from_env(1) }, |_| {});
        assert!(!c.available());
        assert!(c.inner.browser.lock().unwrap().is_none());
        assert!(c.inner.chain.lock().unwrap().is_none());
        let feed = c.local_feed();
        assert!(!feed.casting() && !feed.mute_local());
        feed.push(vec![0; 8]); // nowhere to go, and no wait
        c.flush();
        c.set_volume(140);
        assert_eq!(c.volume(), 100);
        assert!(!c.inner.cfg.socket().exists());
    }
}
