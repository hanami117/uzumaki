//! "Show on Discord" (Settings → General): Discord Rich Presence, the "Listening to …" card
//! Spotify shows on your profile. See docs/DISCORD.md for how to create the Discord Application
//! this needs.
//!
//! How it works: the Discord desktop app listens on a local socket (`ipc.rs`); we say hello with
//! the Application's id and send it an activity (`activity.rs`). Uzumaki makes no network
//! connection for this; Discord itself fetches the cover from Apple when it draws the card.
//!
//! Threads: none unless the setting is on (and an Application id is known). Then exactly one
//! worker, which owns the socket. `Presence::update` (called on the UI thread after every
//! playback message) only builds a small value and compares it to the last one; a change is
//! handed to the worker over a channel. The worker connects (every `RECONNECT` while Discord
//! isn't there, quietly: one log line per outage), keeps only the newest activity while it waits
//! out `MIN_GAP` between sends (Discord allows 5 updates per 20 s), answers Discord's pings, and
//! clears the card when told to stop. A dropped connection is taken up again by itself.
//!
//! Off for headless runs, tests and test instances (`Presence::off`); tests of this module give
//! the worker a fake server's socket path, never the real Discord's.

mod activity;
mod ipc;

#[cfg(windows)]
mod pipe;
// The tests play Discord on a Unix socket (Windows: a pipe, in `pipe.rs`).
#[cfg(all(test, unix))]
mod tests;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, Once, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use activity::{Activity, Snapshot};
use ipc::Conn;

/// The Discord Application id (docs/DISCORD.md: create it at discord.com/developers).
/// TODO(thomas): replace this placeholder with the real Application ID. While it is all zeros
/// the setting does nothing unless `discord.client_id` in settings.json gives another id.
pub const CLIENT_ID: &str = "1572355156471717929";

/// While Discord isn't there (or the connection dropped), try again this often.
const RECONNECT: Duration = Duration::from_secs(30);
/// At most one update this often (Discord drops more than 5 per 20 s).
const MIN_GAP: Duration = Duration::from_secs(2);
/// How often an idle connection is checked for Discord's pings and for it going away.
const IDLE_POLL: Duration = Duration::from_secs(5);
/// A connect or a write that takes longer than this has failed.
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// How long quitting waits for the card to be cleared.
const QUIT_WAIT: Duration = Duration::from_millis(1000);

/// The user's choices (settings.json, `discord`). Off by default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// "Show on Discord".
    pub enabled: bool,
    /// Show the cover (Discord fetches it from Apple). Off: the Uzumaki icon.
    pub artwork: bool,
    /// The "Open in Apple Music" button.
    pub buttons: bool,
    /// Clear the card while paused (otherwise it stays, without the progress bar).
    pub hide_when_paused: bool,
    /// Show "Listening to music" instead of the title (no album, no button).
    pub hide_title: bool,
    /// Another Discord Application id than the app's own (docs/DISCORD.md), for builds of your own.
    pub client_id: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { enabled: false, artwork: true, buttons: true, hide_when_paused: true, hide_title: false, client_id: None }
    }
}

