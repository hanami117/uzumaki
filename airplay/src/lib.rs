//! AirPlay spike (stages 1-2 of docs/research/airplay-sender.md). Off unless `UZUMAKI_AIRPLAY_SPIKE=1`.
//!
//! The bridge taps MusicKit's `<audio>` (Web Audio, the player's own decoded output) and writes s16le
//! stereo 44.1 kHz PCM to a Unix socket this crate binds. A session thread finds one speaker by
//! name (mDNS), runs `cliairplay` for it and pumps the socket into the helper's stdin. Nothing is
//! written to disk; the audio only passes through memory.
//!
//! Order: discover → start the helper → `[STATUS] connected` → bind the PCM socket (the bridge
//! connects within a second and mutes its local output while connected) → `[STATUS] audio
//! buffered_ms` → `ACTION=START` on the command pipe → the speaker plays. When the helper ends, the
//! socket goes away, the bridge plays locally again, and the session tries again (not after a
//! pairing error, which needs the user).
//!
//! Environment:
//! - `UZUMAKI_AIRPLAY_SPIKE=1`: on.
//! - `UZUMAKI_AIRPLAY_TARGET`: the speaker's name as in Apple's picker (default `Bedroom`).
//! - `UZUMAKI_AIRPLAY_HOST`: an address instead of mDNS (tests; then `UZUMAKI_AIRPLAY_PORT`, default 7000).
//! - `UZUMAKI_CLIAIRPLAY`: the helper binary (else see `helper::find`).
//! - `UZUMAKI_AIRPLAY_VOLUME`: the speaker's volume at start, 0-100 (default 30).
//! - `UZUMAKI_AIRPLAY_DEBUG`: cliairplay `--debug` level; `UZUMAKI_AIRPLAY_EXTRA_ARGS`: more helper arguments.
//! - `UZUMAKI_AIRPLAY_LOCAL=1`: keep this computer's output on while casting (A/B checks).
//!
//! Local files (stage 4 of the plan, early): the app's in-process player taps its own decoded
//! output (resampled to s16 stereo 44.1 kHz) and hands it to a [`LocalFeed`]. One helper session
//! serves both players: while a local file is loaded (`LocalFeed::set_active(true)`) the bridge's
//! bytes are dropped and the local ones go to the helper, and while the local player is paused or
//! between songs silence keeps the speaker's stream going, as the bridge's worklet does.

pub mod discover;
pub mod helper;

use discover::{Device, Service};
use helper::Status;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// s16le stereo: the helper's input must stay aligned to whole frames.
const FRAME_BYTES: u64 = 4;
const BYTES_PER_SECOND: u64 = 44100 * FRAME_BYTES;
const MAX_ATTEMPTS: u32 = 5;

fn log(msg: impl AsRef<str>) {
    eprintln!("airplay: {}", msg.as_ref());
}

pub fn enabled() -> bool {
    std::env::var_os("UZUMAKI_AIRPLAY_SPIKE").is_some_and(|v| v == "1")
}

#[derive(Clone, Debug)]
pub struct Config {
    pub target: String,
    /// Skip mDNS and use this address (tests, or a speaker mDNS can't see).
    pub host: Option<(std::net::IpAddr, u16)>,
    pub helper: Option<PathBuf>,
    pub options: helper::Options,
    /// Where the socket and the command pipe live (a private folder).
    pub dir: PathBuf,
    pub discover_timeout: Duration,
    pub retry_delay: Duration,
    /// Keep this computer's output on while casting (`UZUMAKI_AIRPLAY_LOCAL=1`).
    pub keep_local: bool,
}

