//! Client for the Electron playback bridge (`bridge/`): spawns it, writes JSON commands to its
//! stdin and turns its stdout into typed events. See README.md for the protocol.
//!
//! Requests made with [`Bridge::call`] carry a closure that turns the reply into an app message;
//! it runs on the reader thread, so JSON parsing stays off the UI thread.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::models::Song;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PlayState {
    #[default]
    None,
    Loading,
    Playing,
    Paused,
    Stopped,
    Ended,
    Seeking,
    Waiting,
    Stalled,
    Completed,
}

impl PlayState {
    fn parse(s: &str) -> Self {
        match s {
            "loading" => Self::Loading,
            "playing" => Self::Playing,
            "paused" => Self::Paused,
            "stopped" => Self::Stopped,
            "ended" => Self::Ended,
            "seeking" => Self::Seeking,
            "waiting" => Self::Waiting,
            "stalled" => Self::Stalled,
            "completed" => Self::Completed,
            _ => Self::None,
        }
    }

    /// Whether the play/pause button should show "pause" (audio is running or about to).
    pub fn is_active(self) -> bool {
        matches!(self, Self::Playing | Self::Loading | Self::Seeking | Self::Waiting | Self::Stalled)
    }
}

/// Unsolicited messages from the bridge.
#[derive(Clone, Debug)]
pub enum Event {
    /// Widevine CDM install result (first event after launch).
    Widevine { ok: bool, error: Option<String> },
    /// The page's MusicKit instance was found (again after every page reload).
    Ready { authorized: bool, storefront: Option<String>, volume: Option<f64> },
    /// The sign-in changed; `storefront` is MusicKit's storefront at that moment (newer bridges).
    Auth { authorized: bool, storefront: Option<String> },
    /// The page's MusicKit reports another `storefrontId`. That is the catalog's, not necessarily
    /// the account's (`/v1/me/storefront` is asked for that, see `state::signin`).
    Storefront(String),
    NowPlaying(Option<Song>),
    PlayState(PlayState),
    /// Playback position, at most once per second.
    Time { current: f64, duration: f64 },
    /// MusicKit's volume changed (0..1), from us or from the page.
    Volume(f64),
    /// The sign-in window was shown or hidden. `closed`: the user closed it (not the app's `hide`).
    Window { visible: bool, closed: bool },
    Error(String),
    /// The Electron process is gone.
    Exited,
    /// The bridge was stopped on purpose (local only mode, `Engine::stop`): its last word,
    /// instead of `Exited`. Nothing is wrong; media keys only hear that nothing plays.
    Stopped,
}

type ReplyFn<M> = Box<dyn FnOnce(Result<Value, String>) -> Option<M> + Send>;

struct Inner<M> {
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Child>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, ReplyFn<M>>>,
    /// Set by `Engine::stop`: the reader drops whatever still comes and ends with `Stopped`.
    silent: AtomicBool,
}

/// Handle to the bridge process. Cheap to clone.
pub struct Bridge<M> {
    inner: Arc<Inner<M>>,
}

impl<M> Clone for Bridge<M> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// Where the bridge lives: $UZUMAKI_BRIDGE_DIR (or the old $CADENCE_BRIDGE_DIR); else an installed
/// copy (`install.sh`: `<prefix>/bin/uzumaki` next to `<prefix>/share/uzumaki/bridge`); else
/// `../bridge` next to this crate, for builds run from the source tree.
pub fn bridge_dir() -> PathBuf {
    let dir = std::env::var_os("UZUMAKI_BRIDGE_DIR")
        .or_else(|| std::env::var_os("CADENCE_BRIDGE_DIR"))
        .map(PathBuf::from)
        .or_else(installed_bridge_dir)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../bridge"));
    dir.canonicalize().unwrap_or(dir)
}

/// The castLabs Electron executable in a bridge folder's `node_modules`: inside the app bundle on
/// macOS, a plain binary on Linux.
fn electron_exe(bridge: &std::path::Path) -> PathBuf {
    let dist = bridge.join("node_modules/electron/dist");
    if cfg!(target_os = "macos") {
        dist.join("Electron.app/Contents/MacOS/Electron")
    } else {
        dist.join("electron")
    }
}

