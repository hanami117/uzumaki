//! Starting Uzumaki when `uzu` finds it not running: which commands wait for what (a pure
//! decision, tested), spawning the app hidden, and waiting for its socket and bridge.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use uzumaki_ctl::{Client, Reply, Request};

use crate::cli::Command;
use crate::run::Fail;

/// How long to wait for the socket to appear after spawning.
pub const SOCKET_WAIT: Duration = Duration::from_secs(15);
/// How long, after that, to wait for the playback bridge (`Status::connected`).
pub const BRIDGE_WAIT: Duration = Duration::from_secs(20);

/// What a command needs when the app isn't running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Don't start the app at all.
    Never,
    /// Spawn the app but don't wait: the command is answered from the disk cache.
    Cache,
    /// Spawn and wait for the socket.
    Socket,
    /// Spawn, wait for the socket, then for the bridge to be ready.
    Connected,
}

/// `has_cache`: the library cache directory exists, so the library listings can be answered from it.
pub fn plan(cmd: &Command, has_cache: bool) -> Plan {
    match cmd {
        // Quit and Restart are handled by main and never auto-start; this keeps them from starting
        // the app by accident.
        Command::Quit | Command::Restart => Plan::Never,
        Command::Playlists | Command::Playlist(_) | Command::Albums(_) | Command::Album(_) if has_cache => Plan::Cache,
        Command::Play
        | Command::PlayNumber(_)
        | Command::PlayCollection { .. }
        | Command::Toggle
        | Command::Next
        | Command::Prev
        | Command::Seek(_)
        | Command::Shuffle(_)
        | Command::Repeat(_)
        | Command::Autoplay(_)
        | Command::Jump(_)
        | Command::JumpTitle(_)
        | Command::Move { .. }
        | Command::Remove(_)
        | Command::Pick { .. } => Plan::Connected,
        _ => Plan::Socket,
    }
}

pub fn app_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("UZUMAKI_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let beside = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok()).and_then(|e| e.parent().map(|d| d.join("uzumaki")));
    match beside {
        Some(p) if p.exists() => p,
        _ => PathBuf::from("uzumaki"),
    }
}

/// Starts the app detached (its own session, no stdio), windowless.
pub fn spawn_app() -> Result<(), Fail> {
    spawn_binary(&app_binary())
}

/// Environment that a test instance overrides and the user manager wouldn't know about.
fn passes_through(name: &str) -> bool {
    name.starts_with("UZUMAKI_") || name.starts_with("CADENCE_") || matches!(name, "XDG_CONFIG_HOME" | "XDG_CACHE_HOME" | "XDG_STATE_HOME" | "TMPDIR")
}