impl Config {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let host = var("UZUMAKI_AIRPLAY_HOST").and_then(|h| h.trim().parse().ok()).map(|ip| {
            (ip, var("UZUMAKI_AIRPLAY_PORT").and_then(|p| p.trim().parse().ok()).unwrap_or(7000))
        });
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let seed = nanos ^ (u64::from(std::process::id()) << 32);
        Config {
            target: var("UZUMAKI_AIRPLAY_TARGET").unwrap_or_else(|| "Bedroom".into()),
            host,
            helper: helper::find(var("UZUMAKI_CLIAIRPLAY").map(PathBuf::from)),
            options: helper::Options {
                volume: var("UZUMAKI_AIRPLAY_VOLUME").and_then(|v| v.trim().parse().ok()).unwrap_or(30),
                debug: var("UZUMAKI_AIRPLAY_DEBUG").and_then(|v| v.trim().parse().ok()),
                extra: var("UZUMAKI_AIRPLAY_EXTRA_ARGS").map(|v| v.split_whitespace().map(String::from).collect()).unwrap_or_default(),
                dacp: format!("{:016X}", seed),
                active_remote: (seed >> 16) as u32 & 0x7fff_ffff,
            },
            dir: std::env::temp_dir().join(format!("uzumaki-airplay-{}", std::process::id())),
            discover_timeout: Duration::from_secs(10),
            retry_delay: Duration::from_secs(5),
            keep_local: var("UZUMAKI_AIRPLAY_LOCAL").is_some_and(|v| v.trim() == "1"),
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("pcm.sock")
    }

    fn fifo(&self) -> PathBuf {
        self.dir.join("cmd.fifo")
    }
}

/// A running spike: the session thread and what `shutdown` needs.
pub struct Spike {
    socket: PathBuf,
    dir: PathBuf,
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    local: LocalFeed,
}

#[derive(Default)]
struct Shared {
    quit: AtomicBool,
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
}

/// Size of the local player's chunks in the queue: 4096 frames (~93 ms) each, as the bridge's.
const LOCAL_QUEUE: usize = 32;
/// The local player's audio stops (paused, between songs, loading): silence after this long.
const LOCAL_GRACE: Duration = Duration::from_millis(250);
/// How far ahead of the clock the silence is written.
const SILENCE_LEAD: Duration = Duration::from_millis(100);

/// What the app's local-file player uses to send its audio to the speaker. Cheap to clone.
#[derive(Clone)]
pub struct LocalFeed {
    tx: mpsc::SyncSender<Vec<i16>>,
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

    /// Interleaved s16 stereo 44.1 kHz frames. Never blocks (called on the audio thread): when the
    /// queue is full the chunk is dropped.
    pub fn push(&self, chunk: Vec<i16>) {
        let _ = self.tx.try_send(chunk);
    }

    /// The local player has a song loaded (true) or handed over to the bridge (false).
    pub fn set_active(&self, on: bool) {
        if self.shared.local_active.swap(on, Ordering::SeqCst) != on {
            log(if on { "local player in charge: the speaker gets the local file" } else { "bridge in charge: the speaker gets Apple Music" });
        }
    }
}

static SPIKE: OnceLock<Option<Spike>> = OnceLock::new();

/// The spike, started on first use (once per process, by the bridge or the local player). None
/// unless `UZUMAKI_AIRPLAY_SPIKE=1`.
fn global() -> Option<&'static Spike> {
    if !enabled() {
        return None;
    }
    SPIKE.get_or_init(|| {
        let s = Spike::start(Config::from_env()).map_err(|e| log(format!("not started: {e}"))).ok();
        // Every way of quitting reaches atexit (Cmd+Q included): stop the speaker then, not 120 s
        // later when the helper's idle timer notices its input is gone.
        extern "C" fn at_exit() {
            shutdown();
        }
        // SAFETY: registers a plain function; it only touches this crate's statics.
        if s.is_some() && unsafe { libc::atexit(at_exit) } != 0 {
            log("could not register the exit handler");
        }
        s
    })
    .as_ref()
}

/// The environment the bridge needs, starting the spike on first use. Empty unless
/// `UZUMAKI_AIRPLAY_SPIKE=1`, so normal runs are unchanged.
pub fn bridge_env() -> Vec<(&'static str, OsString)> {
    match global() {
        Some(s) => vec![("UZUMAKI_AIRPLAY_PCM_SOCKET", s.socket.clone().into_os_string())],
        None => Vec::new(),
    }
}

/// The local player's way to the speaker, starting the spike on first use. None unless
/// `UZUMAKI_AIRPLAY_SPIKE=1`, so the local player stays as it was.
pub fn local_feed() -> Option<LocalFeed> {
    global().map(|s| s.local_feed())
}

/// Ends the session (if any): tells the speaker to stop, ends the helper, removes the socket.
pub fn shutdown() {
    if let Some(Some(s)) = SPIKE.get() {
        s.stop();
    }
}

