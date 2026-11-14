//! Palette and type styles: dark and minimal, Inter, a coral accent (#ff7a8a).

use std::sync::LazyLock;
use std::cell::Cell;

use rune::{Color, TextStyle};

fn hx(s: &str) -> Color {
    rune::color::hex(s).expect("bad hex colour")
}

pub struct Pal {
    pub bg: Color,
    pub sidebar: Color,
    pub player: Color,
    pub field: Color,
    pub field_border: Color,
    pub card_hover: Color,
    pub row_hover: Color,
    pub chip: Color,
    pub nav_active: Color,
    pub chip_hover: Color,
    pub track: Color,
    pub divider: Color,
    pub text: Color,
    pub text2: Color,
    pub nav_text: Color,
    pub muted: Color,
    pub dim: Color,
    pub accent: Color,
    pub ink: Color,
    pub art_ink: Color,
    pub danger_bg: Color,
    pub danger_text: Color,
    /// Lyrics: lines already sung and lines still to come (the current one is `text`).
    pub lyric_next: Color,
    pub np_base: Color,
    /// Pink-ish tint of favourite / explicit marks on a surface (`ui::collection`).
    pub accent_soft: Color,
    /// Alpha of the accent fill on selected rows.
    pub sel_alpha: f32,
}

static DARK: LazyLock<Pal> = LazyLock::new(|| Pal {
    bg: hx("#0e0e11"),
    sidebar: hx("#121216"),
    player: hx("#15151a"),
    field: hx("#18181d"),
    field_border: hx("#2a2a32"),
    card_hover: hx("#1a1a20"),
    row_hover: hx("#1c1c22"),
    chip: hx("#1c1c22"),
    nav_active: hx("#24242c"),
    chip_hover: hx("#2a2a32"),
    track: hx("#2e2e36"),
    divider: hx("#1f1f25"),
    text: hx("#f2f2f5"),
    text2: hx("#d6d6dc"),
    nav_text: hx("#b4b4bd"),
    muted: hx("#a6a6b0"),
    dim: hx("#8a8a94"),
    accent: hx("#ff7a8a"),
    ink: hx("#111111"),
    art_ink: hx("#00000090"),
    danger_bg: hx("#3a1a1f"),
    danger_text: hx("#ffb4b4"),
    lyric_next: hx("#7a7a84"),
    np_base: hx("#0b0b0e"),
    accent_soft: hx("#ffb4bd"),
    sel_alpha: 0.22,
});

fn light_pal() -> Pal {
    Pal {
    bg: hx("#fcfcfd"),
    sidebar: hx("#f2f2f5"),
    player: hx("#f6f6f8"),
    field: hx("#ffffff"),
    field_border: hx("#d9d9df"),
    card_hover: hx("#f0f0f3"),
    row_hover: hx("#efeff2"),
    chip: hx("#ececf0"),
    nav_active: hx("#e4e4e9"),
    chip_hover: hx("#dcdce2"),
    track: hx("#d4d4db"),
    divider: hx("#e6e6ea"),
    text: hx("#16161a"),
    text2: hx("#3a3a42"),
    nav_text: hx("#44444c"),
    muted: hx("#5f5f69"),
    dim: hx("#6e6e78"),
    accent: hx("#d6304a"),
    ink: hx("#ffffff"),
    art_ink: hx("#00000070"),
    danger_bg: hx("#fde6e8"),
    danger_text: hx("#b3261e"),
    lyric_next: hx("#9a9aa3"),
    np_base: hx("#0b0b0e"),
    accent_soft: hx("#d6304a"),
    sel_alpha: 0.12,
    }
}

static LIGHT: LazyLock<Pal> = LazyLock::new(light_pal);

/// The light palette while a background picture shows behind the content: the grey and accent
/// ink one step darker, so small text keeps 4.5:1 on a background down to mid-light grey (the
/// picture is printed no darker than `uzumaki_fx::LIGHT_FLOOR`). Surfaces are unchanged.
static LIGHT_INK: LazyLock<Pal> = LazyLock::new(|| Pal {
    muted: hx("#45454d"),
    dim: hx("#4a4a52"),
    accent: hx("#9c1a2f"),
    accent_soft: hx("#9c1a2f"),
    ..light_pal()
});

// Per thread: the app builds on its UI thread only, and each headless test window builds on its
// test's thread, so parallel tests in light and dark don't repaint each other's views.
thread_local! {
    /// The palette in force. `ui::view` sets it at the top of every build (from the Theme setting
    /// and `cx.system_dark()`, which `--theme light|dark` drives headless); Now Playing builds
    /// under `dark_scope`.
    static IS_LIGHT: Cell<bool> = const { Cell::new(false) };
    /// Light only: a background picture shows, so `LIGHT_INK` is in force (see `apply`).
    static OVER_PICTURE: Cell<bool> = const { Cell::new(false) };
}

