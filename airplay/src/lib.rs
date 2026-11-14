//! AirPlay: sends what the app plays to one AirPlay speaker on the local network.
//!
//! The audio is the players' own output, already decoded: the bridge taps MusicKit's `<audio>` with
//! Web Audio and writes s16le stereo 44.1 kHz PCM to a Unix socket this crate binds, and the local
//! file player hands its samples to a [`LocalFeed`]. A session runs `cliairplay` (Music
//! Assistant's AirPlay sender, a separate GPL-3.0 program) for the chosen speaker and pumps that
//! PCM into its stdin. Nothing is written to disk; the audio only passes through memory. No keys,
//! no CDM, no encrypted stream is touched.
//!
//! A [`Controller`] is the whole API. It does nothing until asked: no thread and no network use
//! while nobody browses and no speaker is connected.
//!
//! - [`Controller::browse`]: mDNS browsing on or off (the picker, while it is open). Changes of
//!   the device list arrive as [`Event::Devices`].
//! - [`Controller::connect`] / [`Controller::connect_named`]: one session at a time; a new one
//!   ends the old one first. Order inside a session: start the helper → `[STATUS] connected` →
//!   bind the PCM socket and ask for the bridge's capture ([`Event::Capture`]) → `[STATUS] audio
//!   buffered_ms` → `ACTION=START` on the command pipe → `[STATUS] started`: the speaker plays
//!   ([`Cast::On`]).
//! - [`Controller::flush`]: on a seek or a song the user picked, the audio still on its way is
//!   dropped (`ACTION=FLUSH`, then a new START), so the change is heard at once.
//! - A session that ends on its own reports why ([`Event::Failed`]) and the audio is back on
//!   this computer: the local outputs are quiet only while [`LocalFeed::mute_local`] is true and
//!   while the bridge's socket is connected.
//!
//! One session serves both players: while a local file is loaded (`LocalFeed::set_active(true)`)
//! the bridge's bytes are dropped and the local ones go to the helper. When nobody sends audio
//! (paused, between songs, no player yet), silence keeps the speaker's stream alive.
//!
//! Environment, for diagnostics and tests only:
//! - `UZUMAKI_CLIAIRPLAY`: the helper binary (else see `helper::find`).
//! - `UZUMAKI_AIRPLAY_DEBUG`: cliairplay `--debug` level; `UZUMAKI_AIRPLAY_EXTRA_ARGS`: more helper arguments.
//! - `UZUMAKI_AIRPLAY_LOCAL=1`: keep this computer's output on while casting (A/B checks).
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
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
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
    /// How long `connect_named` looks for the speaker.
    pub discover_timeout: Duration,
    /// A speaker that doesn't answer our clock plays silence: after this long the session ends.
    pub stall_grace: Duration,
    /// Devices to show instead of browsing (tests: no network).
    pub fake: Vec<Device>,
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
            discover_timeout: Duration::from_secs(8),
            stall_grace: Duration::from_secs(6),
            fake,
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("pcm.sock")
    }
}

/// Where the audio goes.
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
}