impl Spike {
    pub fn start(cfg: Config) -> Result<Self, String> {
        std::fs::create_dir_all(&cfg.dir).map_err(|e| format!("{}: {e}", cfg.dir.display()))?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&cfg.dir, std::fs::Permissions::from_mode(0o700));
        let socket = cfg.socket();
        let _ = std::fs::remove_file(&socket);
        log(format!(
            "spike on: target \"{}\", helper {}, volume {}, socket {}",
            cfg.target,
            cfg.helper.as_ref().map_or("NOT FOUND".into(), |p| p.display().to_string()),
            cfg.options.volume,
            socket.display()
        ));
        let shared = Arc::new(Shared { keep_local: cfg.keep_local, ..Shared::default() });
        let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(LOCAL_QUEUE);
        let s1 = shared.clone();
        std::thread::Builder::new().name("airplay-local".into()).spawn(move || local_pump(rx, s1)).map_err(|e| e.to_string())?;
        let s2 = shared.clone();
        let cfg_dir = cfg.dir.clone();
        let thread = std::thread::Builder::new().name("airplay".into()).spawn(move || session(cfg, s2)).map_err(|e| e.to_string())?;
        let local = LocalFeed { tx, shared: shared.clone() };
        Ok(Spike { socket, dir: cfg_dir, shared, thread: Mutex::new(Some(thread)), local })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn local_feed(&self) -> LocalFeed {
        self.local.clone()
    }

    /// A speaker takes audio now.
    pub fn casting(&self) -> bool {
        self.shared.casting.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        if self.shared.quit.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shared.casting.store(false, Ordering::SeqCst); // the local outputs play again at once
        if let Some(f) = self.shared.cmd.lock().unwrap().as_mut() {
            let _ = f.write_all(b"ACTION=STOP\n");
        }
        let deadline = Instant::now() + Duration::from_millis(1500);
        loop {
            let mut child = self.shared.child.lock().unwrap();
            match child.as_mut().map(|c| c.try_wait()) {
                Some(Ok(None)) if Instant::now() < deadline => {}
                Some(Ok(None)) | Some(Err(_)) => {
                    let _ = child.as_mut().map(|c| c.kill());
                    break;
                }
                _ => break,
            }
            drop(child);
            std::thread::sleep(Duration::from_millis(50));
        }
        // The session thread cleans up as the helper's output closes. Not joined: at quit it may
        // still be browsing mDNS (up to 10 s), and the process is ending anyway.
        let _ = UnixStream::connect(&self.socket); // wakes the accept loop
        drop(self.thread.lock().unwrap().take());
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(self.dir.join("cmd.fifo"));
        let _ = std::fs::remove_dir(&self.dir);
    }
}

