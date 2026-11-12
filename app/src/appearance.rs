//! Settings → Appearance: the background picture behind the content area, its effect, and the
//! full-screen Now Playing background. Stored in `Settings::appearance`.
//!
//! CONTRACT (fixed by the main session). Drawing lives in `ui/backdrop.rs`; rendering the
//! picture with its effect lives in `backdrop.rs` (service) and the `uzumaki-fx` crate.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use uzumaki_fx::Effect;

/// Opacity range of the background picture on the dark theme (monocode's range: faint to
/// strong, never opaque).
pub const OPACITY_MIN: f32 = 0.05;
pub const OPACITY_MAX: f32 = 0.65;
/// The light theme's top: a picture over near-white needs far more opacity to show as much as
/// it does over near-black (a pale picture differs little from the paper).
pub const LIGHT_OPACITY_MAX: f32 = 0.9;
/// The light theme's defaults (a file from before the light strengths existed gets these): the
/// picture clearly visible, text at 4.5:1 on a bright picture (see `uzumaki_fx::LIGHT_FLOOR`).
pub const LIGHT_OPACITY_DEFAULT: f32 = 0.8;
pub const LIGHT_LIST_OPACITY_DEFAULT: f32 = 0.65;

/// The strength range for a theme, `(min, max)`.
pub fn opacity_range(light: bool) -> (f32, f32) {
    (OPACITY_MIN, if light { LIGHT_OPACITY_MAX } else { OPACITY_MAX })
}

/// What shows behind the content area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BgSource {
    /// The plain dark background, as before.
    #[default]
    Off,
    /// Uzumaki's built-in swirl (`uzumaki_fx::builtin`).
    Builtin,
    /// A picture the user chose (`Appearance::picture`).
    Picture,
    /// The playing song's cover; nothing while nothing plays.
    Cover,
}

/// Which pages show the background (as monocode's "Empty only / All sessions").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShowOn {
    /// Every page: card pages at `Appearance::opacity`, song lists at `list_opacity`.
    #[default]
    All,
    /// Card pages only (Home, New, Radio, Made for You, Recently Added, album and playlist
    /// grids, Artists, an artist, Search); song lists get the plain background.
    Cards,
}

/// What shows behind full-screen Now Playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NpBg {
    /// A gradient in the cover's colour, as before.
    #[default]
    Tint,
    /// The cover, filling the screen, with `Appearance::np_effect`, under a dark scrim.
    Cover,
    /// The same background as the content area (source, picture and effect).
    App,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub source: BgSource,
    /// The chosen picture (absolute path, PNG or JPEG), kept when the source changes.
    pub picture: Option<PathBuf>,
    pub effect: Effect,
    /// The picture's opacity over the dark background on card pages (and in Now Playing's
    /// "Same as app", which is always dark), `OPACITY_MIN..=OPACITY_MAX`.
    pub opacity: f32,
    pub show_on: ShowOn,
    /// The opacity on song-list pages (album and playlist pages, Library → Songs, Local Files),
    /// where dense rows of text sit on it. Unused while `show_on` is `Cards`.
    pub list_opacity: f32,
    /// `opacity` and `list_opacity` for the light theme, `OPACITY_MIN..=LIGHT_OPACITY_MAX`.
    pub light_opacity: f32,
    pub light_list_opacity: f32,
    /// The picture fades out towards the bottom, so lists sit on the plain background.
    pub fade: bool,
    pub np: NpBg,
    /// The effect on the cover when `np` is `Cover`.
    pub np_effect: Effect,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            source: BgSource::Off,
            picture: None,
            effect: Effect::Plain,
            opacity: 0.35,
            show_on: ShowOn::All,
            list_opacity: 0.35,
            light_opacity: LIGHT_OPACITY_DEFAULT,
            light_list_opacity: LIGHT_LIST_OPACITY_DEFAULT,
            fade: true,
            np: NpBg::Tint,
            np_effect: Effect::Blur,
        }
    }
}

impl Appearance {
    /// `opacity`, clamped (an edited file may hold anything).
    pub fn opacity(&self) -> f32 {
        clamp_opacity(self.opacity)
    }

    /// `list_opacity`, clamped.
    pub fn list_opacity(&self) -> f32 {
        clamp_opacity(self.list_opacity)
    }

