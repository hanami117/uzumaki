//! Shuffle, Repeat and AutoPlay in the tray menus (`tray_mac.rs`, `tray_linux.rs`).
//!
//! No state of their own: the menu items call what the window's buttons call (`toggle_shuffle`,
//! `cycle_repeat` / `set_repeat`, `toggle_autoplay`, the same as `uzu shuffle/repeat/autoplay`),
//! and show what those buttons show. `Modes` is that picture, handed to the tray each time
//! one of them changes (`sync_tray_modes`) and when the macOS menu opens.
//!
//! The window's player bar (and so its Shuffle and Repeat buttons) exists only while a song is
//! loaded, and the ∞ button only where AutoPlay applies (Apple Music, not a station): the
//! tray's items are greyed in the same cases (`loaded` is the tray's own: a song is shown).

use super::{Repeat, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modes {
    pub shuffle: bool,
    pub repeat: Repeat,
    pub autoplay: bool,
    /// AutoPlay can play here (the window shows its button).
    pub autoplay_offered: bool,
}

impl Modes {
    /// AutoPlay's item: the ∞ button is on the screen only where AutoPlay applies, and
    /// the player is needed too.
    pub fn autoplay_enabled(&self, loaded: bool) -> bool {
        loaded && self.autoplay_offered
    }

    // Plain-item wording, for trays without checkmarks (Linux, Windows; the Mac menu has checkmarks).

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn shuffle_text(&self) -> String {
        format!("Shuffle: {}", on_off(self.shuffle))
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn repeat_text(&self) -> String {
        format!("Repeat: {}", repeat_word(self.repeat))
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn autoplay_text(&self) -> String {
        format!("AutoPlay: {}", on_off(self.autoplay))
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

/// The word for a repeat mode, in the macOS submenu and the plain items.
pub fn repeat_word(r: Repeat) -> &'static str {
    match r {
        Repeat::Off => "Off",
        Repeat::All => "All",
        Repeat::One => "One",
    }
}

impl State {
    pub fn tray_modes(&self) -> Modes {
        Modes {
            shuffle: self.playback.shuffle,
            repeat: self.playback.repeat,
            autoplay: self.settings.autoplay,
            autoplay_offered: self.autoplay_offered(),
        }
    }

    /// Tells the tray what the three show now (a no-op without a tray).
    pub fn sync_tray_modes(&mut self) {
        let modes = self.tray_modes();
        if let Some(t) = &mut self.tray {
            t.set_modes(&modes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modes_are_what_the_windows_buttons_show() {
        let mut s = State::offline();
        let m = s.tray_modes();
        assert!(!m.shuffle && m.repeat == Repeat::Off);
        s.toggle_shuffle();
        s.cycle_repeat();
        assert!(s.tray_modes().shuffle);
        assert_eq!(s.tray_modes().repeat, Repeat::All);
        s.cycle_repeat();
        assert_eq!(s.tray_modes().repeat, Repeat::One, "the same cycle as the player bar's button");
        s.set_repeat(Repeat::Off);
        assert_eq!(s.tray_modes().repeat, Repeat::Off, "the submenu sets a mode directly");
        let on = s.settings.autoplay;
        s.toggle_autoplay();
        assert_eq!(s.tray_modes().autoplay, !on, "the ∞ button's setting");
    }

    #[test]
    fn autoplay_is_offered_only_where_the_window_offers_it() {
        let mut s = State::offline();
        assert_eq!(s.tray_modes().autoplay_offered, s.autoplay_offered());
        s.settings.music_source = crate::settings::MusicSource::LocalOnly;
        let m = s.tray_modes();
        assert!(!m.autoplay_offered, "local only has no AutoPlay button");
        assert!(!m.autoplay_enabled(true));
    }

    #[test]
    fn the_items_are_greyed_while_nothing_is_loaded() {
        let m = Modes { shuffle: false, repeat: Repeat::Off, autoplay: true, autoplay_offered: true };
        assert!(!m.autoplay_enabled(false) && m.autoplay_enabled(true));
        assert!(!Modes { autoplay_offered: false, ..m }.autoplay_enabled(true));
    }

    #[test]
    fn plain_items_say_their_state() {
        let m = Modes { shuffle: true, repeat: Repeat::All, autoplay: false, autoplay_offered: true };
        assert_eq!(m.shuffle_text(), "Shuffle: On");
        assert_eq!(m.repeat_text(), "Repeat: All");
        assert_eq!(m.autoplay_text(), "AutoPlay: Off");
        assert_eq!(Modes { repeat: Repeat::One, ..m }.repeat_text(), "Repeat: One");
        assert_eq!(Modes { shuffle: false, repeat: Repeat::Off, autoplay: true, ..m }.repeat_text(), "Repeat: Off");
    }

    #[test]
    fn a_change_reaches_the_tray_once_synced() {
        // No tray in tests: syncing is a harmless no-op.
        let mut s = State::offline();
        s.toggle_shuffle();
        s.sync_tray_modes();
        assert!(s.tray.is_none());
    }
}
