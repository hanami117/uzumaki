//! Where Uzumaki keeps things on Windows: the user's AppData folders.
//!
//! Linux and macOS use XDG paths (`~/.config/uzumaki`, `~/.cache/uzumaki`, `~/.local/state/uzumaki`,
//! `~/.local/share/uzumaki`); each place that needs one works it out itself, as it always did, and
//! none of that code changed. This module is the Windows half, shared by the app and `uzu`:
//!
//! | what | folder |
//! |---|---|
//! | settings, `uzu.toml` | `%APPDATA%\Uzumaki` |
//! | data that is not a cache (playlist pictures) | `%APPDATA%\Uzumaki\Data` |
//! | caches (covers, library, local covers) | `%LOCALAPPDATA%\Uzumaki\Cache` |
//! | what the app remembers by itself | `%LOCALAPPDATA%\Uzumaki\State` |
//! | logs | `%LOCALAPPDATA%\Uzumaki\Logs` |
//! | the playback profile (the Apple Music sign-in) | `%LOCALAPPDATA%\Uzumaki\Bridge` |
//!
//! The profile is hundreds of MB, so it is never in the roaming folder. An absolute `XDG_*`
//! variable still wins on Windows, so a test instance can have scratch folders on every system.
//! The rule itself ([`windows_dir`]) takes its environment as an argument and is tested everywhere.

use std::ffi::OsString;
use std::path::PathBuf;

/// Which folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Config,
    Data,
    Cache,
    State,
    Logs,
    Bridge,
}

/// The Windows rule, with the environment passed in. `temp` is the last resort when neither the
/// AppData variables nor `USERPROFILE` are set: a real folder, never the working directory.
pub fn windows_dir(kind: Kind, env: &dyn Fn(&str) -> Option<OsString>, temp: PathBuf) -> PathBuf {
    let abs = |var: &str| env(var).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| is_absolute(p));
    let xdg = match kind {
        Kind::Config | Kind::Bridge => "XDG_CONFIG_HOME",
        Kind::Data => "XDG_DATA_HOME",
        Kind::Cache => "XDG_CACHE_HOME",
        Kind::State | Kind::Logs => "XDG_STATE_HOME",
    };
    if let Some(p) = abs(xdg) {
        let dir = p.join("uzumaki");
        return match kind {
            Kind::Logs => dir.join("Logs"),
            Kind::Bridge => dir.join("bridge"),
            _ => dir,
        };
    }
    let (var, under_profile) = match kind {
        Kind::Config | Kind::Data => ("APPDATA", "AppData\\Roaming"),
        _ => ("LOCALAPPDATA", "AppData\\Local"),
    };
    let base = abs(var).or_else(|| abs("USERPROFILE").map(|h| h.join(under_profile))).unwrap_or_else(|| temp.join("uzumaki-profile"));
    let dir = base.join("Uzumaki");
    match kind {
        Kind::Config => dir,
        Kind::Data => dir.join("Data"),
        Kind::Cache => dir.join("Cache"),
        Kind::State => dir.join("State"),
        Kind::Logs => dir.join("Logs"),
        Kind::Bridge => dir.join("Bridge"),
    }
}

/// Absolute by Windows' rules or by Unix's: the tests of [`windows_dir`] run on every system, where
/// `C:\x` is not absolute to `Path`.
fn is_absolute(p: &std::path::Path) -> bool {
    if p.is_absolute() {
        return true;
    }
    let b = p.as_os_str().as_encoded_bytes();
    b.starts_with(b"\\\\") || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
}

/// The folder of `kind` for this user (Windows only: see the module text).
#[cfg(windows)]
pub fn dir(kind: Kind) -> PathBuf {
    windows_dir(kind, &|v| std::env::var_os(v), std::env::temp_dir())
}

/// The user's home folder: `HOME`, on Windows `USERPROFILE`.
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Windows: the state folder, where `uzu` keeps its small records.
#[cfg(windows)]
pub fn state_dir() -> PathBuf {
    dir(Kind::State)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v))
    }

    fn at(kind: Kind, vars: &[(&str, &str)]) -> String {
        windows_dir(kind, &env(vars), PathBuf::from("T:\\Temp")).to_string_lossy().replace('/', "\\")
    }

    #[test]
    fn the_windows_folders() {
        let pc = [("APPDATA", "C:\\Users\\u\\AppData\\Roaming"), ("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local"), ("USERPROFILE", "C:\\Users\\u")];
        assert_eq!(at(Kind::Config, &pc), "C:\\Users\\u\\AppData\\Roaming\\Uzumaki");
        assert_eq!(at(Kind::Data, &pc), "C:\\Users\\u\\AppData\\Roaming\\Uzumaki\\Data");
        assert_eq!(at(Kind::Cache, &pc), "C:\\Users\\u\\AppData\\Local\\Uzumaki\\Cache");
        assert_eq!(at(Kind::State, &pc), "C:\\Users\\u\\AppData\\Local\\Uzumaki\\State");
        assert_eq!(at(Kind::Logs, &pc), "C:\\Users\\u\\AppData\\Local\\Uzumaki\\Logs");
        // The sign-in profile is never in the roaming folder.
        assert_eq!(at(Kind::Bridge, &pc), "C:\\Users\\u\\AppData\\Local\\Uzumaki\\Bridge");
    }

    #[test]
    fn the_profile_folder_stands_in_for_missing_variables() {
        let bare = [("USERPROFILE", "C:\\Users\\u")];
        assert_eq!(at(Kind::Config, &bare), "C:\\Users\\u\\AppData\\Roaming\\Uzumaki");
        assert_eq!(at(Kind::Cache, &bare), "C:\\Users\\u\\AppData\\Local\\Uzumaki\\Cache");
        // Nothing set, or only relative text: a temp folder, never the working directory.
        assert_eq!(at(Kind::Config, &[]), "T:\\Temp\\uzumaki-profile\\Uzumaki");
        assert_eq!(at(Kind::State, &[("LOCALAPPDATA", "Local"), ("USERPROFILE", "")]), "T:\\Temp\\uzumaki-profile\\Uzumaki\\State");
    }

    #[test]
    fn an_absolute_xdg_variable_wins() {
        let scratch = [("XDG_CONFIG_HOME", "D:\\scratch\\c"), ("XDG_CACHE_HOME", "D:\\scratch\\k"), ("XDG_STATE_HOME", "D:\\scratch\\s"), ("XDG_DATA_HOME", "D:\\scratch\\d"), ("APPDATA", "C:\\A"), ("LOCALAPPDATA", "C:\\L")];
        assert_eq!(at(Kind::Config, &scratch), "D:\\scratch\\c\\uzumaki");
        assert_eq!(at(Kind::Bridge, &scratch), "D:\\scratch\\c\\uzumaki\\bridge");
        assert_eq!(at(Kind::Cache, &scratch), "D:\\scratch\\k\\uzumaki");
        assert_eq!(at(Kind::State, &scratch), "D:\\scratch\\s\\uzumaki");
        assert_eq!(at(Kind::Logs, &scratch), "D:\\scratch\\s\\uzumaki\\Logs");
        assert_eq!(at(Kind::Data, &scratch), "D:\\scratch\\d\\uzumaki");
        // A relative one is ignored.
        assert_eq!(at(Kind::Config, &[("XDG_CONFIG_HOME", "c"), ("APPDATA", "C:\\A")]), "C:\\A\\Uzumaki");
    }
}
