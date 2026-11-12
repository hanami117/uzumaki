//! macOS: an app opened from the Finder, the Dock or Spotlight gets /dev/null as its stderr, so
//! every `eprintln!` is lost. Send stderr to `~/Library/Logs/Uzumaki/uzumaki.log` instead
//! (Console.app lists that folder). Started from a terminal, or with stderr sent anywhere but
//! /dev/null, stderr stays where it is.

use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Over this size the log moves to `uzumaki.log.1` at the next start (one old copy is kept).
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Call once, early in `main`, before other threads start.
pub fn redirect_stderr() {
    if !stderr_is_null() {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else { return };
    let Some(file) = open_log(&Path::new(&home).join("Library/Logs/Uzumaki")) else { return };
    // SAFETY: both descriptors are open; after this fd 2 is the log file. `file` closes only
    // its own descriptor when it drops.
    if unsafe { libc::dup2(file.as_raw_fd(), 2) } >= 0 {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        eprintln!("--- uzumaki {} started, pid {}, unix time {secs}", uzumaki_ctl::version_line(), std::process::id());
    }
}

/// True when fd 2 is /dev/null, as for an app opened from the Finder or the Dock.
fn stderr_is_null() -> bool {
    match (fs::metadata("/dev/fd/2"), fs::metadata("/dev/null")) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino(), a.rdev()) == (b.dev(), b.ino(), b.rdev()),
        _ => false,
    }
}

/// Opens `dir/uzumaki.log` for appending, after moving a log over `MAX_BYTES` to `uzumaki.log.1`.
fn open_log(dir: &Path) -> Option<File> {
    fs::create_dir_all(dir).ok()?;
    let path = dir.join("uzumaki.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(&path, dir.join("uzumaki.log.1"));
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_appends_and_rotates() {
        let dir = std::env::temp_dir().join(format!("uzumaki-logfile-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        use std::io::Write;
        writeln!(open_log(&dir).unwrap(), "one").unwrap();
        writeln!(open_log(&dir).unwrap(), "two").unwrap();
        assert_eq!(fs::read_to_string(dir.join("uzumaki.log")).unwrap(), "one\ntwo\n");
        assert!(!dir.join("uzumaki.log.1").exists());

        fs::write(dir.join("uzumaki.log"), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        writeln!(open_log(&dir).unwrap(), "fresh").unwrap();
        assert_eq!(fs::read_to_string(dir.join("uzumaki.log")).unwrap(), "fresh\n");
        assert_eq!(fs::metadata(dir.join("uzumaki.log.1")).unwrap().len(), MAX_BYTES + 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
