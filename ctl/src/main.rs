//! `uzu`: an mpc-style command line for Uzumaki. The running app plays the part of mpd; this
//! sends one request, prints the reply and exits. The protocol lives in `lib.rs`.

mod args;
mod boot;
mod cache;
mod cli;
mod fmt;
mod last;
mod list;
mod run;
mod tui;
#[cfg(test)]
mod tests;

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

use uzumaki_ctl::{Client, Reply, Request};

use cli::{Command, Invocation};
use run::{Env, Fail, Offline, Term, Timing};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn install_sigint() {
    // Plain signal(): the handler only sets a flag that `uzu watch` checks every few ms, so it
    // can show the cursor again. SIGTERM too (`kill`), or the terminal keeps a hidden cursor.
    for sig in [libc::SIGINT, libc::SIGTERM] {
        unsafe {
            libc::signal(sig, on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(real_main(&args));
}

fn real_main(args: &[String]) -> i32 {
    let mut inv = match cli::parse(args) {
        Ok(inv) => inv,
        Err(msg) => return report(Fail::usage(msg)),
    };
    // No command at all, typed in a terminal: the interactive queue screen.
    if args.iter().all(|a| a == "-q" || a == "--quiet") && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        inv.cmd = Command::Screen;
    }
    // `uzu watch` in a terminal is the same screen; piped, it stays the live status block.
    if inv.cmd == Command::Watch && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        inv.cmd = Command::Screen;
    }
    match execute(&inv) {
        Ok(()) => 0,
        Err(fail) => report(fail),
    }
}

fn report(fail: Fail) -> i32 {
    if !fail.msg.is_empty() {
        eprintln!("{}", fail.msg);
    }
    fail.code
}

fn execute(inv: &Invocation) -> Result<(), Fail> {
    if inv.cmd == Command::Help {
        println!("{}", cli::HELP);
        return Ok(());
    }
    if inv.cmd == Command::Version {
        println!("uzu {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if inv.cmd == Command::CoverInfo {
        println!("{}", tui::cover_info()?);
        return Ok(());
    }
    let env = Env { term: Term::detect(), timing: Timing::default(), stop: &STOP, last: Some(uzumaki_ctl::last_path()) };
    // Not locked for the whole run: the screens' worker threads may write too (a held lock
    // made the cover's terminal query wait until uzu quit).
    let mut out = std::io::stdout();
    match inv.cmd {
        Command::Start => return start(inv, &env, &mut out),
        Command::Quit => return quit(&uzumaki_ctl::socket_path(), boot::QUIT_WAIT, &mut out).map(|_| ()),
        Command::Restart => return restart(inv, &env, &mut out),
        _ => {}
    }
    if matches!(inv.cmd, Command::Watch | Command::Screen) {
        install_sigint();
    }
    let result = match Client::connect() {
        Ok(client) => dispatch(inv, client, &env, &mut out),
        Err(_) => start_then_run(inv, &env, &mut out),
    };
    let _ = out.flush();
    result
}

fn dispatch(inv: &Invocation, mut client: Client, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    if inv.cmd == Command::Screen { tui::run(client, env) } else { run::run(inv, &mut client, env, out) }
}

/// The app isn't running: start it hidden, then run the command as `boot::plan` says.
fn start_then_run(inv: &Invocation, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    let cache = cache::default_dir().filter(|d| d.is_dir());
    let plan = boot::plan(&inv.cmd, cache.is_some());
    match boot::boot(plan, inv.quiet)? {
        Some(client) => dispatch(inv, client, env, out),
        None => match cache {
            Some(dir) => run::run(inv, &mut Offline { dir, settings: cache::default_settings() }, env, out),
            None => Err(Fail::verbatim(run::NOT_RUNNING)),
        },
    }
}

// ------------------------------------------------------------------------------------------ start

/// Starts the app detached (its own session, no stdio) and waits for its socket.
fn start(inv: &Invocation, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    if let Ok(mut client) = Client::connect() {
        let _ = writeln!(out, "Uzumaki is already running.");
        return run::run(&status_only(inv), &mut client, env, out);
    }
    boot::spawn_app()?;
    let _ = writeln!(out, "Starting Uzumaki…");
    let mut client = boot::wait_socket(boot::SOCKET_WAIT)?;
    run::run(&status_only(inv), &mut client, env, out)
}

/// Quits the app at `socket` and waits for it to stop accepting connections. Never starts it.
/// Returns whether it was running.
fn quit(socket: &Path, wait: Duration, out: &mut dyn Write) -> Result<bool, Fail> {
    let Ok(mut client) = Client::connect_to(socket) else {
        let _ = writeln!(out, "Uzumaki isn't running.");
        return Ok(false);
    };
    match client.request(&Request::Quit) {
        Ok(Reply::Error { message }) => return Err(Fail::err(message)),
        Ok(_) => {}
        Err(e) => return Err(Fail::err(format!("lost the connection to Uzumaki ({e})"))),
    }
    drop(client);
    if !boot::wait_gone(socket, wait) {
        return Err(Fail::err(format!("Uzumaki is still running {} seconds after quit was asked", wait.as_secs())));
    }
    let _ = writeln!(out, "Uzumaki quit.");
    Ok(true)
}

/// Quits if running, then starts hidden and prints the status (just `uzu start` when not running).
fn restart(inv: &Invocation, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    if !inv.quiet {
        eprintln!("Restarting Uzumaki…");
    }
    quit(&uzumaki_ctl::socket_path(), boot::QUIT_WAIT, &mut std::io::sink())?;
    boot::spawn_app()?;
    let mut client = boot::wait_socket(boot::SOCKET_WAIT)?;
    boot::wait_connected(&mut client, boot::BRIDGE_WAIT);
    run::run(&status_only(inv), &mut client, env, out)
}

fn status_only(inv: &Invocation) -> Invocation {
    Invocation { cmd: Command::Status, quiet: inv.quiet }
}
