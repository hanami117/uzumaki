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
}

#[derive(Default)]
struct Shared {
    quit: AtomicBool,
    child: Mutex<Option<Child>>,
    cmd: Mutex<Option<std::fs::File>>,
}

static SPIKE: OnceLock<Option<Spike>> = OnceLock::new();

/// The environment the bridge needs, starting the spike on first use (once per process). Empty
/// unless `UZUMAKI_AIRPLAY_SPIKE=1`, so normal runs are unchanged.
pub fn bridge_env() -> Vec<(&'static str, OsString)> {
    if !enabled() {
        return Vec::new();
    }
    let spike = SPIKE.get_or_init(|| {
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
    });
    match spike {
        Some(s) => vec![("UZUMAKI_AIRPLAY_PCM_SOCKET", s.socket.clone().into_os_string())],
        None => Vec::new(),
    }
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
        let shared = Arc::new(Shared::default());
        let s2 = shared.clone();
        let cfg_dir = cfg.dir.clone();
        let thread = std::thread::Builder::new().name("airplay".into()).spawn(move || session(cfg, s2)).map_err(|e| e.to_string())?;
        Ok(Spike { socket, dir: cfg_dir, shared, thread: Mutex::new(Some(thread)) })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn stop(&self) {
        if self.shared.quit.swap(true, Ordering::SeqCst) {
            return;
        }
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
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(child.stdin.take()));
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
                match Pump::start(&cfg.socket(), stdin.clone()) {
                    Ok(p) => pump = Some(p),
                    Err(e) => log(format!("PCM socket {}: {e}", cfg.socket().display())),
                }
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
    let status = shared.child.lock().unwrap().take().map(|mut c| c.wait());
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

/// Zero bytes to send before a new connection's audio so the helper's input stays frame-aligned
/// (a bridge that dropped mid-chunk left `total` bytes that aren't whole frames).
pub fn realign(total: u64) -> usize {
    ((FRAME_BYTES - total % FRAME_BYTES) % FRAME_BYTES) as usize
}

/// The PCM socket: accepts the bridge (one connection at a time, again after it reconnects) and
/// copies its bytes into the helper's stdin.
struct Pump {
    path: PathBuf,
    quit: Arc<AtomicBool>,
    current: Arc<Mutex<Option<UnixStream>>>,
    thread: std::thread::JoinHandle<()>,
}

impl Pump {
    fn start(path: &Path, stdin: Arc<Mutex<Option<ChildStdin>>>) -> std::io::Result<Self> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        let quit = Arc::new(AtomicBool::new(false));
        let current: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
        let (q, cur) = (quit.clone(), current.clone());
        let thread = std::thread::Builder::new().name("airplay-pcm".into()).spawn(move || {
            let mut total: u64 = 0;
            let mut buf = vec![0u8; 16384];
            for conn in listener.incoming() {
                if q.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut s) = conn else { continue };
                *cur.lock().unwrap() = s.try_clone().ok();
                log("bridge connected to the PCM socket: audio is flowing to the speaker");
                let pad = realign(total);
                let mut ok = pad == 0 || write_stdin(&stdin, &vec![0u8; pad]).is_ok();
                total += pad as u64;
                let (mut this, mut next_report) = (0u64, BYTES_PER_SECOND * 30);
                while ok {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            ok = write_stdin(&stdin, &buf[..n]).is_ok();
                            total += n as u64;
                            this += n as u64;
                            if this >= next_report {
                                log(format!("forwarded {} s of audio", this / BYTES_PER_SECOND));
                                next_report += BYTES_PER_SECOND * 30;
                            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realigns_to_whole_frames() {
        assert_eq!(realign(0), 0);
        assert_eq!(realign(16384), 0);
        assert_eq!(realign(16385), 3);
        assert_eq!(realign(16387), 1);
    }
}