impl Why {
    /// One sentence for the user.
    pub fn message(self, name: &str) -> String {
        match self {
            Why::NoHelper => "AirPlay isn't available: the cliairplay helper is missing from this install.".into(),
            Why::NotFound => format!("Couldn't find “{name}” on this network."),
            Why::Unreachable => format!("Couldn't connect to “{name}”. Check that it is on and on the same network."),
            Why::Refused => format!("“{name}” only accepts people of its home. In the Home app, allow speaker access for “Anyone on the Same Network”."),
            Why::Password => format!("Wrong password for “{name}”."),
            Why::Pairing => format!("Couldn't pair with “{name}”. Check the code and try again."),
            Why::Lost => format!("The connection to “{name}” was lost."),
            Why::Slept => format!("AirPlay to “{name}” stopped while the computer slept."),
            Why::Clock => format!("“{name}” didn't answer. A firewall may be blocking AirPlay (UDP ports 319 and 320)."),
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

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The picker's rows changed.
    Devices(Vec<Output>),
    State(Cast),
    /// The bridge's capture must run (a speaker is connected) or stop.
    Capture(bool),
    /// A session ended on its own; the audio is back on this computer.
    Failed { name: String, why: Why },
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

#[derive(Default)]
struct Shared {
    child: Mutex<Option<Child>>,
    cmd: Mutex<Option<std::fs::File>>,
    /// The current helper's input. Every write is whole frames, under this lock, so the bridge's
    /// and the local player's audio never interleave inside a frame.
    stdin: Mutex<Option<ChildStdin>>,
    /// A helper is connected to the speaker and takes audio (the local outputs go quiet).
    casting: AtomicBool,
    /// The local player has a song loaded: its audio (or silence) goes to the speaker, the bridge's doesn't.
    local_active: AtomicBool,
    keep_local: bool,
    /// The local player's way in, while a session runs.
    local_tx: Mutex<Option<mpsc::SyncSender<Vec<i16>>>>,
    /// Milliseconds since `epoch`: the last write to the helper, and until when incoming audio
    /// is dropped after a flush.
    last_write: AtomicU64,
    drop_until: AtomicU64,
    /// Wall-clock ms of the first audio written since the connection or the last flush (0: none yet).
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

    /// Whole frames to the helper. Fails when there is none or it stopped reading.
    fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        match self.stdin.lock().unwrap().as_mut() {
            Some(s) => {
                s.write_all(bytes)?;
                self.last_write.store(self.now_ms(), Ordering::Relaxed);
                let _ = self.first_audio.compare_exchange(0, unix_ms(), Ordering::Relaxed, Ordering::Relaxed);
                Ok(())
            }
            None => Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        }
    }

    fn command(&self, lines: &str) -> bool {
        let mut cmd = self.cmd.lock().unwrap();
        let Some(f) = cmd.as_mut() else { return false };
        f.write_all(lines.as_bytes()).and_then(|_| f.flush()).is_ok()
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
    Volume(u8),
    Flush,
}

enum Ev {
    Line(Status),
    CmdReady(std::fs::File),
    Closed,
    Cmd(Cmd),
}

struct Session {
    tx: mpsc::Sender<Ev>,
    done: Arc<AtomicBool>,
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
    session: Mutex<Option<Session>>,
    /// The last session's thread: the next one waits for it, so two never share the socket.
    chain: Mutex<Option<std::thread::JoinHandle<()>>>,
    pairing: Mutex<Option<Pairing>>,
    asked: Mutex<Option<Asked>>,
    volume: AtomicU8,
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
                session: Mutex::default(),
                chain: Mutex::default(),
                pairing: Mutex::default(),
                asked: Mutex::default(),
                volume: AtomicU8::new(30),
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

    /// Sends the audio to the device with this id (from `Event::Devices`). False when the id
    /// isn't known (any more).
    pub fn connect(&self, id: &str, opts: Connect) -> bool {
        let found = self.inner.devices.lock().unwrap().iter().find(|(o, _)| o.id == id).cloned();
        match found {
            Some(t) => {
                self.inner.start(Target::Known(Box::new(t)), opts);
                true
            }
            None => false,
        }
    }

    /// Looks for the speaker called `name` (as in the picker, any case) and sends the audio there.
    pub fn connect_named(&self, name: &str, opts: Connect) {
        let known = self.inner.devices.lock().unwrap().iter().find(|(o, _)| o.name.eq_ignore_ascii_case(name.trim())).cloned();
        match known {
            Some(t) => self.inner.start(Target::Known(Box::new(t)), opts),
            None => self.inner.start(Target::Named(name.trim().to_string()), opts),
        }
    }

    /// Back to this computer: ends the session, a pairing in progress or a question.
    pub fn disconnect(&self) {
        self.inner.cancel_pairing();
        *self.inner.asked.lock().unwrap() = None;
        if let Some(s) = self.inner.session.lock().unwrap().take() {
            let _ = s.tx.send(Ev::Cmd(Cmd::Stop));
        }
    }

    /// The answer to `Event::Auth`: the password, or the PIN the device shows.
    pub fn answer(&self, text: &str) {
        let text = text.trim();
        let asked = self.inner.asked.lock().unwrap().take();
        if let Some(a) = asked {
            let opts = Connect { volume: self.volume(), password: Some(text.to_string()), auth: None };
            return self.inner.start(Target::Known(a.target), opts);
        }
        if let Some(p) = self.inner.pairing.lock().unwrap().as_mut()
            && let Some(mut stdin) = p.stdin.take()
        {
            log(format!("pairing with \"{}\": sending the code", p.name));
            let _ = writeln!(stdin, "{text}");
            // Dropped here: the helper reads one line.
        }
    }

    /// The speaker's own volume, 0-100. Kept for the next session too.
    pub fn set_volume(&self, volume: u8) {
        let v = volume.min(100);
        self.inner.volume.store(v, Ordering::Relaxed);
        if let Some(s) = self.inner.session.lock().unwrap().as_ref() {
            let _ = s.tx.send(Ev::Cmd(Cmd::Volume(v)));
        }
    }

    pub fn volume(&self) -> u8 {
        self.inner.volume.load(Ordering::Relaxed)
    }

    /// The user seeked or picked another song: drop the audio still on its way to the speaker.
    /// Nothing happens while no speaker plays.
    pub fn flush(&self) {
        if !self.inner.shared.casting.load(Ordering::Relaxed) {
            return;
        }
        if let Some(s) = self.inner.session.lock().unwrap().as_ref() {
            let _ = s.tx.send(Ev::Cmd(Cmd::Flush));
        }
    }

    /// Ends everything (quit): tells the speaker to stop, ends the helper, removes the socket.
    /// Waits up to about two seconds for the helper to say goodbye to the speaker.
    pub fn shutdown(&self) {
        let inner = &self.inner;
        if inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.browse(false);
        inner.cancel_pairing();
        inner.shared.casting.store(false, Ordering::SeqCst); // the local outputs play again at once
        let session = inner.session.lock().unwrap().take();
        if let Some(s) = session {
            let _ = s.tx.send(Ev::Cmd(Cmd::Stop));
            let deadline = Instant::now() + Duration::from_millis(2200);
            while !s.done.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        // Whatever is left (a session that didn't end in time): the helper must not outlive us.
        if let Some(mut c) = inner.shared.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
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

    /// Ends the session there is and starts one for `target`, on its own thread.
    fn start(self: &Arc<Self>, target: Target, opts: Connect) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.cancel_pairing();
        *self.asked.lock().unwrap() = None;
        self.volume.store(opts.volume.min(100), Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<Ev>();
        let done = Arc::new(AtomicBool::new(false));
        let old = self.session.lock().unwrap().replace(Session { tx: tx.clone(), done: done.clone() });
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
            session(&inner, tx, rx, target, opts);
            done.store(true, Ordering::SeqCst);
        });
        match spawned {
            Ok(t) => *chain = Some(t),
            Err(e) => log(format!("could not start the session thread: {e}")),
        }
    }

    /// HomeKit pairing: the device shows a PIN, `answer` sends it, the credentials come back as
    /// `Event::Paired` and the session starts with them.
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
                return self.emit(Event::Failed { name: out.name.clone(), why: Why::Pairing });
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
                    inner.start(Target::Known(p.then), opts);
                }
                None => {
                    log(format!("pairing with \"{name}\" failed"));
                    inner.emit(Event::Failed { name, why: Why::Pairing });
                }
            }
        });
    }
}

