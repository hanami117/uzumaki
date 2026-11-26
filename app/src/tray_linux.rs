//! The system tray icon (StatusNotifierItem through Rune; on Windows, Rune's notification-area icon): Play/Pause, Previous, Next,
//! Shuffle, Repeat and AutoPlay, Show and Quit, plus the current song as the tooltip. Rune's menu items have no checkmarks or
//! submenus, so the three modes are plain items that say their state ("Shuffle: On") and toggle it (Repeat cycles Off, All, One). A left click shows the window.
//!
//! The tray runs on its own thread; its events go to the UI thread with `handle.update`, so
//! they work while the window is closed to the tray (`App::keep_running`). It shows only where
//! a tray host runs (KDE, waybar and most bars; GNOME needs the AppIndicator extension).

use rune::tray::{TrayEvent, TrayIcon, TrayItem, TraySpec};
use rune::Tray;

use crate::models::Song;
use crate::settings::IconVariant;
use crate::state::tray_modes::Modes;
use crate::state::{Handle, Repeat, State};

/// Tray icon side in pixels; hosts scale it.
#[cfg_attr(windows, allow(dead_code))]
const ICON_PX: u32 = 64;

/// The side the icon is handed over at. Linux hosts scale the 64 px drawing themselves. Windows
/// draws exactly what it gets, so it gets the notification area's own size at the system's scale
/// (16 px at 100 %, 20 at 125 %, 24 at 150 %, 32 at 200 %).
fn icon_px() -> u32 {
    #[cfg(windows)]
    return crate::win::tray_icon_px();
    #[cfg(not(windows))]
    ICON_PX
}

pub struct TrayCtl {
    tray: Tray,
    playing: bool,
    tooltip: String,
    /// A song is shown (the window's player bar, and so its Shuffle and Repeat, exist).
    loaded: bool,
    modes: Option<Modes>,
}

impl TrayCtl {
    /// None when the platform has no tray (or D-Bus is unavailable).
    /// The icon is the chosen app icon's; Rune's tray can't swap its image once shown, so a
    /// change in Settings shows here at the next start.
    pub fn start(handle: Handle, app_icon: IconVariant) -> Option<Self> {
        let spec = TraySpec::new("uzumaki", "Uzumaki", icon(app_icon, icon_px())).tooltip("Uzumaki").menu(menu(false, None));
        let on_event = move |ev: TrayEvent| match ev {
            TrayEvent::Activate { token } => match token {
                Some(t) => handle.activate(t),
                None => handle.open(),
            },
            TrayEvent::Menu(id) => match id.as_str() {
                "play-pause" => handle.update(|s: &mut State, _| s.toggle_play()),
                "next" => handle.update(|s: &mut State, _| s.next()),
                "previous" => handle.update(|s: &mut State, _| s.previous()),
                "shuffle" => handle.update(|s: &mut State, _| s.toggle_shuffle()),
                "repeat" => handle.update(|s: &mut State, _| s.cycle_repeat()),
                "autoplay" => handle.update(|s: &mut State, _| s.toggle_autoplay()),
                "show" => handle.open(),
                // The answer shows in the window (the notice, or a toast).
                "check-updates" => {
                    handle.open();
                    handle.update(|s: &mut State, _| s.check_updates_now());
                }
                "quit" => handle.quit(),
                _ => {}
            },
            _ => {}
        };
        match Tray::new(spec, on_event) {
            Ok(tray) => Some(Self { tray, playing: false, tooltip: String::new(), loaded: false, modes: None }),
            Err(e) => {
                eprintln!("uzumaki: no tray icon: {e}");
                None
            }
        }
    }

    /// Rune's tray can't swap its image once shown: the new icon shows at the next start.
    pub fn set_app_icon(&mut self, _icon: IconVariant) {}

    /// Relabels Play/Pause.
    pub fn set_playing(&mut self, playing: bool) {
        if playing != self.playing {
            self.playing = playing;
            self.show_menu();
        }
    }

    /// Relabels Shuffle, Repeat and AutoPlay.
    pub fn set_modes(&mut self, modes: &Modes) {
        if self.modes.as_ref() != Some(modes) {
            self.modes = Some(*modes);
            self.show_menu();
        }
    }

    fn show_menu(&mut self) {
        self.tray.set_menu(menu(self.playing, self.modes.as_ref().map(|m| (m, self.loaded))));
    }

    /// The tooltip: "Title — Artist", or just "Uzumaki".
    pub fn set_song(&mut self, song: Option<&Song>) {
        let tip = match song {
            Some(s) if !s.artist.is_empty() => format!("{} — {}", s.title, s.artist),
            Some(s) => s.title.clone(),
            None => "Uzumaki".into(),
        };
        if tip != self.tooltip {
            self.tray.set_tooltip(tip.clone());
            self.tooltip = tip;
        }
        // The modes are greyed while nothing is loaded.
        if song.is_some() != self.loaded {
            self.loaded = song.is_some();
            self.show_menu();
        }
    }
}