fn installed_bridge_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bridge_in_bundle(&exe).or_else(|| bridge_beside(&exe)).or_else(|| bridge_in_libdir(&exe))
}

/// macOS: `Uzumaki.app/Contents/Resources/bridge` for a binary in `Contents/MacOS`, if it holds a
/// bridge (`packaging/macos/build.sh` puts it there).
fn bridge_in_bundle(exe: &std::path::Path) -> Option<PathBuf> {
    let dir = exe.parent()?.parent()?.join("Resources/bridge");
    dir.join("main.js").is_file().then_some(dir)
}

/// `<prefix>/share/uzumaki/bridge` for a binary at `<prefix>/bin/uzumaki`, if it holds a bridge.
fn bridge_beside(exe: &std::path::Path) -> Option<PathBuf> {
    let dir = exe.parent()?.parent()?.join("share/uzumaki/bridge");
    dir.join("main.js").is_file().then_some(dir)
}

/// `<dir>/bridge` for a binary at `<dir>/uzumaki`, if it holds a bridge: a distribution package
/// keeps the program in `/usr/lib/uzumaki` (`uzumaki`, `uzu`, `bridge/`) with links in `/usr/bin`
/// (`packaging/aur/PKGBUILD`). `current_exe` gives the link's target on Linux.
fn bridge_in_libdir(exe: &std::path::Path) -> Option<PathBuf> {
    let dir = exe.parent()?.join("bridge");
    dir.join("main.js").is_file().then_some(dir)
}

/// True when this binary runs from inside an AppImage: the AppImage runtime sets `APPIMAGE` (the
/// .AppImage file) and `APPDIR` (where it mounted or extracted it), and the binary is under `APPDIR`.
/// Then the bridge's Electron gets `--no-sandbox`. Chromium's sandbox needs either user namespaces
/// (blocked for unprivileged programs on Ubuntu 24.04+ by AppArmor, and on hardened kernels) or the
/// `chrome-sandbox` helper owned by root with the setuid bit. An AppImage can't carry a setuid
/// file (its squashfs is mounted `nosuid` by an unprivileged user), so without the flag Electron
/// aborts on those systems. The bridge only loads music.apple.com in its own window. Packages
/// (AUR) and install.sh keep the sandbox. Both variables are checked, together with the path, so
/// that a stray `APPIMAGE` inherited from another AppImage doesn't turn the sandbox off.
/// [`in_appimage`] for this process.
pub fn running_from_appimage() -> bool {
    let exe = std::env::current_exe().ok();
    in_appimage(std::env::var_os("APPIMAGE"), std::env::var_os("APPDIR"), exe.as_deref())
}

fn in_appimage(appimage: Option<std::ffi::OsString>, appdir: Option<std::ffi::OsString>, exe: Option<&std::path::Path>) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let (Some(image), Some(dir), Some(exe)) = (appimage, appdir, exe) else { return false };
    !image.is_empty() && !dir.is_empty() && exe.starts_with(std::path::Path::new(&dir))
}

#[cfg(all(test, target_os = "linux"))]
mod appimage_tests {
    use super::in_appimage;
    use std::path::Path;

    #[test]
    fn electron_runs_without_the_sandbox_only_inside_an_appimage() {
        let exe = Path::new("/tmp/.mount_UzumakXYZ/usr/lib/uzumaki/uzumaki");
        let some = |s: &str| Some(s.into());
        assert!(in_appimage(some("/home/u/Uzumaki.AppImage"), some("/tmp/.mount_UzumakXYZ"), Some(exe)));
        // Not an AppImage, or another AppImage's variables leaked into an installed copy.
        assert!(!in_appimage(None, None, Some(exe)));
        assert!(!in_appimage(some("/home/u/Other.AppImage"), some("/tmp/.mount_OtherABC"), Some(Path::new("/usr/lib/uzumaki/uzumaki"))));
        assert!(!in_appimage(some(""), some("/tmp/.mount_UzumakXYZ"), Some(exe)));
        assert!(!in_appimage(some("/home/u/Uzumaki.AppImage"), None, Some(exe)));
    }
}

#[cfg(test)]
mod install_tests {
    use super::{bridge_beside, bridge_in_bundle, bridge_in_libdir};

