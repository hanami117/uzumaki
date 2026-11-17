//! macOS: the native chrome follows Uzumaki's Theme setting (Settings → Appearance). Forcing
//! Light while macOS is Dark (or the reverse) used to leave the traffic lights, native dialogs
//! (open panel, alerts), tooltips and AppKit's own menus in the computer's appearance. Setting
//! `NSApplication.appearance` fixes all of them at once: Light is Aqua, Dark is DarkAqua, System
//! is none, so AppKit follows the computer again.
//!
//! A forced appearance also changes what the window reports as "the OS appearance" (winit reads
//! the window's effective appearance, which is now ours). `os_dark` is the real one: the app's
//! own effective appearance while System, the global `AppleInterfaceStyle` while forced. It
//! feeds the "System (Dark)" label, Follow theme for the app icon, and the palette itself.
//! Nothing headless; Linux has no native chrome to follow.

use crate::settings::Theme;

/// The two AppKit appearances Uzumaki ever asks for. System is "none" (no override).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Look {
    Aqua,
    DarkAqua,
}

/// The appearance to force for a theme setting; None leaves AppKit on the computer's.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn look(theme: Theme) -> Option<Look> {
    match theme {
        Theme::Light => Some(Look::Aqua),
        Theme::Dark => Some(Look::DarkAqua),
        Theme::System => None,
    }
}

/// `AppleInterfaceStyle` is "Dark" in dark mode and missing in light mode.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn style_is_dark(style: Option<&str>) -> bool {
    style.is_some_and(|s| s.eq_ignore_ascii_case("dark"))
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{look, style_is_dark, Look, Theme};
    use objc2::{rc::Retained, MainThreadMarker};
    use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication};
    use objc2_foundation::{ns_string, NSArray, NSUserDefaults};
    use std::cell::Cell;

    thread_local! {
        /// The override last applied (`None` inside: System), so a frame does nothing unless it changed.
        static APPLIED: Cell<Option<Option<Look>>> = const { Cell::new(None) };
    }

    fn app() -> Option<Retained<NSApplication>> {
        Some(NSApplication::sharedApplication(MainThreadMarker::new()?))
    }

    pub fn sync(theme: Theme) {
        if rune::is_headless() {
            return;
        }
        let want = look(theme);
        if APPLIED.get() == Some(want) {
            return;
        }
        let Some(app) = app() else { return };
        // SAFETY: the appearance names are AppKit's own constants.
        let appearance = want.and_then(|l| unsafe {
            NSAppearance::appearanceNamed(match l {
                Look::Aqua => NSAppearanceNameAqua,
                Look::DarkAqua => NSAppearanceNameDarkAqua,
            })
        });
        app.setAppearance(appearance.as_deref());
        APPLIED.set(Some(want));
    }

    /// The app's effective appearance is dark. Right after `sync(System)` it is the computer's.
    fn app_dark() -> Option<bool> {
        let app = app()?;
        // SAFETY: the appearance names are AppKit's own constants.
        let names = unsafe { NSArray::from_slice(&[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]) };
        let best = app.effectiveAppearance().bestMatchFromAppearancesWithNames(&names)?;
        // SAFETY: as above.
        Some(unsafe { best.isEqualToString(NSAppearanceNameDarkAqua) })
    }

    fn style_dark() -> bool {
        let style = NSUserDefaults::standardUserDefaults().stringForKey(ns_string!("AppleInterfaceStyle"));
        style_is_dark(style.map(|s| s.to_string()).as_deref())
    }

    pub fn os_dark(theme: Theme, reported: bool) -> bool {
        if rune::is_headless() {
            return reported;
        }
        match theme {
            Theme::System => app_dark().unwrap_or(reported),
            Theme::Light | Theme::Dark => style_dark(),
        }
    }
}

/// Puts the native chrome in the theme's appearance; cheap when nothing changed. Call it from
/// the UI thread before the frame reads the OS appearance (`ui::view`, first thing) and once at
/// start, before the window exists.
#[cfg(target_os = "macos")]
pub fn sync(theme: Theme) {
    imp::sync(theme);
}
#[cfg(not(target_os = "macos"))]
pub fn sync(_theme: Theme) {}

/// The computer's real light/dark appearance. `reported` is `Cx::system_dark`, which is what
/// the window says and is wrong while a theme is forced; it is the answer everywhere but macOS.
#[cfg(target_os = "macos")]
pub fn os_dark(theme: Theme, reported: bool) -> bool {
    imp::os_dark(theme, reported)
}
#[cfg(not(target_os = "macos"))]
pub fn os_dark(_theme: Theme, reported: bool) -> bool {
    reported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_picks_the_appearance_and_system_picks_none() {
        assert_eq!(look(Theme::Light), Some(Look::Aqua));
        assert_eq!(look(Theme::Dark), Some(Look::DarkAqua));
        assert_eq!(look(Theme::System), None, "System must clear the override so AppKit follows the computer");
    }

    #[test]
    fn the_global_style_says_dark_only_for_dark() {
        assert!(style_is_dark(Some("Dark")));
        assert!(!style_is_dark(None), "light mode has no AppleInterfaceStyle");
        assert!(!style_is_dark(Some("")));
    }

    #[test]
    fn off_macos_the_window_report_is_the_answer() {
        if !cfg!(target_os = "macos") {
            assert!(os_dark(Theme::Light, true));
            assert!(!os_dark(Theme::Dark, false));
        }
    }
}