fn session(cfg: Config, shared: Arc<Shared>) {
    let Some(helper_path) = cfg.helper.clone() else {
        log("cliairplay not found: build it with scripts/airplay-spike/build-cliairplay.sh or set UZUMAKI_CLIAIRPLAY");
        return;
    };
    let device = match &cfg.host {
        Some((ip, port)) => {
            log(format!("using {ip}:{port} from UZUMAKI_AIRPLAY_HOST (no mDNS)"));
            Device {
                name: cfg.target.clone(),
                ip: *ip,
                hostname: ip.to_string(),
                airplay: Some(Service { instance: cfg.target.clone(), port: *port, txt: Vec::new() }),
                raop: None,
            }
        }
        None => {
            log(format!("looking for \"{}\" on the network (mDNS, up to {} s)", cfg.target, cfg.discover_timeout.as_secs()));
            let devices = match discover::discover(cfg.discover_timeout, Some(&cfg.target)) {
                Ok(d) => d,
                Err(e) => return log(format!("discovery failed: {e}")),
            };
            match discover::pick(&devices, &cfg.target) {
                Some(d) => {
                    log(format!("found {}", discover::describe(d)));
                    d.clone()
                }
                None => {
                    log(format!("no speaker called \"{}\" found. Seen: {}", cfg.target, if devices.is_empty() { "none".into() } else { String::new() }));
                    for d in &devices {
                        log(format!("  {}", discover::describe(d)));
                    }
                    return;
                }
            }
        }
    };
    let mut attempt = 0;
    while !shared.quit.load(Ordering::SeqCst) {
        attempt += 1;
        match run_once(&cfg, &helper_path, &device, &shared) {
            Outcome::Stop => break,
            Outcome::Retry if attempt >= MAX_ATTEMPTS => {
                log(format!("giving up after {attempt} attempts"));
                break;
            }
            Outcome::Retry => {
                log(format!("trying again in {} s (attempt {}/{MAX_ATTEMPTS})", cfg.retry_delay.as_secs(), attempt + 1));
                let until = Instant::now() + cfg.retry_delay;
                while Instant::now() < until && !shared.quit.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    let _ = std::fs::remove_file(cfg.socket());
    let _ = std::fs::remove_file(cfg.fifo());
    let _ = std::fs::remove_dir(&cfg.dir);
}

enum Outcome {
    Retry,
    Stop,
}

enum Ev {
    Line(Status),
    CmdReady(std::fs::File),
    Closed,
}

fn run_once(cfg: &Config, helper_path: &Path, device: &Device, shared: &Arc<Shared>) -> Outcome {
    let fifo = cfg.fifo();
    let _ = std::fs::remove_file(&fifo);
    if let Err(e) = mkfifo(&fifo) {
        log(format!("command pipe {}: {e}", fifo.display()));
        return Outcome::Stop;
    }
    let args = helper::args(device, &fifo, &cfg.options);
    log(format!("starting {} {}", helper_path.display(), args.join(" ")));
    let mut child = match Command::new(helper_path).args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            log(format!("could not start cliairplay: {e}"));
            return Outcome::Stop;
        }
    };
    *shared.stdin.lock().unwrap() = child.stdin.take();
    let (tx, rx) = mpsc::channel::<Ev>();
    let readers = Arc::new(AtomicU64::new(2));
    let out = child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
    let err = child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
    for stream in [out, err].into_iter().flatten() {
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
    let mut buffered = false;
    let mut start_sent = false;
    let mut last_error: Option<String> = None;
    let mut started_at: Option<Instant> = None;
    let connect_started = Instant::now();
    loop {
        let ev = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(ev) => ev,
            // stop() ends the helper; its output then closes and `Closed` arrives.
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match ev {
            Ev::Line(Status::Route(r)) => log(format!("route: {r}")),
            Ev::Line(Status::Connected) => {
                log(format!("connected to \"{}\" in {} ms; opening the PCM socket for the bridge", device.name, connect_started.elapsed().as_millis()));
                match Pump::start(&cfg.socket(), shared.clone()) {
                    Ok(p) => pump = Some(p),
                    Err(e) => log(format!("PCM socket {}: {e}", cfg.socket().display())),
                }
                shared.casting.store(true, Ordering::SeqCst);
                open_cmd_pipe(fifo.clone(), tx.clone(), alive.clone());
            }
            Ev::CmdReady(f) => {
                *shared.cmd.lock().unwrap() = Some(f);
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
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                started_at = Some(Instant::now());
                log(format!(
                    "STARTED: the speaker plays from unix ms {at_ms} ({} ms from now; requested {requested_ms}). You should hear the song now.",
                    at_ms as i64 - now as i64
                ));
            }
            Ev::Line(Status::ClockReady { mode, state }) => {
                log(format!("clock {mode}: {state}"));
                if state == "stalled" {
                    log(helper::STALLED_HINT);
                }
            }
            Ev::Line(Status::Error { code, http, detail }) => {
                log(format!("ERROR {code} (http {http}): {detail}"));
                log(format!("  → {}", helper::explain(&code)));
                last_error = Some(code);
            }
            Ev::Line(Status::Eof) => log("helper input ended"),
            Ev::Line(Status::IdleTimeout) => log("helper idle timeout (no START within 120 s?)"),
            Ev::Line(Status::Stopped) => log("helper stopped"),
            Ev::Closed => break,
        }
    }
    alive.store(false, Ordering::SeqCst);
    shared.casting.store(false, Ordering::SeqCst);
    let status = shared.child.lock().unwrap().take().map(|mut c| c.wait());
    *shared.stdin.lock().unwrap() = None;
    *shared.cmd.lock().unwrap() = None;
    if let Some(p) = pump.take() {
        p.stop();
    }
    let _ = std::fs::remove_file(&fifo);
    log(format!(
        "cliairplay ended ({}){}",
        match status {
            Some(Ok(s)) => s.to_string(),
            Some(Err(e)) => e.to_string(),
            None => "?".into(),
        },
        started_at.map_or(String::new(), |t| format!(" after {} s of playback", t.elapsed().as_secs()))
    ));
    if shared.quit.load(Ordering::SeqCst) {
        return Outcome::Stop;
    }
    match last_error.as_deref() {
        Some("auth_required" | "auth_failed") => {
            log("not trying again: this needs a change on the speaker (see above)");
            Outcome::Stop
        }
        _ => Outcome::Retry,
    }
}

fn send_start(shared: &Shared) -> bool {
    let mut cmd = shared.cmd.lock().unwrap();
    let Some(f) = cmd.as_mut() else { return false };
    match f.write_all(b"START_UNIX_MS=0\nACTION=START\n").and_then(|_| f.flush()) {
        Ok(()) => {
            log("sent START (as soon as possible)");
            true
        }
        Err(e) => {
            log(format!("could not send START: {e}"));
            false
        }
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
                    std::thread::sleep(Duration::from_millis(100));
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
/// copies its whole frames into the helper's stdin, unless the local player is in charge.
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
                log("bridge connected to the PCM socket: audio is flowing to the speaker");
                let mut frames = Frames::default(); // a new connection starts on a frame
                let mut ok = true;
                let (mut this, mut next_report) = (0u64, BYTES_PER_SECOND * 30);
                while ok {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let whole = frames.whole(&buf[..n]);
                            if !whole.is_empty() && !shared.local_active.load(Ordering::SeqCst) {
                                ok = write_stdin(&shared.stdin, whole).is_ok();
                                this += whole.len() as u64;
                                if this >= next_report {
                                    log(format!("forwarded {} s of Apple Music audio", this / BYTES_PER_SECOND));
                                    next_report += BYTES_PER_SECOND * 30;
                                }
                            }
                            frames.consume();
                        }
                    }
                }
                *cur.lock().unwrap() = None;
                log(format!("bridge PCM stream ended after {:.1} s of audio{}", this as f64 / BYTES_PER_SECOND as f64, if ok { "" } else { " (helper input closed)" }));
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

fn write_stdin(stdin: &Mutex<Option<ChildStdin>>, bytes: &[u8]) -> std::io::Result<()> {
    match stdin.lock().unwrap().as_mut() {
        Some(s) => s.write_all(bytes),
        None => Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
    }
}

fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// How much silence the local pump writes now so the speaker's input runs `SILENCE_LEAD` ahead of
/// the clock, once the local audio has stopped for `LOCAL_GRACE` (else nothing). In frames.
pub fn silence_due(now: Instant, last_audio: Instant, audio_until: Instant) -> usize {
    if now.duration_since(last_audio) < LOCAL_GRACE {
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

/// The local player's chunks → the helper, while the local player is in charge and a speaker is
/// connected; silence when it has nothing (paused, loading, between songs). Lives with the spike.
fn local_pump(rx: mpsc::Receiver<Vec<i16>>, shared: Arc<Shared>) {
    let mut last_audio = Instant::now();
    let mut audio_until = Instant::now();
    let (mut sent, mut next_report) = (0u64, 44100u64 * 30);
    let mut was_on = false;
    while !shared.quit.load(Ordering::SeqCst) {
        let chunk = match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let on = shared.casting.load(Ordering::SeqCst) && shared.local_active.load(Ordering::SeqCst);
        if on != was_on {
            was_on = on;
            last_audio = Instant::now();
            audio_until = Instant::now();
        }
        if !on {
            continue; // nobody to send to, or the bridge is in charge: drop it
        }
        let now = Instant::now();
        let bytes = match chunk {
            Some(c) if c.len() >= 2 => {
                let c = &c[..c.len() - c.len() % 2]; // whole stereo frames
                last_audio = now;
                audio_until = audio_until.max(now) + Duration::from_secs_f64((c.len() / 2) as f64 / 44100.0);
                sent += (c.len() / 2) as u64;
                if sent >= next_report {
                    log(format!("forwarded {} s of local audio", sent / 44100));
                    next_report += 44100 * 30;
                }
                to_bytes(c)
            }
            _ => match silence_due(now, last_audio, audio_until) {
                0 => continue,
                frames => {
                    audio_until = audio_until.max(now) + Duration::from_secs_f64(frames as f64 / 44100.0);
                    vec![0u8; frames * FRAME_BYTES as usize]
                }
            },
        };
        let _ = write_stdin(&shared.stdin, &bytes);
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
}