    #[test]
    fn finds_a_packaged_bridge_in_the_programs_own_folder() {
        let lib = std::env::temp_dir().join(format!("uzumaki-libdir-{}/usr/lib/uzumaki", std::process::id()));
        let exe = lib.join("uzumaki");
        assert_eq!(bridge_in_libdir(&exe), None);
        std::fs::create_dir_all(lib.join("bridge")).unwrap();
        std::fs::write(lib.join("bridge/main.js"), "").unwrap();
        assert_eq!(bridge_in_libdir(&exe), Some(lib.join("bridge")));
        assert_eq!(bridge_beside(&exe), None);
        std::fs::remove_dir_all(lib.ancestors().nth(3).unwrap()).unwrap();
    }

    #[test]
    fn finds_the_bridge_inside_a_mac_app_bundle() {
        let app = std::env::temp_dir().join(format!("uzumaki-bundle-{}/Uzumaki.app", std::process::id()));
        let exe = app.join("Contents/MacOS/uzumaki");
        let bridge = app.join("Contents/Resources/bridge");
        assert_eq!(bridge_in_bundle(&exe), None);
        std::fs::create_dir_all(&bridge).unwrap();
        std::fs::write(bridge.join("main.js"), "").unwrap();
        assert_eq!(bridge_in_bundle(&exe), Some(bridge));
        std::fs::remove_dir_all(app.parent().unwrap()).unwrap();
    }

    #[test]
    fn finds_the_installed_bridge_next_to_the_binary() {
        let prefix = std::env::temp_dir().join(format!("uzumaki-prefix-{}", std::process::id()));
        let exe = prefix.join("bin/uzumaki");
        let bridge = prefix.join("share/uzumaki/bridge");
        // A build in a source tree (target/release/uzumaki) has no installed bridge beside it.
        assert_eq!(bridge_beside(&exe), None);
        std::fs::create_dir_all(&bridge).unwrap();
        assert_eq!(bridge_beside(&exe), None, "an empty folder isn't a bridge");
        std::fs::write(bridge.join("main.js"), "").unwrap();
        assert_eq!(bridge_beside(&exe), Some(bridge));
        std::fs::remove_dir_all(&prefix).unwrap();
    }
}