/// How a session ended.
enum End {
    /// The user (or a newer session) stopped it.
    User,
    Failed(Why),
    Auth { need: Need, wrong: bool },
    /// The stored pairing was refused.
    StalePairing,
}

fn session(inner: &Arc<Inner>, tx: mpsc::Sender<Ev>, rx: mpsc::Receiver<Ev>, target: Target, opts: Connect) {
    let stopped = |rx: &mpsc::Receiver<Ev>| rx.try_iter().any(|e| matches!(e, Ev::Cmd(Cmd::Stop)));
    if stopped(&rx) {
        return;
    }
    let (name, id) = match &target {
        Target::Known(t) => (t.0.name.clone(), t.0.id.clone()),
        Target::Named(n) => (n.clone(), n.to_lowercase()),
    };
    inner.emit(Event::State(Cast::Connecting { id, name: name.clone() }));
    let fail = |why: Why| {
        log(format!("\"{name}\": {}", why.message(&name)));
        inner.emit(Event::Failed { name: name.clone(), why });
        inner.emit(Event::State(Cast::Off));
    };
    let Some(helper_path) = inner.cfg.helper.clone() else {
        return fail(Why::NoHelper);
    };
    let target: Box<(Output, Device)> = match target {
        Target::Known(t) => t,
        Target::Named(n) => {
            let fake = inner.cfg.fake.iter().find(|d| d.name.eq_ignore_ascii_case(&n)).cloned();
            let found = match fake {
                Some(d) => Some(d),
                None if !inner.cfg.fake.is_empty() => None,
                None => {
                    log(format!("looking for \"{n}\" on the network (mDNS, up to {} s)", inner.cfg.discover_timeout.as_secs()));
                    match discover::discover(inner.cfg.discover_timeout, Some(&n)) {
                        Ok(devices) => discover::outputs(&devices, &discover::own_addresses()).into_iter().map(|(_, d)| d).find(|d| d.name.eq_ignore_ascii_case(&n)),
                        Err(e) => {
                            log(format!("discovery failed: {e}"));
                            None
                        }
                    }
                }
            };
            if stopped(&rx) {
                return inner.emit(Event::State(Cast::Off));
            }
            match found.and_then(|d| discover::outputs(std::slice::from_ref(&d), &[]).pop()) {
                Some(t) => {
                    log(format!("found {}", discover::describe(&t.1)));
                    inner.emit(Event::State(Cast::Connecting { id: t.0.id.clone(), name: t.0.name.clone() }));
                    Box::new(t)
                }
                None => return fail(Why::NotFound),
            }
        }
    };
    let (out, device) = (&target.0, &target.1);
    let options = helper::Options {
        debug: inner.cfg.debug,
        extra: inner.cfg.extra.clone(),
        password: opts.password.clone(),
        auth: opts.auth.clone(),
        ..helper::Options::seeded(inner.cfg.seed, opts.volume.min(100))
    };
    let end = run(inner, &tx, &rx, &helper_path, out, device, &options);
    match end {
        End::User => inner.emit(Event::State(Cast::Off)),
        End::Failed(why) => fail(why),
        End::Auth { need: Need::Password, wrong } => {
            inner.emit(Event::State(Cast::Off));
            *inner.asked.lock().unwrap() = Some(Asked { target: target.clone() });
            inner.emit(Event::Auth { id: out.id.clone(), name: out.name.clone(), need: Need::Password, wrong });
        }
        End::Auth { need: Need::Pin, .. } => {
            inner.emit(Event::State(Cast::Off));
            inner.pair(target.clone());
        }
        End::StalePairing => {
            inner.emit(Event::Forget { id: out.id.clone() });
            inner.emit(Event::State(Cast::Off));
            inner.pair(target.clone());
        }
    }
}