pub struct PalRef;
impl std::ops::Deref for PalRef {
    type Target = Pal;
    fn deref(&self) -> &Pal {
        match (IS_LIGHT.get(), OVER_PICTURE.get()) {
            (true, true) => &LIGHT_INK,
            (true, false) => &LIGHT,
            (false, _) => &DARK,
        }
    }
}
pub static PAL: PalRef = PalRef;

/// The colours the AirPlay glyphs are rasterised in (`symbols.rs`): text, secondary text and accent of every palette.
#[cfg(target_os = "macos")]
pub fn symbol_colors() -> Vec<Color> {
    [&*DARK, &*LIGHT, &*LIGHT_INK].into_iter().flat_map(|p| [p.text, p.text2, p.accent]).chain([Color::WHITE]).collect()
}

/// Puts the palette for the next build in force (`ui::view`, first thing). When the theme
/// flipped, the native window's own background follows (macOS; see `main::sync_window_bg`).
pub fn apply(light: bool) {
    if IS_LIGHT.replace(light) != light {
        crate::sync_window_bg();
    }
}
/// Light only: the darker ink for text over a background picture (`ui::view`, after `apply`).
/// No effect on the dark palette.
pub fn set_over_picture(on: bool) {
    OVER_PICTURE.set(on);
}
/// True while the light palette uses the darker ink (memo keys that depend on the palette).
pub fn over_picture() -> bool {
    is_light() && OVER_PICTURE.get()
}
pub fn is_light() -> bool {
    IS_LIGHT.get()
}
/// Builds `f` with the dark palette (Now Playing is a dark stage in both themes).
pub fn dark_scope<T>(f: impl FnOnce() -> T) -> T {
    let was = IS_LIGHT.replace(false);
    let out = f();
    IS_LIGHT.set(was);
    out
}
/// Hover/press overlay: white over dark surfaces, black over light ones.
pub fn ov(a: f32) -> Color {
    if is_light() { Color::BLACK.alpha(a * 0.8) } else { Color::WHITE.alpha(a) }
}
/// The opposite overlay, for fills that are `PAL.text` (the round play button): black over
/// the near-white dark-theme fill, white over the near-black light-theme one.
pub fn ov_inv(a: f32) -> Color {
    if is_light() { Color::WHITE.alpha(a) } else { Color::BLACK.alpha(a) }
}
/// Accent fill of a drag-and-drop target: `dark_a` in dark, a lighter 0.14 in light.
pub fn drop_fill(dark_a: f32) -> Color {
    PAL.accent.alpha(if is_light() { 0.14 } else { dark_a })
}
/// The selected segment inside a `chip` track: a white pill on light (nav_active is almost the
/// track's own grey there), the usual lift on dark.
pub fn seg_on() -> Color {
    if is_light() { PAL.field } else { PAL.nav_active }
}
/// Fill and border of menus and popovers: today's grey panel in dark; in light a white one with
/// a hairline, since a grey panel looks muddy on white and a soft shadow alone can't separate white from white.
pub fn pop_fill() -> Color {
    if is_light() { PAL.field } else { PAL.nav_active }
}
pub fn pop_border() -> Color {
    if is_light() { PAL.field_border } else { PAL.chip_hover }
}
/// Drop shadows: lighter on a light theme.
pub fn shadow(a: f32) -> Color {
    Color::BLACK.alpha(if is_light() { a * 0.35 } else { a })
}
/// Modal scrims.
pub fn scrim(a: f32) -> Color {
    Color::BLACK.alpha(if is_light() { a * 0.45 } else { a })
}

pub const SIDEBAR_W: f32 = 232.0;
/// Collapsed sidebar (icon rail) width: the macOS traffic lights (60 pt across) sit centred
/// over it, 8 pt in from either edge. The same on every platform.
pub const RAIL_W: f32 = 76.0;
/// macOS: the band kept clear at the sidebar's top for the traffic lights, draggable like a
/// title bar, as in Music for Mac.
pub const TITLEBAR_BAND: f32 = 28.0;
/// macOS: the close button's top-left in points (`rune::Chrome::traffic_lights`). The three
/// buttons (14 pt on a 23 pt pitch) are pinned centred over the rail and the band, rather
/// than left where AppKit puts them (about (9, 9) since macOS 26), which hung past the rail.
pub const TRAFFIC_LIGHTS: (f32, f32) = (
    (RAIL_W - (rune::Chrome::TRAFFIC_LIGHT + 2.0 * rune::Chrome::TRAFFIC_PITCH)) / 2.0,
    (TITLEBAR_BAND - rune::Chrome::TRAFFIC_LIGHT) / 2.0,
);
/// Below this window width the sidebar is a rail whatever the saved setting says.
pub const AUTO_COLLAPSE_W: f32 = 900.0;