impl<M: From<Event> + Send + 'static> Bridge<M> {
    /// Starts Electron and a reader thread. Every event and every reply message goes to `sink`
    /// (called on the reader thread). Electron's stderr goes to `log_path`.
    pub fn spawn(
        show_window: bool,
        log_path: &std::path::Path,
        sink: impl Fn(M) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let dir = bridge_dir();
        let electron = electron_exe(&dir);
        if !electron.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "castLabs Electron not found at {} (run `npm ci` and then `node node_modules/electron/install.js` in {})",
                    electron.display(),
                    dir.display()
                ),
            ));
        }
        let mut cmd = Command::new(&electron);
        cmd.arg(&dir);
        if show_window {
            cmd.arg("--show");
        }
        if running_from_appimage() {
            cmd.arg("--no-sandbox");
        }
        Self::spawn_command(cmd, log_path, sink)
    }

    /// Starts `cmd` as the bridge (`spawn`; tests pass a stand-in process).
    pub fn spawn_command(mut cmd: Command, log_path: &std::path::Path, sink: impl Fn(M) + Send + Sync + 'static) -> std::io::Result<Self> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(log_path)?)
            .spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let inner = Arc::new(Inner {
            stdin: Mutex::new(child.stdin.take()),
            child: Mutex::new(child),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            silent: AtomicBool::new(false),
        });
        let reader = inner.clone();
        std::thread::Builder::new()
            .name("bridge-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if reader.silent.load(Ordering::SeqCst) {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                    if let Some(name) = msg["event"].as_str() {
                        if let Some(event) = parse_event(name, &msg) {
                            sink(M::from(event));
                        }
                    } else if let Some(id) = msg["id"].as_u64() {
                        let handler = reader.pending.lock().unwrap().remove(&id);
                        let result = if msg["ok"].as_bool() == Some(true) {
                            Ok(msg["result"].clone())
                        } else {
                            Err(msg["error"].as_str().unwrap_or("unknown bridge error").to_string())
                        };
                        if let Some(m) = handler.and_then(|h| h(result)) {
                            sink(m);
                        }
                    }
                }
                let silent = reader.silent.load(Ordering::SeqCst);
                sink(M::from(if silent { Event::Stopped } else { Event::Exited }));
            })?;
        Ok(Self { inner })
    }

    /// Sends a command and hands its reply to `on_reply` (on the reader thread).
    pub fn call(&self, mut cmd: Value, on_reply: impl FnOnce(Result<Value, String>) -> Option<M> + Send + 'static) {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        cmd["id"] = json!(id);
        self.inner.pending.lock().unwrap().insert(id, Box::new(on_reply));
        let written = match self.inner.stdin.lock().unwrap().as_mut() {
            Some(stdin) => writeln!(stdin, "{cmd}").and_then(|_| stdin.flush()).is_ok(),
            None => false,
        };
        if !written {
            // The reader thread reports Exited; just drop the handler.
            self.inner.pending.lock().unwrap().remove(&id);
        }
    }

    /// Fire-and-forget command; a failure becomes an [`Event::Error`].
    pub fn send(&self, cmd: Value) {
        let name = cmd["cmd"].as_str().unwrap_or("?").to_string();
        self.call(cmd, move |r| r.err().map(|e| M::from(Event::Error(format!("{name}: {e}")))));
    }

    /// Apple Music API request through the page's MusicKit (`{{storefrontId}}` is substituted).
    pub fn api(&self, path: &str, params: Value, on_reply: impl FnOnce(Result<Value, String>) -> Option<M> + Send + 'static) {
        self.call(json!({"cmd": "api", "path": path, "params": params}), on_reply);
    }

    pub fn play(&self) {
        self.send(json!({"cmd": "play"}));
    }
    /// Plays the song MusicKit holds paused (`prime_queue`) from `seconds` in.
    pub fn play_at(&self, seconds: f64) {
        self.send(json!({"cmd": "play", "at": seconds.max(0.0)}));
    }
    pub fn pause(&self) {
        self.send(json!({"cmd": "pause"}));
    }
    pub fn next(&self) {
        self.send(json!({"cmd": "next"}));
    }
    pub fn previous(&self) {
        self.send(json!({"cmd": "previous"}));
    }
    pub fn seek(&self, seconds: f64) {
        self.send(json!({"cmd": "seek", "time": seconds}));
    }
    pub fn set_volume(&self, value: f64) {
        self.send(json!({"cmd": "volume", "value": value.clamp(0.0, 1.0)}));
    }
    /// Starts `ids[0]` now; the rest is what follows (the bridge feeds it to MusicKit lazily).
    /// `cycle`: the whole list for repeat all, else None. `on_fail` gets MusicKit's error when it
    /// refuses (e.g. `[mk-007] NOT_FOUND` for a song gone from the catalog); a request replaced
    /// by a newer one isn't a failure.
    /// `start`: seconds into the first song (a resumed song), else 0.
    pub fn play_queue(&self, ids: &[String], cycle: Option<&[String]>, start: f64, on_fail: impl FnOnce(String) -> Option<M> + Send + 'static) {
        let mut cmd = json!({"cmd": "setQueue", "songIds": ids, "loop": cycle});
        if start > 0.0 {
            cmd["startTime"] = json!(start);
        }
        self.call(cmd, move |r| r.err().and_then(on_fail));
    }
    /// Like `play_queue`, but leaves the first song paused at `start` (Resume where you left
    /// off): `play_at` then starts it. `done` gets Ok once MusicKit holds it.
    pub fn prime_queue(&self, ids: &[String], cycle: Option<&[String]>, start: f64, done: impl FnOnce(Result<(), String>) -> Option<M> + Send + 'static) {
        let cmd = json!({"cmd": "setQueue", "songIds": ids, "loop": cycle, "startTime": start.max(0.0), "startPlaying": false});
        self.call(cmd, move |r| match r {
            Ok(v) if v.get("superseded").and_then(Value::as_bool) == Some(true) => None,
            Ok(_) => done(Ok(())),
            Err(e) => done(Err(e)),
        });
    }
    /// Asks the bridge to send the current song, play state and time again as events.
    pub fn announce(&self) {
        self.send(json!({"cmd": "announce"}));
    }
    /// Replaces what plays after the current song (shuffle, play next, add to queue).
    pub fn set_upcoming(&self, ids: &[String], cycle: Option<&[String]>) {
        self.send(json!({"cmd": "setUpcoming", "songIds": ids, "loop": cycle}));
    }
    /// "off" | "one" | "all"
    pub fn set_repeat(&self, mode: &str) {
        self.send(json!({"cmd": "setRepeat", "mode": mode}));
    }
    /// Time events per second: 1 normally, ~4 while synced lyrics are on screen.
    pub fn time_rate(&self, hz: u32) {
        self.send(json!({"cmd": "timeRate", "hz": hz}));
    }
    /// Apple Music API write (POST with a JSON body).
    pub fn api_post(&self, path: &str, body: Value, on_reply: impl FnOnce(Result<Value, String>) -> Option<M> + Send + 'static) {
        self.call(json!({"cmd": "api", "path": path, "params": {}, "method": "POST", "body": body}), on_reply);
    }
    /// A write with any method ("POST", "DELETE", …); `path` may carry a query string.
    pub fn api_send(&self, method: &str, path: &str, body: Option<Value>, on_reply: impl FnOnce(Result<Value, String>) -> Option<M> + Send + 'static) {
        let mut cmd = json!({"cmd": "api", "path": path, "params": {}, "method": method});
        if let Some(b) = body {
            cmd["body"] = b;
        }
        self.call(cmd, on_reply);
    }
    pub fn play_album(&self, album_id: &str, start_with: usize) {
        self.send(json!({"cmd": "setQueue", "album": album_id, "startWith": start_with}));
    }
    pub fn play_station(&self, station_id: &str) {
        self.send(json!({"cmd": "setQueue", "station": station_id}));
    }
    pub fn play_playlist(&self, playlist_id: &str, start_with: usize) {
        self.send(json!({"cmd": "setQueue", "playlist": playlist_id, "startWith": start_with}));
    }
    /// 256 (best) or 64 (data saver). MusicKit resets it on every page load, so the app
    /// sends it again after each `ready`.
    pub fn set_bitrate(&self, kbps: u32) {
        self.send(json!({"cmd": "setBitrate", "kbps": kbps}));
    }
    /// Signs MusicKit out, clears the profile's apple.com cookies and the page's storage, then
    /// reloads the page (a `ready` with `authorized: false` follows).
    pub fn sign_out(&self, on_reply: impl FnOnce(Result<Value, String>) -> Option<M> + Send + 'static) {
        self.call(json!({"cmd": "signOut"}), on_reply);
    }
    /// The guided sign-in: the window, small and centred (on `near`, our window's frame in screen
    /// points, when given), then Apple's sign-in. Again while it shows: brings it to the front.
    pub fn sign_in(&self, near: Option<[f32; 4]>) {
        self.send(sign_in_cmd(near));
    }
    /// Puts the page's MusicKit on the account's storefront (`mk.storefrontId`; the page starts on
    /// its URL's, "us"). The reply says what the page had and has (`applyStorefront` in signin.js).
    pub fn set_storefront(&self, id: &str, on_reply: impl FnOnce(Result<Value, String>) -> Option<M> + Send + 'static) {
        self.call(set_storefront_cmd(id), on_reply);
    }
    pub fn show_window(&self, visible: bool) {
        self.send(json!({"cmd": if visible { "show" } else { "hide" }}));
    }

    /// Asks Electron to quit, closes its stdin, and kills it if it is still there after 3 s.
    pub fn shutdown(&self) {
        self.call(json!({"cmd": "quit"}), |_| None);
        self.inner.stdin.lock().unwrap().take();
        let mut child = self.inner.child.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Starts the bridge when Apple Music is wanted (at launch, after the onboarding's choice, or
/// on a switch in Settings → Account) and stops it again for local only mode. `State` holds one
/// and asks it; `main` keeps a clone to shut down whatever runs at exit. Cheap to clone.
pub struct Engine<M> {
    spawn: Arc<dyn Fn(bool) -> std::io::Result<Bridge<M>> + Send + Sync>,
    current: Arc<Mutex<Option<Bridge<M>>>>,
    log_path: PathBuf,
}

impl<M> Clone for Engine<M> {
    fn clone(&self) -> Self {
        Self { spawn: self.spawn.clone(), current: self.current.clone(), log_path: self.log_path.clone() }
    }
}

impl<M: From<Event> + Send + 'static> Engine<M> {
    /// `spawn(show_window)` starts one bridge (`Bridge::spawn` with the app's sink).
    pub fn new(log_path: PathBuf, spawn: impl Fn(bool) -> std::io::Result<Bridge<M>> + Send + Sync + 'static) -> Self {
        Self { spawn: Arc::new(spawn), current: Arc::default(), log_path }
    }

    /// Electron's log, for error messages.
    pub fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }

    /// Starts a bridge (`show_window`: with the sign-in window showing, for `--login`).
    pub fn start(&self, show_window: bool) -> std::io::Result<Bridge<M>> {
        let b = (self.spawn)(show_window)?;
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(b.clone());
        Ok(b)
    }

    /// Stops `b` on purpose: from now on its events are dropped and its last one is
    /// [`Event::Stopped`], so a bridge started after it never hears about it. Electron quits on a
    /// thread of its own (up to 3 s), so the UI doesn't wait.
    pub fn stop(&self, b: Bridge<M>) {
        {
            let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
            if cur.as_ref().is_some_and(|c| Arc::ptr_eq(&c.inner, &b.inner)) {
                *cur = None;
            }
        }
        b.inner.silent.store(true, Ordering::SeqCst);
        let _ = std::thread::Builder::new().name("bridge-stop".into()).spawn(move || b.shutdown());
    }

    /// At exit: shuts down the bridge that runs, if any.
    pub fn shutdown(&self) {
        let b = self.current.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(b) = b {
            b.shutdown();
        }
    }

    /// Whether a bridge runs (started and not stopped).
    #[cfg(test)]
    pub fn running(&self) -> bool {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }
}

