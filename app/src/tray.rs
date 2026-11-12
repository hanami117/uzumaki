//! The tray icon: Linux has Rune's StatusNotifierItem tray (`tray_linux.rs`), macOS a menu bar
//! item of its own (`tray_mac.rs`, since Rune has no Mac tray). Both give `TrayCtl` the same API.
//! `show_window` is the one way to bring the window back, because on the Mac close to tray also
//! hides the Dock icon.

#[cfg(not(target_os = "macos"))]
#[path = "tray_linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "tray_mac.rs"]
mod imp;

pub use imp::TrayCtl;

use crate::state::Handle;

/// Shows the window and, on macOS, the Dock icon that close to tray hid. The dock request is
/// applied before the window one, so the window opens into a regular app.
pub fn show_window(handle: &Handle) {
    #[cfg(target_os = "macos")]
    handle.set_dock_icon(true);
    handle.open();
}
