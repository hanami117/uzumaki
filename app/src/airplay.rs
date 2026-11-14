//! AirPlay in the app: the one `uzumaki_airplay::Controller` of the process, and what is kept
//! between runs (`airplay.json` in the config folder, beside `settings.json`).
//!
//! The controller does nothing until the picker opens or a speaker is chosen (`state/airplay.rs`
//! drives it; `ui/airplay.rs` is the picker). Headless runs and tests have none.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use uzumaki_airplay::{Config, Controller, Event, LocalFeed};

/// What is kept between runs. Its own file (mode 0600): it holds the pairings of the user's
/// Apple TVs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Settings → Playback → "Reconnect to last AirPlay device".
    pub reconnect: bool,
    /// The speaker in use when the app last quit (None: this computer).
    pub last: Option<Last>,
    /// The speaker volume last chosen, 0-100.
    pub volume: Option<u8>,
    /// Makes this install's AirPlay identity (the DACP id a pairing is bound to).
    pub seed: Option<u64>,
    /// HomeKit pairings by device id (`Event::Paired`).
    pub pairings: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Last {
    pub id: String,
    pub name: String,
}

/// The speaker volume of a first connection: quiet enough to never startle.
pub const DEFAULT_VOLUME: u8 = 30;

fn path() -> Option<PathBuf> {
    Some(crate::settings::config_dir()?.join("airplay.json"))
}

impl Prefs {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Writes the file on a background thread (small, rare: a device or a volume was chosen).
    pub fn save(&self) {
        let copy = self.clone();
        std::thread::spawn(move || copy.write());
    }

    /// Writes the file now (atomically, mode 0600).
    fn write(&self) {
        static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let Some(path) = path() else { return };
        let Ok(json) = serde_json::to_vec_pretty(self) else { return };
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    pub fn volume(&self) -> u8 {
        self.volume.unwrap_or(DEFAULT_VOLUME).min(100)
    }
}

static CONTROLLER: OnceLock<Controller> = OnceLock::new();

/// Creates the controller (once, at launch; nothing runs yet) and returns it. `sink` gets its
/// events on the controller's threads.
pub fn init(sink: impl Fn(Event) + Send + Sync + 'static) -> &'static Controller {
    CONTROLLER.get_or_init(|| {
        // First run: this install's AirPlay identity, written before anything else reads the file.
        let mut prefs = Prefs::load();
        let seed = match prefs.seed {
            Some(s) => s,
            None => {
                let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
                let seed = nanos ^ (u64::from(std::process::id()) << 32);
                prefs.seed = Some(seed);
                prefs.write();
                seed
            }
        };
        let cfg = Config::from_env(seed);
        match &cfg.helper {
            Some(p) => eprintln!("airplay: ready (helper {}); nothing runs until a speaker is chosen", p.display()),
            None => eprintln!("airplay: no cliairplay helper found; AirPlay is off (no button, no speaker search)"),
        }
        // Every way of quitting reaches atexit (Cmd+Q included): the speaker is told to stop
        // and the helper ends with the app.
        extern "C" fn at_exit() {
            shutdown();
        }
        // SAFETY: registers a plain function; it only touches this module's static.
        if unsafe { libc::atexit(at_exit) } != 0 {
            eprintln!("airplay: could not register the exit handler");
        }
        Controller::new(cfg, sink)
    })
}

pub fn controller() -> Option<&'static Controller> {
    CONTROLLER.get()
}

/// The environment the bridge needs: where the PCM socket will be (it exists only while a
/// speaker is connected, and the bridge looks for it only when told to capture).
pub fn bridge_env() -> Vec<(&'static str, OsString)> {
    match controller() {
        Some(c) => vec![("UZUMAKI_AIRPLAY_PCM_SOCKET", c.socket().into_os_string())],
        None => Vec::new(),
    }
}

/// The local player's way to the speaker. None in headless runs and tests.
pub fn local_feed() -> Option<LocalFeed> {
    controller().map(Controller::local_feed)
}

/// Quit: stops the speaker and its helper.
pub fn shutdown() {
    if let Some(c) = controller() {
        c.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_and_empty_files_read_as_defaults() {
        let p: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Prefs::default());
        assert!(!p.reconnect, "reconnecting is opt-in");
        assert_eq!(p.volume(), DEFAULT_VOLUME);
        let p: Prefs = serde_json::from_str(r#"{"reconnect":true,"last":{"id":"aa","name":"Bedroom"},"volume":250,"future":1}"#).unwrap();
        assert!(p.reconnect);
        assert_eq!(p.last, Some(Last { id: "aa".into(), name: "Bedroom".into() }));
        assert_eq!(p.volume(), 100);
    }
}
