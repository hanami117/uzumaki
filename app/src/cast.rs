//! Chromecast in the app: the one `uzumaki_cast::Controller` of the process. It is created idle
//! (no thread, no mDNS, no server) in windowed runs; headless runs and tests have none.
//!
//! Settings → Speakers → Chromecast (off by default) decides whether it is used at all: while
//! off, the picker shows no Cast devices, nothing looks for them and `uzu cast` says so
//! (`state/cast.rs`). `UZUMAKI_CAST_SPIKE=1` turns it on regardless, for development.

use std::sync::OnceLock;

use uzumaki_cast::{Config, Controller, Event, LocalFeed};

static CONTROLLER: OnceLock<Controller> = OnceLock::new();

pub const OFF: &str = "Chromecast is off (Settings → Speakers → Chromecast)";

/// Unix only for now: Apple Music's audio reaches the session over a Unix socket. On Windows
/// the setting is hidden and Chromecast stays off.
pub const SUPPORTED: bool = cfg!(unix);

/// The development override: on whatever the setting says.
pub fn forced() -> bool {
    SUPPORTED && std::env::var("UZUMAKI_CAST_SPIKE").is_ok_and(|v| v.trim() == "1")
}

/// Local files the TV can play go to it as files (it plays them itself); `UZUMAKI_CAST_FILES=0`
/// sends them through the live stream like Apple Music.
pub fn files() -> bool {
    std::env::var("UZUMAKI_CAST_FILES").map_or(true, |v| v.trim() != "0")
}

/// Creates the controller (once, at launch; nothing runs until a device is looked for or picked).
pub fn init(sink: impl Fn(Event) + Send + Sync + 'static) -> &'static Controller {
    CONTROLLER.get_or_init(|| {
        let cfg = Config::from_env();
        extern "C" fn at_exit() {
            shutdown();
        }
        // SAFETY: registers a plain function; it only touches this module's static.
        if unsafe { libc::atexit(at_exit) } != 0 {
            eprintln!("cast: could not register the exit handler");
        }
        Controller::new(cfg, sink)
    })
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