fn pid_file(cfg: &Config) -> PathBuf {
    cfg.dir.with_file_name("uzumaki-airplay-helper.pid")
}

/// A helper left by a copy of the app that was killed (it ends on its own after two minutes; a
/// new session shouldn't wait for that): ended now, if that process still is a cliairplay.
fn end_stale_helper(cfg: &Config) {
    let path = pid_file(cfg);
    let Some(pid) = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<i32>().ok()) else { return };
    let _ = std::fs::remove_file(&path);
    if pid <= 1 {
        return;
    }
    let name = Command::new("ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if name.is_some_and(|n| n.ends_with("cliairplay")) {
        log(format!("ending a helper left by an earlier run (pid {pid})"));
        // SAFETY: a plain signal to a process id we just checked.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
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

#[allow(clippy::too_many_lines)]
fn run(inner: &Arc<Inner>, tx: &mpsc::Sender<Ev>, rx: &mpsc::Receiver<Ev>, helper_path: &Path, out: &Output, device: &Device, options: &helper::Options) -> End {
    let cfg = &inner.cfg;
    let shared = &inner.shared;
    if let Err(e) = std::fs::create_dir_all(&cfg.dir) {
        log(format!("{}: {e}", cfg.dir.display()));
        return End::Failed(Why::Unreachable);
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&cfg.dir, std::fs::Permissions::from_mode(0o700));
    }
    end_stale_helper(cfg);
    let fifo = cfg.dir.join("cmd.fifo");
    let _ = std::fs::remove_file(&fifo);
    if let Err(e) = mkfifo(&fifo) {
        log(format!("command pipe {}: {e}", fifo.display()));
        return End::Failed(Why::Unreachable);
    }
    let args = helper::args(device, &fifo, options);
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
    let mut child = match spawn_helper(helper_path, &args) {
        Ok(c) => c,
        Err(e) => {
            log(format!("could not start cliairplay: {e}"));
            let _ = std::fs::remove_file(&fifo);
            return End::Failed(Why::NoHelper);
        }
    };
    let _ = std::fs::write(pid_file(cfg), child.id().to_string());
    *shared.stdin.lock().unwrap() = child.stdin.take();
    let readers = Arc::new(AtomicU64::new(2));
    let streams = [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)];
    for stream in streams.into_iter().flatten() {
        let tx = tx.clone();
        let readers = readers.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                eprintln!("cliairplay: {line}");
                if let Some(s) = helper::parse_status(&line) {
                    let _ = tx.send(Ev::Line(s));
                }
            }
            if readers.fetch_sub(1, Ordering::SeqCst) == 1 {
                let _ = tx.send(Ev::Closed);
            }
        });
    }
    *shared.child.lock().unwrap() = Some(child);

    let alive = Arc::new(AtomicBool::new(true));
    let mut pump: Option<Pump> = None;
    let mut connected = false;
    let mut buffered = false;
    let mut start_sent = false;
    let mut start_tries = 0u32;
    let mut started = false;
    let mut latency: Option<u32> = None;
    let mut error: Option<String> = None;
    let mut user = false;
    let mut failed: Option<Why> = None;
    let mut kill_at: Option<Instant> = None;
    let mut stalled: Option<Instant> = None;
    let began = Instant::now();
    let (mut wall, mut mono) = (SystemTime::now(), Instant::now());
    let send_start = |shared: &Shared| {
        let ok = shared.command("START_UNIX_MS=0\nACTION=START\n");
        if !ok {
            log("could not send START");
        }
        ok
    };
    loop {
        let ev = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ev) => Some(ev),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // The clocks: a sleep shows as the wall clock running ahead of the process clock.
        let (w, m) = (SystemTime::now(), Instant::now());
        let jump = w.duration_since(wall).unwrap_or_default().saturating_sub(m.duration_since(mono));
        (wall, mono) = (w, m);
        let kill = |why: Option<Why>, failed: &mut Option<Why>| {
            if failed.is_none() {
                *failed = why;
            }
            if let Some(c) = shared.child.lock().unwrap().as_mut() {
                let _ = c.kill();
            }
        };
        if jump > SLEEP_JUMP && !user {
            log(format!("the computer slept ({} s): ending the session", jump.as_secs()));
            kill(Some(Why::Slept), &mut failed);
        }
        if !connected && !user && failed.is_none() && began.elapsed() > cfg.connect_timeout {
            log(format!("no answer from \"{}\" after {} s", out.name, cfg.connect_timeout.as_secs()));
            kill(Some(Why::Unreachable), &mut failed);
        }
        if stalled.is_some_and(|t| t.elapsed() > cfg.stall_grace) && failed.is_none() && !user {
            stalled = None;
            log(helper::STALLED_HINT);
            kill(Some(Why::Clock), &mut failed);
        }
        if kill_at.is_some_and(|t| Instant::now() >= t) {
            kill_at = None;
            kill(None, &mut failed);
        }
        let Some(ev) = ev else { continue };
        match ev {
            Ev::Cmd(Cmd::Stop) => {
                if !user {
                    user = true;
                    log(format!("stopping \"{}\"", out.name));
                    shared.casting.store(false, Ordering::SeqCst); // this computer plays again at once
                    if started && shared.command("ACTION=STOP\n") {
                        kill_at = Some(Instant::now() + Duration::from_millis(1500));
                    } else {
                        // Nothing plays yet: no goodbye to wait for.
                        kill(None, &mut failed);
                    }
                }
            }
            Ev::Cmd(Cmd::Volume(v)) => {
                if connected && shared.command(&format!("VOLUME={v}\n")) {
                    log(format!("volume {v}"));
                }
            }
            Ev::Cmd(Cmd::Flush) => {
                // Audio already on its way is the old position's.
                shared.drop_until.store(shared.now_ms() + FLUSH_DROP.as_millis() as u64, Ordering::Relaxed);
                if start_sent && !user && shared.command("ACTION=FLUSH\n") {
                    log("flush (seek or new song): the speaker starts again from the new audio");
                    start_sent = false;
                    buffered = false;
                    shared.first_audio.store(0, Ordering::Relaxed);
                }
            }
            Ev::Line(Status::Route(r)) => log(format!("route: {r}")),
            Ev::Line(Status::Connected) => {
                connected = true;
                log(format!("connected to \"{}\" in {} ms; opening the PCM socket for the bridge", out.name, began.elapsed().as_millis()));
                shared.first_audio.store(0, Ordering::Relaxed);
                let (ltx, lrx) = mpsc::sync_channel::<Vec<i16>>(LOCAL_QUEUE);
                *shared.local_tx.lock().unwrap() = Some(ltx);
                let (s, a) = (shared.clone(), alive.clone());
                let _ = std::thread::Builder::new().name("airplay-local".into()).spawn(move || local_pump(lrx, s, a));
                match Pump::start(&cfg.socket(), shared.clone()) {
                    Ok(p) => pump = Some(p),
                    Err(e) => log(format!("PCM socket {}: {e}", cfg.socket().display())),
                }
                if !user {
                    shared.casting.store(true, Ordering::SeqCst);
                    inner.emit(Event::Capture(true));
                }
                open_cmd_pipe(fifo.clone(), tx.clone(), alive.clone());
            }
            Ev::CmdReady(f) => {
                *shared.cmd.lock().unwrap() = Some(f);
                // A volume chosen while connecting.
                let v = inner.volume.load(Ordering::Relaxed);
                if v != options.volume {
                    shared.command(&format!("VOLUME={v}\n"));
                }
                if buffered && !start_sent {
                    start_sent = send_start(shared);
                }
            }
            Ev::Line(Status::Buffered { ms }) => {
                log(format!("audio buffered ({ms} ms)"));
                buffered = true;
                if !start_sent && shared.cmd.lock().unwrap().is_some() {
                    start_sent = send_start(shared);
                }
            }
            Ev::Line(Status::Started { requested_ms, at_ms }) => {
                let first = shared.first_audio.load(Ordering::Relaxed);
                if first > 0 && at_ms > first {
                    latency = Some((at_ms - first).min(60_000) as u32);
                }
                log(format!(
                    "the speaker plays from unix ms {at_ms} ({} ms from now; requested {requested_ms}); a sound is heard about {} ms after the player makes it",
                    at_ms as i64 - unix_ms() as i64,
                    latency.map_or("?".into(), |l| l.to_string())
                ));
                start_tries = 0;
                started = true;
                if !user {
                    inner.emit(Event::State(Cast::On { id: out.id.clone(), name: out.name.clone(), latency_ms: latency }));
                }
            }
            Ev::Line(Status::Flushed) => log("flushed"),
            Ev::Line(Status::Latency { warm_lead_ms }) => log(format!("the speaker keeps {warm_lead_ms} ms of audio queued")),
            Ev::Line(Status::ClockReady { mode, state }) => {
                log(format!("clock {mode}: {state}"));
                stalled = (state == "stalled").then(Instant::now);
            }
            Ev::Line(Status::Error { code, http, detail }) => {
                log(format!("ERROR {code} (http {http}): {detail}"));
                log(format!("  → {}", helper::explain(&code)));
                match code.as_str() {
                    // The stream goes on as it was.
                    "flush_failed" => start_sent = true,
                    "start_failed" => {
                        start_tries += 1;
                        start_sent = false;
                        if start_tries >= 3 {
                            kill(Some(Why::Lost), &mut failed);
                        } else if buffered {
                            start_sent = send_start(shared);
                        }
                    }
                    "auth_required" | "auth_failed" | "connect_failed" => error = Some(code),
                    _ => {}
                }
            }
            Ev::Line(Status::Eof) => log("helper input ended"),
            Ev::Line(Status::IdleTimeout) => log("helper idle timeout"),
            Ev::Line(Status::Stopped) => log("helper stopped"),
            Ev::Closed => break,
        }
    }
    alive.store(false, Ordering::SeqCst);
    shared.casting.store(false, Ordering::SeqCst);
    let status = shared.child.lock().unwrap().take().map(|mut c| c.wait());
    *shared.stdin.lock().unwrap() = None;
    *shared.cmd.lock().unwrap() = None;
    *shared.local_tx.lock().unwrap() = None;
    if let Some(p) = pump.take() {
        p.stop();
    }
    let _ = std::fs::remove_file(&fifo);
    let _ = std::fs::remove_file(pid_file(cfg));
    if connected {
        inner.emit(Event::Capture(false));
    }
    log(format!(
        "session with \"{}\" ended ({}) after {} s",
        out.name,
        match status {
            Some(Ok(s)) => s.to_string(),
            Some(Err(e)) => e.to_string(),
            None => "?".into(),
        },
        began.elapsed().as_secs()
    ));
    if user {
        return End::User;
    }
    if let Some(why) = failed {
        return End::Failed(why);
    }
    // A screen to show a PIN on: a HomePod has none, so its refusal is a setting in the Home app.
    let can_pin = !matches!(out.kind, Kind::HomePod | Kind::HomePodMini) && device.ip.is_ipv4() && device.airplay.is_some();
    match error.as_deref() {
        Some("auth_required") if device.wants_password() && options.password.is_none() => End::Auth { need: Need::Password, wrong: false },
        Some("auth_required" | "auth_failed") if options.password.is_some() => End::Auth { need: Need::Password, wrong: true },
        Some("auth_failed") if options.auth.is_some() && can_pin => End::StalePairing,
        Some("auth_required") if can_pin && options.auth.is_none() => End::Auth { need: Need::Pin, wrong: false },
        Some("auth_required" | "auth_failed") => End::Failed(Why::Refused),
        Some(_) => End::Failed(Why::Unreachable),
        None if started => End::Failed(Why::Lost),
        None => End::Failed(Why::Unreachable),
    }
}