#[cfg(test)]
pub(crate) mod engine_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A stand-in bridge: a process that reads its stdin and says nothing.
    pub(crate) fn quiet_process() -> Command {
        let mut c = Command::new("sh");
        c.args(["-c", "cat > /dev/null"]);
        c
    }

    /// An engine of stand-in bridges; the counter counts starts.
    pub(crate) fn fake<M: From<Event> + Send + 'static>(sink: impl Fn(M) + Send + Sync + Clone + 'static) -> (Engine<M>, Arc<AtomicUsize>) {
        let starts = Arc::new(AtomicUsize::new(0));
        let n = starts.clone();
        let log = std::env::temp_dir().join(format!("uzumaki-fake-bridge-{}.log", std::process::id()));
        let engine = Engine::new(log.clone(), move |_show| {
            n.fetch_add(1, Ordering::SeqCst);
            Bridge::spawn_command(quiet_process(), &log, sink.clone())
        });
        (engine, starts)
    }

    fn wait_for(events: &Mutex<Vec<Event>>, what: impl Fn(&Event) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if events.lock().unwrap().iter().any(&what) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_stopped_bridge_says_stopped_and_a_crashed_one_exited() {
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let ev = events.clone();
        let (engine, starts) = fake::<Event>(move |e| ev.lock().unwrap().push(e));
        assert!(!engine.running());
        let b = engine.start(false).unwrap();
        assert!(engine.running());
        engine.stop(b);
        assert!(!engine.running());
        assert!(wait_for(&events, |e| matches!(e, Event::Stopped)), "stopped on purpose: Stopped");
        assert!(!events.lock().unwrap().iter().any(|e| matches!(e, Event::Exited)), "never Exited");

        // A second bridge, ending by itself (as a crash would), says Exited; the exit shutdown
        // finds nothing left to do once it was stopped.
        events.lock().unwrap().clear();
        let b = engine.start(false).unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        b.shutdown();
        assert!(wait_for(&events, |e| matches!(e, Event::Exited)));
        engine.shutdown();
        assert!(!engine.running());
    }
}

