//! Chromecast spike: the one `uzumaki_cast::Controller` of the process, only when the app was
//! started with `UZUMAKI_CAST_SPIKE=1`. Without it nothing exists: no thread, no mDNS, no server,
//! and `uzu cast` says Chromecast is off. Driven by `uzu cast` (`state/cast.rs`); no picker row yet.

use std::sync::OnceLock;

use uzumaki_cast::{Config, Controller, Event, LocalFeed};

static CONTROLLER: OnceLock<Controller> = OnceLock::new();

pub const OFF: &str = "Chromecast is off in this app (start it with UZUMAKI_CAST_SPIKE=1)";

/// The opt-in switch.
pub fn wanted() -> bool {
    std::env::var("UZUMAKI_CAST_SPIKE").is_ok_and(|v| v.trim() == "1")
}

/// Creates the controller when the switch is on (once, at launch; nothing runs yet).
pub fn init(sink: impl Fn(Event) + Send + Sync + 'static) -> Option<&'static Controller> {
    if !wanted() {
        return None;
    }
    Some(CONTROLLER.get_or_init(|| {
        let cfg = Config::from_env();
        eprintln!("cast: spike on (UZUMAKI_CAST_SPIKE=1), {} stream; nothing runs until `uzu cast <name>`", cfg.format.ext());
        extern "C" fn at_exit() {
            shutdown();
        }
        // SAFETY: registers a plain function; it only touches this module's static.
        if unsafe { libc::atexit(at_exit) } != 0 {
            eprintln!("cast: could not register the exit handler");
        }
        Controller::new(cfg, sink)
    }))
}

pub fn controller() -> Option<&'static Controller> {
    CONTROLLER.get()
}

pub fn local_feed() -> Option<LocalFeed> {
    controller().map(Controller::local_feed)
}

pub fn shutdown() {
    if let Some(c) = controller() {
        c.shutdown();
    }
}
