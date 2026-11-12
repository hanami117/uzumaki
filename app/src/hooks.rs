//! Script hooks: executables the user drops into `$XDG_CONFIG_HOME/uzumaki/hooks/` (else
//! `~/.config/uzumaki/hooks/`, also on macOS), run by the app itself, so they work with the
//! window and with `uzu` alike. Git-hooks style, no config file:
//!
//! - `on-song-change`: a new song starts (a click, a skip, the queue moving on);
//! - `on-state-change`: playback goes to playing, paused or stopped.
//!
//! No shell: the file is run directly and gets its data only as `UZUMAKI_*` environment
//! variables (see `Event::new`). stdin is null; stdout and stderr go to the app's own stderr (the
//! log). The hook runs in its own process group, and a hook still running after 10 s gets SIGTERM
//! for the group, then SIGKILL. A hook that is writable by group or others, or owned by someone
//! else, is not run (the reason is logged once). At most one run per hook at a time; while one
//! runs, only the newest pending event is kept, so skipping through songs can't queue a storm.
//!
//! Nothing here blocks the UI thread: `Hooks::observe` only compares and hands an event to a
//! worker thread, which does every file check, the cover wait and the run. Off in test instances
//! (`UZUMAKI_TEST_INSTANCE`), so tests never run the user's scripts.
//!
//! Idea from rmpc's `on_song_change` (BSD-3); no code taken.

use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use std::{fs, io, thread};

use crate::bridge::PlayState;
use crate::models::{Artwork, Song};

const SONG_CHANGE: &str = "on-song-change";
const STATE_CHANGE: &str = "on-state-change";

/// How long a hook may run before it is stopped.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How long a hook gets after SIGTERM before SIGKILL.
const GRACE: Duration = Duration::from_secs(2);
/// How long to wait for the cover to land in the art cache.
const COVER_WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy)]
struct Timing {
    timeout: Duration,
    grace: Duration,
    cover_wait: Duration,
}

const TIMING: Timing = Timing { timeout: TIMEOUT, grace: GRACE, cover_wait: COVER_WAIT };

/// One thing to tell a hook.
#[derive(Clone, Debug, PartialEq)]
struct Event {
    vars: Vec<(&'static str, String)>,
    /// Resolved to `UZUMAKI_COVER` on the worker thread, once the hook is known to run.
    cover: Option<Artwork>,
}

impl Event {
    /// `event` is "song-change" or "state-change"; `state` one of playing, paused, stopped,
    /// loading. Every variable is always set (empty when unknown), so none leaks in from the
    /// app's own environment.
    fn new(event: &str, state: &str, song: Option<&Song>, prev_id: &str) -> Self {
        let text = |f: fn(&Song) -> String| song.map(f).unwrap_or_default();
        let vars = vec![
            ("UZUMAKI_EVENT", event.to_string()),
            ("UZUMAKI_STATE", state.to_string()),
            ("UZUMAKI_TITLE", text(|s| s.title.clone())),
            ("UZUMAKI_ARTIST", text(|s| s.artist.clone())),
            ("UZUMAKI_ALBUM", text(|s| s.album.clone())),
            // Whole seconds.
            ("UZUMAKI_DURATION", song.map(|s| format!("{}", s.duration.round() as u64)).unwrap_or_default()),
            ("UZUMAKI_ID", text(|s| s.id.clone())),
            ("UZUMAKI_LOCAL", song.map(|s| if s.is_local() { "1" } else { "0" }.to_string()).unwrap_or_default()),
            ("UZUMAKI_FILE", text(|s| s.local_path().map(|p| p.display().to_string()).unwrap_or_default())),
            ("UZUMAKI_SOURCE", song.map(|s| if s.is_local() { "local" } else { "apple" }.to_string()).unwrap_or_default()),
            ("UZUMAKI_PREV_ID", prev_id.to_string()),
        ];
        Self { vars, cover: song.and_then(|s| s.artwork.clone()) }
    }
}

/// The coalescing rule: one run at a time, newest pending event only.
#[derive(Default)]
struct Slot {
    running: bool,
    pending: Option<Event>,
    /// The last refusal logged, so each reason is logged once.
    warned: Option<String>,
}

impl Slot {
    /// Returns the event to run now, or keeps it as the one pending (dropping an older one).
    fn submit(&mut self, ev: Event) -> Option<Event> {
        if self.running {
            self.pending = Some(ev);
            None
        } else {
            self.running = true;
            Some(ev)
        }
    }