fn parse_event(name: &str, msg: &Value) -> Option<Event> {
    let flag = |k: &str| msg[k].as_bool().unwrap_or(false);
    Some(match name {
        "widevine" => Event::Widevine { ok: flag("ok"), error: msg["error"].as_str().map(String::from) },
        "ready" => Event::Ready {
            authorized: flag("authorized"),
            storefront: msg["storefront"].as_str().map(String::from),
            volume: msg["volume"].as_f64(),
        },
        "auth" => Event::Auth { authorized: flag("authorized"), storefront: msg["storefront"].as_str().map(String::from) },
        "storefront" => Event::Storefront(msg["id"].as_str().filter(|s| !s.is_empty())?.to_string()),
        "nowPlaying" => Event::NowPlaying(Song::from_bridge_item(&msg["item"])),
        "playbackState" => Event::PlayState(PlayState::parse(msg["state"].as_str().unwrap_or(""))),
        "time" => Event::Time {
            current: msg["current"].as_f64().unwrap_or(0.0),
            duration: msg["duration"].as_f64().unwrap_or(0.0),
        },
        "volume" => Event::Volume(msg["value"].as_f64()?),
        "window" => Event::Window { visible: flag("visible"), closed: msg["reason"].as_str() == Some("closed") },
        "error" => Event::Error(msg["message"].as_str().unwrap_or("unknown error").to_string()),
        _ => return None,
    })
}

