//! Settings → General → "Open at login" and "Open in the background".
//!
//! The switch shows what the system holds, read back at start (`read`), not a saved bool:
//! - Linux (`login_linux.rs`): an XDG autostart entry, `~/.config/autostart/<app id>.desktop`,
//!   whose `Exec` runs this copy (the AppImage file, the packaged binary or a source install),
//!   with `--hidden` when "Start in the background" is on.
//! - Windows (`login_win.rs`): a value under the current user's Run key in the registry, with
//!   `--hidden` when "Start in the background" is on; Windows' own switch for it (Task Manager,
//!   Settings → Apps → Startup) is read too.
//! - macOS (`login_mac.rs`): the app itself as a login item (`SMAppService.mainApp`). A login
//!   item gets no arguments, so the background start is decided at launch: with both switches
//!   on the app starts windowless and opens its window at once unless macOS says this launch is
//!   the login one (`watch_launch`).
//!
//! Every function here touches the real system: callers keep them out of headless runs, tests
//! and test instances (`State::set_start_at_login` and `main`).

#[cfg(target_os = "linux")]
#[path = "login_linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "login_mac.rs"]
mod imp;

#[cfg(windows)]
#[path = "login_win.rs"]
mod imp;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub use imp::*;

// The autostart entry's text is plain std: its tests run on the Mac too (not on Windows: they
// are about Unix paths).
#[cfg(all(test, unix, not(target_os = "linux")))]
#[path = "login_linux.rs"]
#[allow(dead_code)]
mod linux_entry;

/// What the system says about starting at login.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Login {
    /// Why this copy can't start at login (shown under the disabled switch), or None.
    pub unavailable: Option<&'static str>,
    /// Starts at login (Linux: the autostart entry exists and isn't turned off; macOS: the login
    /// item is registered, approved or not).
    pub on: bool,
    /// Linux and Windows: the entry starts with `--hidden`. None where the system doesn't say (macOS, or no entry).
    pub hidden: Option<bool>,
    /// macOS: registered, but waiting for the user's OK in System Settings → Login Items.
    pub needs_approval: bool,
}

impl Login {
    pub fn unavailable(why: &'static str) -> Self {
        Login { unavailable: Some(why), ..Login::default() }
    }

    /// The switch can be used.
    pub fn available(&self) -> bool {
        self.unavailable.is_none()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod other {
    use super::Login;

    pub fn read() -> Login {
        Login::unavailable("Not available on this system.")
    }

    pub fn set(_on: bool, _hidden: bool) -> Result<Login, String> {
        Err("not available on this system".into())
    }

    pub fn repair() {}
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub use other::*;