fn mkfifo(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| std::io::Error::other("bad path"))?;
    // SAFETY: a valid NUL-terminated path.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Opens the command pipe for writing once the helper reads it (it opens it after connecting).
/// Non-blocking attempts, so a helper that dies first doesn't leave this thread stuck.
fn open_cmd_pipe(path: PathBuf, tx: mpsc::Sender<Ev>, alive: Arc<AtomicBool>) {
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
                    let _ = tx.send(Ev::CmdReady(f));
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
/// copies its whole frames into the helper's stdin, unless the local player is in charge or a
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
                log("the bridge is connected to the PCM socket: Apple Music's audio goes to the speaker");
                let mut frames = Frames::default(); // a new connection starts on a frame
                let mut ok = true;
                let (mut this, mut next_report) = (0u64, BYTES_PER_SECOND * 60);
                while ok {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let whole = frames.whole(&buf[..n]);
                            if !whole.is_empty() && !shared.local_active.load(Ordering::SeqCst) && !shared.dropping() {
                                ok = shared.write(whole).is_ok();
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
                log(format!("the bridge's PCM stream ended after {:.1} s of audio{}", this as f64 / BYTES_PER_SECOND as f64, if ok { "" } else { " (helper input closed)" }));
                if !ok || q.load(Ordering::SeqCst) {
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
                        let _ = shared.write(&quiet);
                        shared.last_write.store(keep, Ordering::Relaxed);
                        continue;
                    }
                }
            }
        };
        let _ = shared.write(&bytes);
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
        for why in [Why::NotFound, Why::Unreachable, Why::Refused, Why::Password, Why::Pairing, Why::Lost, Why::Slept, Why::Clock] {
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
