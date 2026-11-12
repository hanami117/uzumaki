//! The system tray icon (StatusNotifierItem through Rune): Play/Pause, Previous, Next, Show and
//! Quit, plus the current song as the tooltip. A left click shows the window.
//!
//! The tray runs on its own thread; its events go to the UI thread with `handle.update`, so
//! they work while the window is closed to the tray (`App::keep_running`). It shows only where
//! a tray host runs (KDE, waybar and most bars; GNOME needs the AppIndicator extension).

use rune::tray::{TrayEvent, TrayIcon, TrayItem, TraySpec};
use rune::Tray;

use crate::models::Song;
use crate::settings::IconVariant;
use crate::state::{Handle, State};

/// Tray icon side in pixels; hosts scale it.
const ICON_PX: u32 = 64;

pub struct TrayCtl {
    tray: Tray,
    playing: bool,
    tooltip: String,
}

impl TrayCtl {
    /// None when the platform has no tray (or D-Bus is unavailable).
    /// The icon is the chosen app icon's; Rune's tray can't swap its image once shown, so a
    /// change in Settings shows here at the next start.
    pub fn start(handle: Handle, app_icon: IconVariant) -> Option<Self> {
        let spec = TraySpec::new("uzumaki", "Uzumaki", icon(app_icon)).tooltip("Uzumaki").menu(menu(false));
        let on_event = move |ev: TrayEvent| match ev {
            TrayEvent::Activate { token } => match token {
                Some(t) => handle.activate(t),
                None => handle.open(),
            },
            TrayEvent::Menu(id) => match id.as_str() {
                "play-pause" => handle.update(|s: &mut State, _| s.toggle_play()),
                "next" => handle.update(|s: &mut State, _| s.next()),
                "previous" => handle.update(|s: &mut State, _| s.previous()),
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
            Ok(tray) => Some(Self { tray, playing: false, tooltip: String::new() }),
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
            self.tray.set_menu(menu(playing));
        }
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
    }
}

fn menu(playing: bool) -> Vec<TrayItem> {
    vec![
        TrayItem::action("play-pause", if playing { "Pause" } else { "Play" }),
        TrayItem::action("previous", "Previous"),
        TrayItem::action("next", "Next"),
        TrayItem::Separator,
        TrayItem::action("show", "Show Uzumaki"),
        TrayItem::action("check-updates", "Check for Updates\u{2026}"),
        TrayItem::Separator,
        TrayItem::action("quit", "Quit Uzumaki"),
    ]
}

/// The chosen app icon's small drawing (`assets/icons/variants/<name>/tray.png`, 64 px; the dark
/// one is the original `uzumaki-tray.png`).
fn icon(app_icon: IconVariant) -> TrayIcon {
    TrayIcon::rgba(ICON_PX, ICON_PX, icon_pixels(app_icon))
}

fn tray_png(app_icon: IconVariant) -> &'static [u8] {
    match app_icon {
        IconVariant::Dark => include_bytes!("../../assets/icons/variants/dark/tray.png"),
        IconVariant::Light => include_bytes!("../../assets/icons/variants/light/tray.png"),
        IconVariant::Coral => include_bytes!("../../assets/icons/variants/coral/tray.png"),
    }
}

/// Straight RGBA rows of `icon` (decoded from the PNG built into the binary).
fn icon_pixels(app_icon: IconVariant) -> Vec<u8> {
    match image::load_from_memory_with_format(tray_png(app_icon), image::ImageFormat::Png) {
        Ok(img) => img.resize_exact(ICON_PX, ICON_PX, image::imageops::FilterType::Lanczos3).to_rgba8().into_raw(),
        Err(_) => vec![0; (ICON_PX * ICON_PX * 4) as usize],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_icon_is_a_coral_swirl_on_a_dark_rounded_tile() {
        let rgba = super::icon_pixels(super::IconVariant::Dark);
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
            let rgba = super::icon_pixels(i);
            let at = ((32 * super::ICON_PX + 10) * 4) as usize; // left edge of the tile
            [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
        };
        assert!(tile(IconVariant::Light)[0] > 0xf0 && tile(IconVariant::Light)[2] > 0xf0, "white tile");
        assert!(tile(IconVariant::Coral)[0] > 0xf0 && tile(IconVariant::Coral)[1] > 0xc0 && tile(IconVariant::Coral)[1] < 0xe0, "pale coral tile");
        assert_eq!(super::tray_png(IconVariant::Dark), include_bytes!("../../assets/icons/uzumaki-tray.png"));
    }

}