/// The narrow layout, below `AUTO_COLLAPSE_W`: the sidebar is a rail, the right panel can't open
/// (its buttons open full-screen Now Playing instead, `ui/narrow.rs`) and Now Playing stacks its
/// lyrics under the cover. The one test for all three.
pub fn is_narrow(win_w: f32) -> bool {
    win_w < AUTO_COLLAPSE_W
}
/// Below this the player bar drops its secondary controls.
pub const COMPACT_W: f32 = 760.0;
pub const PLAYER_H: f32 = 80.0;

// Inter ships 400/500/600 only, so the spec's 700 becomes 600.
pub const BRAND: TextStyle = TextStyle::new(17.0, 600.0).tracking(-0.17);
pub const H1: TextStyle = TextStyle::new(30.0, 600.0).tracking(-0.6);
pub const H2: TextStyle = TextStyle::new(18.0, 600.0);
pub const HERO: TextStyle = TextStyle::new(36.0, 600.0).tracking(-0.72);
pub const NAV: TextStyle = TextStyle::new(14.0, 400.0);
pub const BODY14: TextStyle = TextStyle::new(14.0, 400.0);
pub const TRACK_TITLE: TextStyle = TextStyle::new(14.0, 500.0);
pub const CARD_TITLE: TextStyle = TextStyle::new(14.0, 600.0);
pub const SMALL13: TextStyle = TextStyle::new(13.0, 400.0);
pub const SMALL12: TextStyle = TextStyle::new(12.0, 400.0).features("'tnum' 1");
pub const LABEL12: TextStyle = TextStyle::new(12.0, 600.0).tracking(0.96);
pub const SECTION11: TextStyle = TextStyle::new(11.0, 600.0).tracking(0.88);
pub const TABLE_HEAD: TextStyle = TextStyle::new(12.0, 400.0).tracking(0.72);
pub const BUTTON: TextStyle = TextStyle::new(15.0, 600.0);
pub const TIME14: TextStyle = TextStyle::new(14.0, 400.0).features("'tnum' 1");

#[cfg(test)]
mod tests {
    use super::*;

    fn lin(c: f32) -> f32 {
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    }
    /// WCAG relative luminance of an sRGB colour (channels 0..=255).
    fn lum(rgb: [f32; 3]) -> f32 {
        0.2126 * lin(rgb[0] / 255.0) + 0.7152 * lin(rgb[1] / 255.0) + 0.0722 * lin(rgb[2] / 255.0)
    }
    fn rgb(hex: &str) -> [f32; 3] {
        let v = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f32;
        [v(1), v(3), v(5)]
    }

    /// Over a picture on light the grey and accent ink is one step darker, so it keeps 4.5:1 on
    /// a background down to mid-light grey (sRGB 200), where the plain light ink needs near-white
    /// (245). There is no backing under text, so a darker part of the picture than that can
    /// still bring small text below 4.5:1 (the strength slider is the user's control).
    #[test]
    fn light_ink_over_a_picture_is_darker_and_holds_on_light_grey() {
        let ratio = |hex: &str, bg: f32| (lum([bg, bg, bg]) + 0.05) / (lum(rgb(hex)) + 0.05);
        for hex in ["#16161a", "#3a3a42", "#45454d", "#4a4a52", "#9c1a2f"] {
            assert!(ratio(hex, 200.0) >= 4.5, "{hex}: {:.2}", ratio(hex, 200.0));
        }
        for hex in ["#5f5f69", "#6e6e78", "#d6304a"] {
            assert!(ratio(hex, 200.0) < 4.5, "{hex}: the plain light ink would not hold there");
        }
        assert!(ratio("#6e6e78", 245.0) >= 4.5, "the plain light ink needs near-white");
        // The palette really uses those, and only those change.
        assert_eq!((LIGHT_INK.muted, LIGHT_INK.dim, LIGHT_INK.accent), (hx("#45454d"), hx("#4a4a52"), hx("#9c1a2f")));
        assert_eq!((LIGHT_INK.text, LIGHT_INK.text2, LIGHT_INK.bg), (LIGHT.text, LIGHT.text2, LIGHT.bg));
    }
}
