//! The app was called Cadence before it became Uzumaki. Its data lived under `cadence/` in the
//! config folder (settings and the Electron profile holding the Apple Music login) and in the
//! cache folder. On the first start after the rename it moves over, so nothing is lost and the
//! user stays signed in. `main.rs` runs this only when no old copy is running.

use std::path::{Path, PathBuf};

const OLD: &str = "cadence";
const NEW: &str = "uzumaki";

/// `$XDG_CONFIG_HOME` (else `~/.config`): where settings and the Electron profile live.
fn config_base() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

/// `$XDG_CACHE_HOME` (else `~/.cache`).
fn cache_base() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
}

/// Moves `cadence/` to `uzumaki/` in the config and cache folders, where only the old one exists.
pub fn migrate() {
    for base in [config_base(), cache_base()].into_iter().flatten() {
        if let Err(e) = move_dir(&base) {
            eprintln!("uzumaki: couldn't move {} to {}: {e}", base.join(OLD).display(), base.join(NEW).display());
        }
    }
}

fn move_dir(base: &Path) -> std::io::Result<bool> {
    let (old, new) = (base.join(OLD), base.join(NEW));
    if !old.is_dir() || new.exists() {
        return Ok(false);
    }
    std::fs::rename(&old, &new)?;
    eprintln!("uzumaki: moved {} to {}", old.display(), new.display());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_the_old_folder_once_and_never_over_a_new_one() {
        let base = std::env::temp_dir().join(format!("uzumaki-legacy-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("cadence/bridge")).unwrap();
        std::fs::write(base.join("cadence/settings.json"), "{}").unwrap();
        assert!(move_dir(&base).unwrap());
        assert!(base.join("uzumaki/bridge").is_dir() && base.join("uzumaki/settings.json").is_file());
        assert!(!base.join("cadence").exists());
        // Both exist (say, the old build ran again): leave both alone.
        std::fs::create_dir_all(base.join("cadence")).unwrap();
        assert!(!move_dir(&base).unwrap());
        assert!(base.join("cadence").is_dir() && base.join("uzumaki/settings.json").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }
}
