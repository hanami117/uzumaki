//! The tray icon. Each system has its own native one, all with the same menu and the same
//! `TrayCtl` API:
//! - Linux (and other Unix desktops): a StatusNotifierItem with a `com.canonical.dbusmenu` menu
//!   (`tray_linux.rs`, through ksni);
//! - Windows: a notification-area icon with a Win32 popup menu (`tray_win.rs`);
//! - macOS: a menu bar item (`tray_mac.rs`).
//!
//! Linux and Windows draw the menu from `tray_menu.rs`. `show_window` is the one way to bring
//! the window back, because on the Mac close to tray also hides the Dock icon.

#[cfg(all(unix, not(target_os = "macos")))]
#[path = "tray_linux.rs"]
mod imp;
#[cfg(windows)]
#[path = "tray_win.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "tray_mac.rs"]
mod imp;

#[cfg(any(not(target_os = "macos"), test))]
#[path = "tray_menu.rs"]
pub mod menu;

pub use imp::TrayCtl;

use crate::state::Handle;

/// Shows the window and, on macOS, the Dock icon that close to tray hid. The dock request is
/// applied before the window one, so the window opens into a regular app.
pub fn show_window(handle: &Handle) {
    #[cfg(target_os = "macos")]
    handle.set_dock_icon(true);
    handle.open();
}

/// "Title — Artist", the title alone, or empty with no song: the tooltip and the menu's first line.
#[cfg(not(target_os = "macos"))]
pub fn song_line(song: Option<&crate::models::Song>) -> String {
    match song {
        Some(s) if !s.artist.is_empty() => format!("{} — {}", s.title, s.artist),
        Some(s) => s.title.clone(),
        None => String::new(),
    }
}

/// The tray drawing of Linux and Windows, from the chosen app icon.
#[cfg(any(not(target_os = "macos"), test))]
pub mod icon {
    use crate::settings::IconVariant;

    /// The side the PNGs are drawn at; Linux hands this over and hosts scale it.
    #[cfg_attr(windows, allow(dead_code))]
    pub const ICON_PX: u32 = 64;

    /// The chosen app icon's small drawing (`assets/icons/variants/<name>/tray.png`, 64 px; the
    /// dark one is the original `uzumaki-tray.png`).
    pub fn tray_png(app_icon: IconVariant) -> &'static [u8] {
        match app_icon {
            IconVariant::Dark => include_bytes!("../../assets/icons/variants/dark/tray.png"),
            IconVariant::Light => include_bytes!("../../assets/icons/variants/light/tray.png"),
            IconVariant::Coral => include_bytes!("../../assets/icons/variants/coral/tray.png"),
        }
    }

    /// Straight RGBA rows of the icon, `px` pixels a side (decoded from the PNG built into the binary).
    pub fn icon_pixels(app_icon: IconVariant, px: u32) -> Vec<u8> {
        match image::load_from_memory_with_format(tray_png(app_icon), image::ImageFormat::Png) {
            Ok(img) => img.resize_exact(px, px, image::imageops::FilterType::Lanczos3).to_rgba8().into_raw(),
            Err(_) => vec![0; (px * px * 4) as usize],
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_icon_is_a_coral_swirl_on_a_dark_rounded_tile() {
            let rgba = icon_pixels(IconVariant::Dark, ICON_PX);
            assert_eq!(rgba.len(), (ICON_PX * ICON_PX * 4) as usize);
            let px = |x: u32, y: u32| {
                let i = ((y * ICON_PX + x) * 4) as usize;
                [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
            };
            assert_eq!(px(0, 0)[3], 0, "rounded corner is transparent");
            assert_eq!(px(63, 63)[3], 0, "rounded corner is transparent");
            let tile = px(10, 32);
            assert!(tile[3] == 255 && tile[0] < 0x40, "dark tile at the left edge, got {tile:?}");
            let coral = (0..64).flat_map(|y| (0..64).map(move |x| (x, y))).filter(|&(x, y)| {
                let p = px(x, y);
                p[3] == 255 && p[0] > 0xf0 && p[1] > 0x60 && p[1] < 0x90
            });
            assert!(coral.count() > 200, "a coral swirl");
        }

        #[test]
        fn each_app_icon_has_its_own_tray_image() {
            let tile = |i: IconVariant| {
                let rgba = icon_pixels(i, ICON_PX);
                let at = ((32 * ICON_PX + 10) * 4) as usize; // left edge of the tile
                [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
            };
            assert!(tile(IconVariant::Light)[0] > 0xf0 && tile(IconVariant::Light)[2] > 0xf0, "white tile");
            assert!(tile(IconVariant::Coral)[0] > 0xf0 && tile(IconVariant::Coral)[1] > 0xc0 && tile(IconVariant::Coral)[1] < 0xe0, "pale coral tile");
            assert_eq!(tray_png(IconVariant::Dark), include_bytes!("../../assets/icons/uzumaki-tray.png"));
        }

        /// Windows asks for the notification area's own size: the drawing comes out at any of them.
        #[test]
        fn the_icon_is_drawn_at_the_size_asked_for() {
            for px in [16u32, 20, 24, 32] {
                let rgba = icon_pixels(IconVariant::Dark, px);
                assert_eq!(rgba.len(), (px * px * 4) as usize);
                // Scaling leaves a trace of the tile's edge in the corner pixel, never more.
                assert!(rgba[3] < 48, "the rounded corner stays clear at {px} px, got alpha {}", rgba[3]);
                let mid = ((px / 2 * px + px / 4) * 4) as usize;
                assert!(rgba[mid + 3] > 200, "the tile is solid at {px} px, got alpha {}", rgba[mid + 3]);
            }
        }
    }
}