/// `{"cmd":"signIn","near":{x,y,w,h}}`, `near` left out when unknown (Linux).
/// `setStorefront`. The id goes as `storefront`: `id` is the message's own (`call` sets it).
fn set_storefront_cmd(storefront: &str) -> Value {
    json!({"cmd": "setStorefront", "storefront": storefront})
}

fn sign_in_cmd(near: Option<[f32; 4]>) -> Value {
    let mut cmd = json!({"cmd": "signIn"});
    if let Some([x, y, w, h]) = near.filter(|r| r.iter().all(|v| v.is_finite()) && r[2] > 0.0 && r[3] > 0.0) {
        cmd["near"] = json!({"x": x.round(), "y": y.round(), "w": w.round(), "h": h.round()});
    }
    cmd
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    fn event(line: &str) -> Option<Event> {
        let v: Value = serde_json::from_str(line).unwrap();
        parse_event(v["event"].as_str().unwrap(), &v)
    }

    #[test]
    fn the_window_event_says_whether_the_user_closed_it() {
        let closed = |line| matches!(event(line), Some(Event::Window { visible: false, closed: true }));
        assert!(closed(r#"{"event":"window","visible":false,"reason":"closed"}"#));
        assert!(!closed(r#"{"event":"window","visible":false,"reason":"hidden"}"#));
        // An older bridge sends no reason: not a close.
        assert!(!closed(r#"{"event":"window","visible":false}"#));
        assert!(matches!(event(r#"{"event":"window","visible":true}"#), Some(Event::Window { visible: true, closed: false })));
    }

    #[test]
    fn set_storefront_keeps_clear_of_the_message_id() {
        let cmd = set_storefront_cmd("it");
        assert_eq!(cmd["storefront"], "it");
        assert!(cmd.get("id").is_none(), "call() puts the message id there");
    }

    #[test]
    fn auth_and_storefront_events_carry_the_storefront() {
        let auth = event(r#"{"event":"auth","authorized":true,"storefront":"it"}"#);
        assert!(matches!(auth, Some(Event::Auth { authorized: true, storefront: Some(ref f) }) if f == "it"));
        // An older bridge: no storefront.
        assert!(matches!(event(r#"{"event":"auth","authorized":true}"#), Some(Event::Auth { storefront: None, .. })));
        assert!(matches!(event(r#"{"event":"storefront","id":"jp"}"#), Some(Event::Storefront(ref f)) if f == "jp"));
        assert!(event(r#"{"event":"storefront","id":""}"#).is_none());
        assert!(event(r#"{"event":"storefront"}"#).is_none());
    }

    #[test]
    fn sign_in_sends_our_frame_only_when_known() {
        assert_eq!(sign_in_cmd(None), json!({"cmd": "signIn"}));
        assert_eq!(
            sign_in_cmd(Some([100.4, 50.0, 1360.0, 860.0])),
            json!({"cmd": "signIn", "near": {"x": 100.0, "y": 50.0, "w": 1360.0, "h": 860.0}})
        );
        assert_eq!(sign_in_cmd(Some([0.0, 0.0, 0.0, 0.0])), json!({"cmd": "signIn"}));
        assert_eq!(sign_in_cmd(Some([f32::NAN, 0.0, 10.0, 10.0])), json!({"cmd": "signIn"}));
    }
}