/// `modes`: what Shuffle, Repeat and AutoPlay show, and whether a song is loaded.
fn menu(playing: bool, modes: Option<(&Modes, bool)>) -> Vec<TrayItem> {
    let (m, loaded) = modes.unwrap_or((&Modes { shuffle: false, repeat: Repeat::Off, autoplay: false, autoplay_offered: false }, false));
    vec![
        TrayItem::action("play-pause", if playing { "Pause" } else { "Play" }),
        TrayItem::action("previous", "Previous"),
        TrayItem::action("next", "Next"),
        TrayItem::Separator,
        TrayItem::action("shuffle", m.shuffle_text()).enabled(loaded),
        TrayItem::action("repeat", m.repeat_text()).enabled(loaded),
        TrayItem::action("autoplay", m.autoplay_text()).enabled(m.autoplay_enabled(loaded)),
        TrayItem::Separator,
        TrayItem::action("show", "Show Uzumaki"),
        TrayItem::action("check-updates", "Check for Updates\u{2026}"),
        TrayItem::Separator,
        TrayItem::action("quit", "Quit Uzumaki"),
    ]
}

/// The chosen app icon's small drawing (`assets/icons/variants/<name>/tray.png`, 64 px; the dark
/// one is the original `uzumaki-tray.png`).
fn icon(app_icon: IconVariant, px: u32) -> TrayIcon {
    TrayIcon::rgba(px, px, icon_pixels(app_icon, px))
}

fn tray_png(app_icon: IconVariant) -> &'static [u8] {
    match app_icon {
        IconVariant::Dark => include_bytes!("../../assets/icons/variants/dark/tray.png"),
        IconVariant::Light => include_bytes!("../../assets/icons/variants/light/tray.png"),
        IconVariant::Coral => include_bytes!("../../assets/icons/variants/coral/tray.png"),
    }
}

/// Straight RGBA rows of `icon`, `px` pixels a side (decoded from the PNG built into the binary).
fn icon_pixels(app_icon: IconVariant, px: u32) -> Vec<u8> {
    match image::load_from_memory_with_format(tray_png(app_icon), image::ImageFormat::Png) {
        Ok(img) => img.resize_exact(px, px, image::imageops::FilterType::Lanczos3).to_rgba8().into_raw(),
        Err(_) => vec![0; (px * px * 4) as usize],
    }
}

#[cfg(test)]
mod tests {
    use rune::tray::TrayItem;

    fn labels(items: &[TrayItem]) -> Vec<(String, bool)> {
        items.iter().filter_map(|i| match i {
            TrayItem::Action { label, enabled, .. } => Some((label.clone(), *enabled)),
            _ => None,
        }).collect()
    }

    #[test]
    fn the_menu_has_the_three_modes_after_next_and_greys_them_with_nothing_loaded() {
        use super::{menu, Modes, Repeat};
        let m = Modes { shuffle: true, repeat: Repeat::One, autoplay: true, autoplay_offered: true };
        let l = labels(&menu(false, Some((&m, true))));
        assert_eq!(&l[..6], &[
            ("Play".to_string(), true), ("Previous".to_string(), true), ("Next".to_string(), true),
            ("Shuffle: On".to_string(), true), ("Repeat: One".to_string(), true), ("AutoPlay: On".to_string(), true),
        ]);
        let idle = labels(&menu(false, Some((&m, false))));
        assert!(idle[3..6].iter().all(|(_, on)| !on), "greyed, like the window's missing buttons: {idle:?}");
        assert_eq!(idle[3].0, "Shuffle: On", "still says its state");
        let local = Modes { autoplay_offered: false, ..m };
        let l = labels(&menu(false, Some((&local, true))));
        assert!(l[3].1 && l[4].1 && !l[5].1, "no AutoPlay where the window has no ∞ button");
        let first = labels(&menu(true, None));
        assert_eq!(first[0].0, "Pause");
        assert!(first[3..6].iter().all(|(_, on)| !on), "before the first sync: greyed");
    }

    #[test]
    fn the_icon_is_a_coral_swirl_on_a_dark_rounded_tile() {
        let rgba = super::icon_pixels(super::IconVariant::Dark, super::ICON_PX);
        assert_eq!(rgba.len(), (super::ICON_PX * super::ICON_PX * 4) as usize);
        let px = |x: u32, y: u32| {
            let i = ((y * super::ICON_PX + x) * 4) as usize;
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
        use super::IconVariant;
        let tile = |i: IconVariant| {
            let rgba = super::icon_pixels(i, super::ICON_PX);
            let at = ((32 * super::ICON_PX + 10) * 4) as usize; // left edge of the tile
            [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
        };
        assert!(tile(IconVariant::Light)[0] > 0xf0 && tile(IconVariant::Light)[2] > 0xf0, "white tile");
        assert!(tile(IconVariant::Coral)[0] > 0xf0 && tile(IconVariant::Coral)[1] > 0xc0 && tile(IconVariant::Coral)[1] < 0xe0, "pale coral tile");
        assert_eq!(super::tray_png(IconVariant::Dark), include_bytes!("../../assets/icons/uzumaki-tray.png"));
    }

    /// Windows asks for the notification area's own size: the drawing comes out at any of them.
    #[test]
    fn the_icon_is_drawn_at_the_size_asked_for() {
        for px in [16u32, 20, 24, 32] {
            let rgba = super::icon_pixels(super::IconVariant::Dark, px);
            assert_eq!(rgba.len(), (px * px * 4) as usize);
            // Scaling leaves a trace of the tile's edge in the corner pixel, never more.
            assert!(rgba[3] < 48, "the rounded corner stays clear at {px} px, got alpha {}", rgba[3]);
            let mid = ((px / 2 * px + px / 4) * 4) as usize;
            assert!(rgba[mid + 3] > 200, "the tile is solid at {px} px, got alpha {}", rgba[mid + 3]);
        }
    }

}