    /// A run ended: the pending event to run next, or None (and idle again).
    fn finish(&mut self) -> Option<Event> {
        let next = self.pending.take();
        self.running = next.is_some();
        next
    }
}

fn locked(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the file check found.
#[derive(Debug, PartialEq)]
enum Check {
    Missing,
    Ok,
    Refused(String),
}

/// A hook runs only if it is a regular, executable file, owned by `uid` and not writable by
/// group or others (otherwise anyone who can write it gets to run code as the user). Follows
/// symlinks, so the target is what is judged.
fn check(path: &Path, uid: u32) -> Check {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Check::Missing,
        Err(e) => return Check::Refused(format!("can't read it: {e}")),
    };
    let mode = meta.mode();
    if !meta.is_file() {
        Check::Refused("not a regular file".into())
    } else if meta.uid() != uid {
        Check::Refused("owned by another user".into())
    } else if mode & 0o022 != 0 {
        Check::Refused(format!("writable by group or others (mode {:o}); chmod go-w it", mode & 0o7777))
    } else if mode & 0o100 == 0 {
        Check::Refused("not executable; chmod +x it".into())
    } else {
        Check::Ok
    }
}

/// One hook file and its worker state.
#[derive(Clone)]
struct Runner {
    path: PathBuf,
    slot: Arc<Mutex<Slot>>,
    timing: Timing,
}

impl Runner {
    fn new(path: PathBuf, timing: Timing) -> Self {
        Self { path, slot: Arc::default(), timing }
    }

    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Hands the event to the worker thread (starting one if none is running).
    fn submit(&self, ev: Event) {
        let Some(first) = locked(&self.slot).submit(ev) else { return };
        let this = self.clone();
        let spawned = thread::Builder::new().name("hook".into()).spawn(move || {
            let mut ev = first;
            loop {
                this.run_one(ev);
                match locked(&this.slot).finish() {
                    Some(next) => ev = next,
                    None => break,
                }
            }
        });
        if spawned.is_err() {
            locked(&self.slot).finish();
        }
    }

    fn run_one(&self, ev: Event) {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        match check(&self.path, uid) {
            Check::Missing => return,
            Check::Refused(why) => {
                let mut slot = locked(&self.slot);
                if slot.warned.as_deref() != Some(&why) {
                    eprintln!("uzumaki: hook {} skipped: {why}", self.path.display());
                    slot.warned = Some(why);
                }
                return;
            }
            Check::Ok => locked(&self.slot).warned = None,
        }
        let mut vars = ev.vars;
        let mut cover = String::new();
        if let Some(art) = &ev.cover {
            match self.wait_for_cover(art) {
                Some(p) => cover = p.display().to_string(),
                None if locked(&self.slot).pending.is_some() => return, // overtaken by a newer event
                None => {}
            }
        }
        vars.push(("UZUMAKI_COVER", cover));
        self.execute(&vars);
    }

    /// Polls the art cache for the cover, up to `cover_wait`; gives up early when a newer event
    /// is waiting (this one is then dropped by the caller).
    fn wait_for_cover(&self, art: &Artwork) -> Option<PathBuf> {
        let start = Instant::now();
        loop {
            if let Some(p) = crate::art::cached_file(art) {
                return Some(p);
            }
            if start.elapsed() >= self.timing.cover_wait || locked(&self.slot).pending.is_some() {
                return None;
            }
            thread::sleep(POLL);
        }
    }