/// The `systemd-run` command line (without the program name) that starts `bin` hidden as a
/// transient user unit. `env` is the caller's environment; only the overrides that matter pass.
/// Pure, so it is tested without running anything.
pub fn systemd_run_args(env: &BTreeMap<String, String>, bin: &Path, unit_stamp: u64) -> Vec<String> {
    let mut args = vec!["--user".to_string(), "--collect".into(), "--quiet".into(), format!("--unit=uzumaki-{unit_stamp}")];
    args.extend(env.iter().filter(|(k, _)| passes_through(k)).map(|(k, v)| format!("--setenv={k}={v}")));
    args.push(bin.display().to_string());
    args.push("--hidden".into());
    args
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

fn spawn_binary(bin: &Path) -> Result<(), Fail> {
    let found = if bin.components().count() > 1 { bin.exists() } else { on_path(&bin.to_string_lossy()).is_some() };
    if !found {
        return Err(Fail::err(format!("can't start {}: No such file or directory (os error 2)", bin.display())));
    }
    if on_path("systemd-run").is_some() && spawn_unit(bin) {
        return Ok(());
    }
    spawn_direct(bin)
}

/// Starts the app as a transient user service, which has the desktop session's environment
/// (display variables) even when `uzu` runs over ssh. False if `systemd-run` didn't succeed.
fn spawn_unit(bin: &Path) -> bool {
    let env: BTreeMap<String, String> = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Process::new("systemd-run")
        .args(systemd_run_args(&env, bin, stamp))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn spawn_direct(bin: &Path) -> Result<(), Fail> {
    Process::new(bin)
        .arg("--hidden")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| Fail::err(format!("can't start {}: {e}", bin.display())))
}

/// How long `uzu quit` waits for the socket to stop accepting connections.
pub const QUIT_WAIT: Duration = Duration::from_secs(10);

/// Waits until nothing accepts connections on the socket any more.
pub fn wait_gone(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if Client::connect_to(path).is_err() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits for the socket to accept a connection.
pub fn wait_socket(timeout: Duration) -> Result<Client, Fail> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(client) = Client::connect() {
            return Ok(client);
        }
        if Instant::now() >= deadline {
            return Err(Fail::err(format!("Uzumaki didn't come up within {} seconds", timeout.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Waits until the status says the bridge is connected; gives up quietly after `timeout`.
pub fn wait_connected(client: &mut Client, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match client.request(&Request::Status) {
            Ok(Reply::Status(st)) if st.connected => return,
            Ok(_) => {}
            Err(_) => return,
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Spawns the app and waits as `plan` says. `Plan::Cache` returns `None` right after the spawn.
pub fn boot(plan: Plan, quiet: bool) -> Result<Option<Client>, Fail> {
    if !quiet && plan != Plan::Never {
        eprintln!("Starting Uzumaki…");
    }
    match plan {
        Plan::Never => Ok(None),
        Plan::Cache => {
            let _ = spawn_app(); // the cache answers either way
            Ok(None)
        }
        Plan::Socket | Plan::Connected => {
            spawn_app()?;
            let mut client = wait_socket(SOCKET_WAIT)?;
            if plan == Plan::Connected {
                wait_connected(&mut client, BRIDGE_WAIT);
            }
            Ok(Some(client))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uzumaki_ctl::{RepeatMode, SeekTo};

    #[test]
    fn what_each_command_waits_for() {
        let parse = |s: &str| crate::cli::parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>()).unwrap().cmd;
        for c in ["playlists", "albums", "playlist road", "album disco"] {
            assert_eq!(plan(&parse(c), true), Plan::Cache, "{c}");
            assert_eq!(plan(&parse(c), false), Plan::Socket, "{c} without a cache");
        }
        for c in ["play 3", "play mix 2", "toggle", "next", "prev", "seek +10", "jump 2", "jump numb", "pick 1", "insert 1", "add 1", "move 2 1", "del 1", "shuffle", "repeat all", "autoplay", "autoplay off"] {
            assert_eq!(plan(&parse(c), true), Plan::Connected, "{c}");
        }
        for c in ["status", "watch", "queue", "search x", "show", "vol", "vol 50", "pause"] {
            assert_eq!(plan(&parse(c), true), Plan::Socket, "{c}");
        }
        assert_eq!(plan(&Command::Screen, false), Plan::Socket);
        for has_cache in [true, false] {
            assert_eq!(plan(&Command::Quit, has_cache), Plan::Never);
            assert_eq!(plan(&Command::Restart, has_cache), Plan::Never);
        }
        assert_eq!(plan(&Command::Seek(SeekTo::Percent(5.0)), false), Plan::Connected);
        assert_eq!(plan(&Command::Repeat(Some(RepeatMode::One)), false), Plan::Connected);
    }

    #[test]
    fn systemd_run_argv() {
        let env: BTreeMap<String, String> = [("HOME", "/home/x"), ("PATH", "/bin"), ("UZUMAKI_CTL_SOCKET", "/tmp/s.sock"), ("CADENCE_MUTE", "1"), ("XDG_CONFIG_HOME", "/tmp/c"), ("XDG_DATA_HOME", "/tmp/d"), ("TMPDIR", "/tmp/t")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let args = systemd_run_args(&env, Path::new("/opt/uzumaki"), 1700000000);
        assert_eq!(
            args,
            [
                "--user",
                "--collect",
                "--quiet",
                "--unit=uzumaki-1700000000",
                "--setenv=CADENCE_MUTE=1",
                "--setenv=TMPDIR=/tmp/t",
                "--setenv=UZUMAKI_CTL_SOCKET=/tmp/s.sock",
                "--setenv=XDG_CONFIG_HOME=/tmp/c",
                "/opt/uzumaki",
                "--hidden"
            ]
        );
        let bare = systemd_run_args(&BTreeMap::new(), Path::new("uzumaki"), 5);
        assert_eq!(bare, ["--user", "--collect", "--quiet", "--unit=uzumaki-5", "uzumaki", "--hidden"]);
    }

    #[test]
    fn a_missing_binary_is_a_clear_error() {
        let fail = spawn_binary(Path::new("/nonexistent/uzumaki-test-binary")).unwrap_err();
        assert!(fail.msg.starts_with("uzu: can't start /nonexistent/uzumaki-test-binary"), "{}", fail.msg);
    }
}