fn is_id(s: &str) -> bool {
    (15..=25).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_placeholder(s: &str) -> bool {
    s.bytes().all(|b| b == b'0')
}

impl Prefs {
    /// The Application id to use: the configured one when it looks like an id, else the app's.
    /// None while that is still the placeholder.
    pub fn effective_client_id(&self) -> Option<String> {
        let own = self.client_id.as_deref().map(str::trim).filter(|s| is_id(s) && !is_placeholder(s));
        own.or((!is_placeholder(CLIENT_ID)).then_some(CLIENT_ID)).map(String::from)
    }

    /// Whether this build can show anything at all (it has an Application id).
    pub fn available(&self) -> bool {
        self.effective_client_id().is_some()
    }
}

/// How the worker behaves: where the sockets are and how long it waits. Tests give their own.
#[derive(Clone)]
struct Env {
    paths: Vec<PathBuf>,
    reconnect: Duration,
    min_gap: Duration,
    idle_poll: Duration,
    io_timeout: Duration,
    /// Clear the card from an `atexit` handler too (the real app only).
    at_exit: bool,
}

impl Env {
    fn system() -> Self {
        Self {
            paths: ipc::candidates(&|k| std::env::var_os(k)),
            reconnect: RECONNECT,
            min_gap: MIN_GAP,
            idle_poll: IDLE_POLL,
            io_timeout: IO_TIMEOUT,
            at_exit: true,
        }
    }
}

enum Cmd {
    /// Show this (None: clear the card).
    Set(Option<Activity>),
    /// Clear the card, hang up, end. The sender is told when that is done.
    Stop(Option<Sender<()>>),
}

struct Worker {
    tx: Sender<Cmd>,
    client_id: String,
}

/// Owned by `State`.
pub struct Presence {
    env: Option<Env>,
    prefs: Prefs,
    worker: Option<Worker>,
    /// What the worker was last given.
    last: Option<Activity>,
    /// A worker that was told to stop: the next one waits for it, so its clearing can't wipe
    /// the new card.
    retiring: Option<Receiver<()>>,
}

impl Presence {
    /// Never shows anything (headless runs, tests, test instances).
    pub fn off() -> Self {
        Self { env: None, prefs: Prefs::default(), worker: None, last: None, retiring: None }
    }

    /// The real one, unless this is headless or a test instance.
    pub fn for_app() -> Self {
        let test = std::env::var_os("UZUMAKI_TEST_INSTANCE").is_some_and(|v| !v.is_empty());
        if test || rune::is_headless() {
            return Self::off();
        }
        Self::with_env(Env::system())
    }

    fn with_env(env: Env) -> Self {
        Self { env: Some(env), ..Self::off() }
    }

    /// A worker is running: worth building snapshots for.
    pub fn active(&self) -> bool {
        self.worker.is_some()
    }

    /// Applies the user's choices: starts the worker when the setting comes on, stops it (after
    /// it clears the card) when it goes off or the id changes. Never waits.
    pub fn configure(&mut self, prefs: &Prefs) {
        self.prefs = prefs.clone();
        self.last = None; // the next update tells the worker again
        let want = if self.env.is_some() && prefs.enabled { prefs.effective_client_id() } else { None };
        match (&self.worker, want) {
            (Some(w), Some(id)) if w.client_id == id => {}
            (Some(_), want) => {
                self.stop();
                if let Some(id) = want {
                    self.start(id);
                }
            }
            (None, Some(id)) => self.start(id),
            (None, None) => {}
        }
    }

    fn start(&mut self, client_id: String) {
        let Some(env) = self.env.clone() else { return };
        let (tx, rx) = channel();
        let after = self.retiring.take();
        let id = client_id.clone();
        let spawned = thread::Builder::new().name("discord".into()).spawn({
            let env = env.clone();
            move || run(rx, env, id, after)
        });
        match spawned {
            Ok(_) => {
                if env.at_exit {
                    register_exit(&tx);
                }
                self.worker = Some(Worker { tx, client_id });
            }
            Err(e) => eprintln!("uzumaki: Discord: couldn't start its thread: {e}"),
        }
    }

    fn stop(&mut self) {
        if let Some(w) = self.worker.take() {
            let (ack, done) = channel();
            let _ = w.tx.send(Cmd::Stop(Some(ack)));
            self.retiring = Some(done);
            if self.env.as_ref().is_some_and(|e| e.at_exit) {
                forget_exit();
            }
        }
        self.last = None;
    }

    /// Stops and waits (up to `wait`) for the card to be cleared.
    #[cfg(all(test, unix))]
    fn close(&mut self, wait: Duration) {
        self.stop();
        if let Some(done) = self.retiring.take() {
            let _ = done.recv_timeout(wait);
        }
    }

    /// What is playing now (None: nothing is, or it is stopped). Cheap: builds the activity and
    /// compares it to the last one; only a change reaches the worker. Seeks and pauses show up
    /// as changed timestamps or as no activity.
    pub fn update(&mut self, snap: Option<&Snapshot>) {
        self.update_at(snap, unix_now());
    }

    fn update_at(&mut self, snap: Option<&Snapshot>, now: u64) {
        let Some(w) = &self.worker else { return };
        let act = activity::build(snap, &self.prefs, now);
        let same = match (&act, &self.last) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_as(b),
            _ => false,
        };
        if same {
            return;
        }
        self.last = act.clone();
        if w.tx.send(Cmd::Set(act)).is_err() {
            // The worker is gone (it can only panic); nothing more to do until reconfigured.
            self.worker = None;
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------- quitting

/// The real app's worker, for `shutdown`.
static EXIT: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);

extern "C" fn at_exit() {
    shutdown();
}

fn register_exit(tx: &Sender<Cmd>) {
    static HOOK: Once = Once::new();
    *EXIT.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx.clone());
    HOOK.call_once(|| {
        // SAFETY: `at_exit` is a plain `extern "C" fn()` that lives for the whole process.
        if unsafe { libc::atexit(at_exit) } != 0 {
            eprintln!("uzumaki: Discord: couldn't arrange clearing the card at quit");
        }
    });
}

fn forget_exit() {
    *EXIT.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Quitting: clears the card and waits a moment for that. Called at the end of `main` and from
/// an `atexit` handler (every way of quitting reaches one of them); the second call finds
/// nothing to do.
pub fn shutdown() {
    let tx = EXIT.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(tx) = tx {
        let (ack, done) = channel();
        if tx.send(Cmd::Stop(Some(ack))).is_ok() {
            let _ = done.recv_timeout(QUIT_WAIT);
        }
    }
}

// ---------------------------------------------------------------- the worker

fn log(msg: impl std::fmt::Display) {
    eprintln!("uzumaki: Discord: {msg}");
}

/// Hangs up politely: the card goes first, if there is one.
fn finish(conn: &mut Option<Conn>, shown: &Option<Activity>) {
    if let (Some(c), Some(_)) = (conn.as_mut(), shown) {
        let _ = c.set_activity(None);
    }
}

fn run(rx: Receiver<Cmd>, env: Env, client_id: String, after: Option<Receiver<()>>) {
    if let Some(prev) = after {
        let _ = prev.recv_timeout(Duration::from_secs(2));
    }
    let mut conn: Option<Conn> = None;
    // What we want Discord to show, and what it has (None: nothing).
    let mut desired: Option<Activity> = None;
    let mut shown: Option<Activity> = None;
    let mut next_try = Instant::now();
    let mut last_send: Option<Instant> = None;
    let mut outage_logged = false;
    loop {
        if desired.is_some() && conn.is_none() && Instant::now() >= next_try {
            match Conn::connect(&env.paths, &client_id, env.io_timeout) {
                Ok(c) => {
                    log(if outage_logged { "connected again" } else { "connected" });
                    outage_logged = false;
                    conn = Some(c);
                }
                Err(e) => {
                    if !outage_logged {
                        log(format_args!("{} (trying again every {} s)", ipc::describe(&e), env.reconnect.as_secs().max(1)));
                        outage_logged = true;
                    }
                    next_try = Instant::now() + env.reconnect;
                }
            }
        }
        if conn.is_none() {
            shown = None; // Discord drops the card with the connection
        }
        let gap = |t: Option<Instant>| t.map_or(Duration::ZERO, |t| env.min_gap.saturating_sub(t.elapsed()));
        if let Some(c) = conn.as_mut().filter(|_| desired != shown && gap(last_send).is_zero()) {
            match c.set_activity(desired.as_ref().map(Activity::to_json).as_ref()) {
                Ok(refused) => {
                    if let Some(msg) = refused {
                        log(format_args!("Discord didn't take the activity: {msg}"));
                    }
                    shown = desired.clone();
                    last_send = Some(Instant::now());
                }
                Err(e) => {
                    log(format_args!("lost the connection ({e})"));
                    conn = None;
                    shown = None;
                    outage_logged = true;
                    next_try = Instant::now() + env.reconnect;
                }
            }
        }
        let wait = match (&conn, desired != shown) {
            (Some(_), true) => gap(last_send),
            (Some(_), false) => env.idle_poll,
            (None, _) if desired.is_some() => next_try.saturating_duration_since(Instant::now()),
            (None, _) => Duration::from_secs(3600),
        };
        match rx.recv_timeout(wait) {
            Ok(Cmd::Set(a)) => desired = a,
            Ok(Cmd::Stop(ack)) => {
                finish(&mut conn, &shown);
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(Err(e)) = conn.as_mut().map(Conn::poll) {
                    log(format_args!("lost the connection ({e})"));
                    conn = None;
                    shown = None;
                    outage_logged = true;
                    next_try = Instant::now() + env.reconnect;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                finish(&mut conn, &shown);
                return;
            }
        }
    }
}