    fn execute(&self, vars: &[(&'static str, String)]) {
        let name = self.name();
        // stdout and stderr both go to our stderr, which is the log file when opened from the Finder.
        let log = || {
            io::stderr().as_fd().try_clone_to_owned().map(Stdio::from).unwrap_or_else(|_| Stdio::null())
        };
        let first_try = Instant::now();
        let mut child = loop {
            let mut cmd = Command::new(&self.path);
            cmd.envs(vars.iter().map(|(k, v)| (*k, v))).stdin(Stdio::null()).stdout(log()).stderr(log()).process_group(0);
            match cmd.spawn() {
                Ok(c) => break c,
                // "Text file busy" (Linux): another thread's fork holds a just-written script open
                // until it execs, which can take a while on a busy machine.
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && first_try.elapsed() < Duration::from_secs(2) => {
                    thread::sleep(Duration::from_millis(25))
                }
                Err(e) => {
                    eprintln!("uzumaki: hook {name}: could not start: {e}");
                    return;
                }
            }
        };
        let pid = child.id() as libc::pid_t;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        eprintln!("uzumaki: hook {name}: {status}");
                    }
                    return;
                }
                Ok(None) => {}
                Err(e) => return eprintln!("uzumaki: hook {name}: {e}"),
            }
            if start.elapsed() >= self.timing.timeout {
                break;
            }
            thread::sleep(POLL.min(Duration::from_millis(10)));
        }
        eprintln!("uzumaki: hook {name}: still running after {:?}, stopping it", self.timing.timeout);
        // SAFETY: killpg with our own child's group (process_group(0) made it the leader).
        unsafe { libc::killpg(pid, libc::SIGTERM) };
        let grace = Instant::now();
        while grace.elapsed() < self.timing.grace {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // SAFETY: as above.
        unsafe { libc::killpg(pid, libc::SIGKILL) };
        let _ = child.wait();
    }
}

/// The two hooks and what they last heard, owned by `State`.
pub struct Hooks {
    song: Option<Runner>,
    state: Option<Runner>,
    last_song: Option<Song>,
    /// "playing", "paused" or "stopped"; None until the first one is seen.
    last_state: Option<&'static str>,
}

impl Hooks {
    /// No hooks at all (headless runs, tests, test instances).
    pub fn off() -> Self {
        Self { song: None, state: None, last_song: None, last_state: None }
    }

    /// The hooks of the real app: in the config folder, unless this is a test instance.
    pub fn for_app() -> Self {
        let test = std::env::var_os("UZUMAKI_TEST_INSTANCE").is_some_and(|v| !v.is_empty());
        match crate::settings::config_dir() {
            Some(dir) if !test => Self::in_dir(&dir.join("hooks"), TIMING),
            _ => Self::off(),
        }
    }

    fn in_dir(dir: &Path, timing: Timing) -> Self {
        let runner = |n: &str| Some(Runner::new(dir.join(n), timing));
        Self { song: runner(SONG_CHANGE), state: runner(STATE_CHANGE), last_song: None, last_state: None }
    }

    /// Called after anything that may have changed playback. Cheap when nothing changed.
    /// Returns true when a new song began (the same detector feeds the notifications).
    pub fn observe(&mut self, song: Option<&Song>, state: PlayState) -> bool {
        if self.song.is_none() && self.state.is_none() {
            return false;
        }
        let mut song_changed = false;
        if let Some(s) = song {
            let new = self.last_song.as_ref().is_none_or(|last| last.id != s.id && !last.same_track(s));
            if new {
                let prev = self.last_song.as_ref().map(|l| l.id.clone()).unwrap_or_default();
                self.last_song = Some(s.clone());
                song_changed = true;
                if let Some(r) = &self.song {
                    r.submit(Event::new("song-change", state_name(state), song, &prev));
                }
            }
        }
        if let Some(now) = stable_state(state) {
            let first = self.last_state.is_none();
            if self.last_state != Some(now) {
                self.last_state = Some(now);
                // The app starting up and reporting "stopped" isn't news.
                if !(first && now == "stopped") {
                    if let Some(r) = &self.state {
                        r.submit(Event::new("state-change", now, song, ""));
                    }
                }
            }
        }
        song_changed
    }
}