    /// The card-page opacity for a theme (`light`), clamped to its range.
    pub fn opacity_for(&self, light: bool) -> f32 {
        if light { clamp_light(self.light_opacity, LIGHT_OPACITY_DEFAULT) } else { self.opacity() }
    }

    /// The song-list opacity for a theme, clamped to its range.
    pub fn list_opacity_for(&self, light: bool) -> f32 {
        if light { clamp_light(self.light_list_opacity, LIGHT_LIST_OPACITY_DEFAULT) } else { self.list_opacity() }
    }

    /// The background's opacity on a page for a theme: `list` says whether it is a song-list
    /// page. None draws no background there (`show_on` is `Cards`).
    pub fn page_opacity(&self, list: bool, light: bool) -> Option<f32> {
        match (list, self.show_on) {
            (false, _) => Some(self.opacity_for(light)),
            (true, ShowOn::All) => Some(self.list_opacity_for(light)),
            (true, ShowOn::Cards) => None,
        }
    }
}

fn clamp_opacity(v: f32) -> f32 {
    if v.is_finite() { v.clamp(OPACITY_MIN, OPACITY_MAX) } else { 0.35 }
}

fn clamp_light(v: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(OPACITY_MIN, LIGHT_OPACITY_MAX) } else { default }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_change_nothing_and_round_trip() {
        let a = Appearance::default();
        assert_eq!(a.source, BgSource::Off);
        assert_eq!(a.np, NpBg::Tint);
        let back: Appearance = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        let partial: Appearance = serde_json::from_str(r#"{"source":"Builtin","effect":"Ascii"}"#).unwrap();
        assert_eq!((partial.source, partial.effect, partial.np_effect), (BgSource::Builtin, Effect::Ascii, Effect::Blur));
    }

    #[test]
    fn page_opacity_follows_show_on() {
        let mut a = Appearance { opacity: 0.4, list_opacity: 0.1, light_opacity: 0.8, light_list_opacity: 0.3, ..Appearance::default() };
        assert_eq!(a.page_opacity(false, false), Some(0.4));
        assert_eq!(a.page_opacity(true, false), Some(0.1));
        assert_eq!(a.page_opacity(false, true), Some(0.8));
        assert_eq!(a.page_opacity(true, true), Some(0.3));
        a.show_on = ShowOn::Cards;
        assert_eq!(a.page_opacity(false, false), Some(0.4));
        assert_eq!(a.page_opacity(true, false), None);
        assert_eq!(a.page_opacity(true, true), None);
        let old: Appearance = serde_json::from_str(r#"{"source":"Builtin","opacity":0.5}"#).unwrap();
        assert_eq!((old.show_on, old.list_opacity()), (ShowOn::All, 0.35), "files from before keep loading");
    }

    /// A file from before the light strengths keeps its dark ones and gets the light defaults
    /// (not a copy of the dark values, which are too faint on white).
    #[test]
    fn light_strengths_are_separate_and_default_for_old_files() {
        let old: Appearance = serde_json::from_str(r#"{"source":"Picture","opacity":0.28,"list_opacity":0.35}"#).unwrap();
        assert_eq!((old.opacity_for(false), old.list_opacity_for(false)), (0.28, 0.35));
        assert_eq!((old.opacity_for(true), old.list_opacity_for(true)), (LIGHT_OPACITY_DEFAULT, LIGHT_LIST_OPACITY_DEFAULT));
        let a = Appearance { light_opacity: 0.85, opacity: 0.85, ..Appearance::default() };
        assert_eq!((a.opacity_for(true), a.opacity_for(false)), (0.85, OPACITY_MAX), "light goes higher than dark");
        let a = Appearance { light_opacity: 5.0, light_list_opacity: f32::NAN, ..Appearance::default() };
        assert_eq!((a.opacity_for(true), a.list_opacity_for(true)), (LIGHT_OPACITY_MAX, LIGHT_LIST_OPACITY_DEFAULT));
        assert_eq!(opacity_range(true), (OPACITY_MIN, LIGHT_OPACITY_MAX));
        assert_eq!(opacity_range(false), (OPACITY_MIN, OPACITY_MAX));
    }

    #[test]
    fn opacity_is_clamped() {
        let a = Appearance { opacity: 3.0, ..Appearance::default() };
        assert_eq!(a.opacity(), OPACITY_MAX);
        let a = Appearance { opacity: f32::NAN, ..Appearance::default() };
        assert_eq!(a.opacity(), 0.35);
    }
}