/// For `UZUMAKI_STATE`.
fn state_name(s: PlayState) -> &'static str {
    match s {
        PlayState::Playing => "playing",
        PlayState::Paused => "paused",
        PlayState::Loading | PlayState::Waiting | PlayState::Stalled | PlayState::Seeking => "loading",
        PlayState::None | PlayState::Stopped | PlayState::Ended | PlayState::Completed => "stopped",
    }
}

/// The states `on-state-change` reports; the in-between ones (loading, seeking, stalled) and
/// "nothing yet" are not changes.
fn stable_state(s: PlayState) -> Option<&'static str> {
    match s {
        PlayState::Playing => Some("playing"),
        PlayState::Paused => Some("paused"),
        PlayState::Stopped | PlayState::Ended | PlayState::Completed => Some("stopped"),
        PlayState::None | PlayState::Loading | PlayState::Waiting | PlayState::Stalled | PlayState::Seeking => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("uzumaki-hooks-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: format!("Title {id}"),
            artist: "Some Artist".into(),
            album: "Some Album".into(),
            duration: 215.4,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: vec![],
        }
    }

    fn fast() -> Timing {
        Timing { timeout: Duration::from_secs(10), grace: Duration::from_secs(2), cover_wait: Duration::from_millis(600) }
    }

    /// Waits until `f` gives Some.
    fn eventually<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < end, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn ev(n: u32) -> Event {
        Event { vars: vec![("N", n.to_string())], cover: None }
    }

    #[test]
    fn a_hook_gets_its_data_as_environment_variables() {
        let dir = temp("env");
        let out = dir.join("out");
        script(&dir, SONG_CHANGE, &format!("env | grep '^UZUMAKI_' | sort > '{0}.tmp'; mv '{0}.tmp' '{0}'", out.display()));
        let mut hooks = Hooks::in_dir(&dir, fast());
        hooks.observe(Some(&song("100")), PlayState::Loading);
        let text = eventually("the hook's output", || fs::read_to_string(&out).ok());
        for want in [
            "UZUMAKI_EVENT=song-change",
            "UZUMAKI_STATE=loading",
            "UZUMAKI_TITLE=Title 100",
            "UZUMAKI_ARTIST=Some Artist",
            "UZUMAKI_ALBUM=Some Album",
            "UZUMAKI_DURATION=215",
            "UZUMAKI_ID=100",
            "UZUMAKI_LOCAL=0",
            "UZUMAKI_FILE=",
            "UZUMAKI_COVER=",
            "UZUMAKI_SOURCE=apple",
            "UZUMAKI_PREV_ID=",
        ] {
            assert!(text.lines().any(|l| l == want), "missing {want} in:\n{text}");
        }
        // The next song names the one before it, and a local file shows its path.
        fs::remove_file(&out).unwrap();
        let local = song("file:///music/a b.flac");
        hooks.observe(Some(&local), PlayState::Playing);
        let text = eventually("the second run", || fs::read_to_string(&out).ok());
        for want in ["UZUMAKI_PREV_ID=100", "UZUMAKI_LOCAL=1", "UZUMAKI_FILE=/music/a b.flac", "UZUMAKI_SOURCE=local", "UZUMAKI_STATE=playing"] {
            assert!(text.lines().any(|l| l == want), "missing {want} in:\n{text}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_hook_gets_no_shell_so_odd_titles_stay_data() {
        let dir = temp("noshell");
        let out = dir.join("out");
        script(&dir, SONG_CHANGE, &format!("printf '%s' \"$UZUMAKI_TITLE\" > '{0}.tmp'; mv '{0}.tmp' '{0}'", out.display()));
        let mut hooks = Hooks::in_dir(&dir, fast());
        let mut s = song("1");
        s.title = "$(touch /tmp/pwned); `x` \"q\" ; rm -rf".into();
        hooks.observe(Some(&s), PlayState::Playing);
        assert_eq!(eventually("output", || fs::read_to_string(&out).ok()), s.title);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn song_change_fires_per_new_song_and_state_change_per_new_state() {
        let dir = temp("events");
        let log = dir.join("log");
        let body = format!("echo \"$UZUMAKI_EVENT $UZUMAKI_STATE $UZUMAKI_ID\" >> '{}'", log.display());
        script(&dir, SONG_CHANGE, &body);
        script(&dir, STATE_CHANGE, &body);
        let mut hooks = Hooks::in_dir(&dir, fast());
        let a = song("a");
        hooks.observe(None, PlayState::None); // nothing yet
        hooks.observe(Some(&a), PlayState::Loading); // song change only: loading isn't a state change
        hooks.observe(Some(&a), PlayState::Playing);
        hooks.observe(Some(&a), PlayState::Playing); // nothing new
        // One state at a time: a state hook still running would coalesce the next two.
        let state_ran = |hooks: &Hooks, what: &str| {
            let want = format!("state-change {what} a");
            eventually(&want.clone(), || fs::read_to_string(&log).ok().filter(|t| t.lines().any(|l| l == want)).map(|_| ()));
            eventually("the state hook going idle", || (!locked(&hooks.state.as_ref().unwrap().slot).running).then_some(()));
        };
        state_ran(&hooks, "playing");
        hooks.observe(Some(&a), PlayState::Paused);
        state_ran(&hooks, "paused");
        hooks.observe(Some(&Song { catalog_id: Some("a".into()), id: "i.a".into(), ..a.clone() }), PlayState::Paused); // same track, other id
        hooks.observe(Some(&a), PlayState::Stopped);
        let text = eventually("all four events", || fs::read_to_string(&log).ok().filter(|t| t.lines().count() >= 4));
        let mut lines: Vec<&str> = text.lines().collect();
        lines.sort();
        assert_eq!(lines, ["song-change loading a", "state-change paused a", "state-change playing a", "state-change stopped a"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_stopped_and_a_missing_hook_are_quiet() {
        let dir = temp("quiet");
        let log = dir.join("log");
        script(&dir, STATE_CHANGE, &format!("echo \"$UZUMAKI_STATE\" >> '{}'", log.display()));
        let mut hooks = Hooks::in_dir(&dir, fast()); // no on-song-change file: nothing to do, no failure
        hooks.observe(Some(&song("a")), PlayState::Stopped); // the first report, not a change
        hooks.observe(Some(&song("a")), PlayState::Playing);
        assert_eq!(eventually("playing", || fs::read_to_string(&log).ok()), "playing\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hook_that_hangs_is_killed_by_the_timeout() {
        let dir = temp("timeout");
        let pidfile = dir.join("pid");
        // The sleep is a child of the script: the group kill has to reach it too.
        // The timeout is long enough for the shell to start and write the pid even on a loaded
        // machine (it was 300 ms, which a busy test run could spend before `echo` ran), and far
        // below the sleep, so finishing early can only mean the kill.
        script(&dir, SONG_CHANGE, &format!("sleep 300 & echo $! > '{}'; wait", pidfile.display()));
        let timing = Timing { timeout: Duration::from_secs(2), grace: Duration::from_secs(2), cover_wait: Duration::ZERO };
        let runner = Runner::new(dir.join(SONG_CHANGE), timing);
        let start = Instant::now();
        runner.submit(ev(1));
        let pid: i32 = eventually("the pid", || fs::read_to_string(&pidfile).ok().and_then(|t| t.trim().parse().ok()));
        eventually("the runner going idle", || (!locked(&runner.slot).running).then_some(()));
        let took = start.elapsed();
        assert!(took >= Duration::from_secs(2) && took < Duration::from_secs(60), "took {took:?}");
        // The sleep is gone too (signal 0 only checks that the process exists).
        eventually("the sleep to die", || (unsafe { libc::kill(pid, 0) } != 0).then_some(()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hook_that_ignores_sigterm_gets_sigkill() {
        let dir = temp("sigkill");
        // The hook says when its trap is set. The timeout leaves the shell time to get there
        // even on a loaded machine (at 200 ms a busy run sometimes sent SIGTERM before the trap,
        // the shell died of it and the grace was never used).
        let ready = dir.join("ready");
        script(&dir, SONG_CHANGE, &format!("trap '' TERM\ntouch '{}'\nwhile true; do sleep 1; done", ready.display()));
        let timing = Timing { timeout: Duration::from_secs(2), grace: Duration::from_millis(500), cover_wait: Duration::ZERO };
        let runner = Runner::new(dir.join(SONG_CHANGE), timing);
        let start = Instant::now();
        runner.submit(ev(1));
        eventually("the trap to be set", || ready.exists().then_some(()));
        assert!(start.elapsed() < timing.timeout, "the shell took {:?} to set its trap: the test proves nothing", start.elapsed());
        eventually("the runner going idle", || (!locked(&runner.slot).running).then_some(()));
        let took = start.elapsed();
        // Only SIGKILL ends it, after the timeout and the whole grace.
        assert!(took >= timing.timeout + timing.grace && took < Duration::from_secs(60), "took {took:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_slot_keeps_only_the_newest_pending_event() {
        let mut slot = Slot::default();
        assert_eq!(slot.submit(ev(1)), Some(ev(1)), "idle: runs at once");
        assert_eq!(slot.submit(ev(2)), None);
        assert_eq!(slot.submit(ev(3)), None);
        assert_eq!(slot.submit(ev(4)), None);
        assert_eq!(slot.finish(), Some(ev(4)), "the middle ones were dropped");
        assert_eq!(slot.finish(), None);
        assert!(!slot.running);
        assert_eq!(slot.submit(ev(5)), Some(ev(5)), "idle again");
    }

    #[test]
    fn a_slow_hook_sees_the_first_event_and_the_last_not_the_storm() {
        let dir = temp("coalesce");
        let log = dir.join("log");
        // The hook holds until the test opens the gate, so the storm surely lands while it runs.
        let gate = dir.join("gate");
        script(&dir, SONG_CHANGE, &format!("echo \"$UZUMAKI_ID\" >> '{}'; while [ ! -e '{}' ]; do sleep 0.02; done", log.display(), gate.display()));
        let mut hooks = Hooks::in_dir(&dir, fast());
        hooks.observe(Some(&song("1")), PlayState::Playing);
        eventually("the first song's hook", || fs::read_to_string(&log).ok().filter(|t| t.lines().last() == Some("1")));
        for i in 2..=6 {
            hooks.observe(Some(&song(&i.to_string())), PlayState::Playing);
        }
        fs::write(&gate, b"").unwrap();
        eventually("the last song", || fs::read_to_string(&log).ok().filter(|t| t.lines().last() == Some("6")));
        // Nothing else may follow: the worker has nothing left once it goes idle.
        eventually("the runner going idle", || (!locked(&hooks.song.as_ref().unwrap().slot).running).then_some(()));
        let text = fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().collect::<Vec<_>>(), ["1", "6"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn permissions_decide_whether_a_hook_may_run() {
        let dir = temp("perm");
        let me = unsafe { libc::geteuid() };
        let p = script(&dir, "h", "true");
        assert_eq!(check(&p, me), Check::Ok);
        assert_eq!(check(&dir.join("nope"), me), Check::Missing);
        let set = |mode| fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        let refused = |c: Check, part: &str| match c {
            Check::Refused(w) => assert!(w.contains(part), "{w}"),
            other => panic!("expected a refusal, got {other:?}"),
        };
        set(0o775);
        refused(check(&p, me), "group or others");
        set(0o757);
        refused(check(&p, me), "group or others");
        set(0o700);
        assert_eq!(check(&p, me), Check::Ok);
        set(0o600);
        refused(check(&p, me), "not executable");
        set(0o755);
        refused(check(&p, me.wrapping_add(1)), "another user");
        refused(check(&dir, me), "not a regular file");
        // A symlink is judged by its target.
        let link = dir.join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert_eq!(check(&link, me), Check::Ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_hook_does_not_run_and_is_logged_once() {
        let dir = temp("refused");
        let out = dir.join("out");
        let p = script(&dir, SONG_CHANGE, &format!("touch '{}'", out.display()));
        fs::set_permissions(&p, fs::Permissions::from_mode(0o777)).unwrap();
        let runner = Runner::new(p.clone(), fast());
        runner.submit(ev(1));
        eventually("idle", || (!locked(&runner.slot).running).then_some(()));
        let first = locked(&runner.slot).warned.clone();
        assert!(first.is_some_and(|w| w.contains("group or others")));
        runner.submit(ev(2));
        eventually("idle", || (!locked(&runner.slot).running).then_some(()));
        assert!(!out.exists(), "a group/other-writable hook must not run");
        // Fixed: it runs, and the warning is forgotten.
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        runner.submit(ev(3));
        eventually("the hook to run", || out.exists().then_some(()));
        assert_eq!(locked(&runner.slot).warned, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cover_is_waited_for_and_passed_when_it_lands() {
        let dir = temp("cover");
        let out = dir.join("out");
        let cover = dir.join("cover.jpg");
        script(&dir, SONG_CHANGE, &format!("printf '%s' \"$UZUMAKI_COVER\" > '{0}.tmp'; mv '{0}.tmp' '{0}'", out.display()));
        let mut s = song("1");
        s.artwork = Some(Artwork { template: format!("file://{}", cover.display()), bg: None });
        // A wait far longer than the landing, however slow the machine: the hook runs as soon
        // as the cover is there, with it.
        let mut hooks = Hooks::in_dir(&dir, Timing { cover_wait: Duration::from_secs(60), ..fast() });
        hooks.observe(Some(&s), PlayState::Playing); // returns at once, the cover isn't there yet
        let later = cover.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            fs::write(later, b"jpg").unwrap();
        });
        assert_eq!(eventually("output", || fs::read_to_string(&out).ok()), cover.display().to_string());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cover_that_never_comes_costs_at_most_the_wait_and_the_hook_still_runs() {
        let dir = temp("nocover");
        let out = dir.join("out");
        script(&dir, SONG_CHANGE, &format!("printf '[%s]' \"$UZUMAKI_COVER\" > '{0}.tmp'; mv '{0}.tmp' '{0}'", out.display()));
        let mut s = song("1");
        s.artwork = Some(Artwork { template: format!("file://{}/missing.jpg", dir.display()), bg: None });
        let timing = Timing { cover_wait: Duration::from_secs(3), ..fast() };
        let mut hooks = Hooks::in_dir(&dir, timing);
        let t = Instant::now();
        hooks.observe(Some(&s), PlayState::Playing);
        assert!(t.elapsed() < timing.cover_wait / 2, "observe must not wait: the UI thread calls it ({:?})", t.elapsed());
        assert_eq!(eventually("output", || fs::read_to_string(&out).ok()), "[]");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn off_runs_nothing() {
        let mut hooks = Hooks::off();
        hooks.observe(Some(&song("1")), PlayState::Playing);
        assert!(hooks.last_song.is_none() && hooks.song.is_none());
    }
}
