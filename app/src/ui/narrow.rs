//! The narrow window (`theme::is_narrow`): the sidebar is a rail whatever its setting says, and
//! there is no room for the right panel. What can't open there says so with a short shake, like
//! macOS's wrong-password field, instead of doing nothing (or changing a setting that only shows
//! once the window is wide again):
//! - Ctrl/Cmd+B and the rail's sidebar button shake the rail;
//! - Ctrl/Cmd+Shift+B shakes the player bar's Lyrics and Up Next buttons (the page when no bar shows);
//! - the Lyrics and Up Next buttons (and their shortcuts) open full-screen Now Playing on that
//!   column instead, which fits a narrow window.
//!
//! The shake is a `Live::pulse` on a bound offset, sprung by the renderer: no view runs, no polls,
//! and nothing draws once it has settled. With reduced motion the target dims for a moment instead.
//!
//! Each shake also shows a small bubble beside what shook, saying why ("Widen the window to show
//! the sidebar"), for about 2 s. It is a pulse too (opacity, and a short slide without reduced
//! motion), so it costs frames only while it fades; a press while it shows restarts it. It floats
//! over the page (no layout shift) and takes no pointer input. The bubbles are in the tree, clear,
//! whenever the window is narrow: a pulse only plays on an element bound before it starts.

use std::time::Duration;

use rune::{El, Live, MotionIntent, Spring, el, text};

use crate::settings::RightTab;
use crate::state::State;

/// Whether the window is narrow, from the last layout of the app root (`UiState::win_w`; 0 until
/// the first layout, which counts as wide, as the view's own first-frame guess does).
pub fn narrow(s: &State) -> bool {
    s.ui.win_w > 0.0 && crate::theme::is_narrow(s.ui.win_w)
}

// Between the narrow line and `right_panel::OVERLAY_BELOW` the panel opens over the page.
const _: () = assert!(super::right_panel::OVERLAY_BELOW > crate::theme::AUTO_COLLAPSE_W);

/// What can shake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The sidebar rail's contents.
    Rail,
    /// The player bar's Lyrics and Up Next buttons.
    PanelButtons,
    /// The page, for the right panel when no player bar shows.
    Page,
}

/// How far the first swing goes, in points.
const AMPLITUDE: f32 = 6.0;
/// Out to the first swing: quick and critically damped.
const OUT: Spring = Spring::new(0.06, 0.0);
/// Leave for home this long after the pulse starts, about when the first swing peaks.
const HOLD: Duration = Duration::from_millis(30);
/// Home with a few damped swings (about 0.3 s in all).
const BACK: Spring = Spring::new(0.12, 0.78);
/// Reduced motion: a short dip in opacity instead.
const DIM: f32 = 0.45;
const DIM_IN: Spring = Spring::new(0.08, 0.0);
const DIM_OUT: Spring = Spring::new(0.25, 0.0);
const DIM_HOLD: Duration = Duration::from_millis(80);

/// The hint: in quickly, held, then out a little slower.
const HINT_HOLD: Duration = Duration::from_millis(2000);
const HINT_IN: Spring = Spring::new(0.14, 0.0);
const HINT_OUT: Spring = Spring::new(0.28, 0.0);
/// How far it slides in from (towards what shook), in points.
const HINT_SLIDE: f32 = 4.0;
/// Room between the hint and what it is about.
const HINT_GAP: f32 = 8.0;
const HINT_H: f32 = 28.0;

/// The hint's bound values: opacity, and its offset for the slide in (sideways from the rail,
/// up from the player bar or down from the top of the page).
struct Hint {
    opacity: Live<f32>,
    x: Live<[f32; 2]>,
    y: Live<[f32; 2]>,
}

impl Default for Hint {
    fn default() -> Self {
        // The y slide's rest is set per target by the first `fire` (down for the bar's, up for the page's).
        Self { opacity: Live::new(0.0), x: Live::new([-HINT_SLIDE, 0.0]), y: Live::new([0.0, HINT_SLIDE]) }
    }
}

/// Where a hint slides in from, vertically: from below above the bar, from above at the page's top.
fn slide_y(t: Target) -> f32 {
    if t == Target::Page { -HINT_SLIDE } else { HINT_SLIDE }
}

/// One target's bound values.
struct Wobble {
    x: Live<[f32; 2]>,
    dim: Live<f32>,
    hint: Hint,
}

impl Default for Wobble {
    fn default() -> Self {
        Self { x: Live::new([0.0, 0.0]), dim: Live::new(1.0), hint: Hint::default() }
    }
}

/// The shakes, in `UiState`.
#[derive(Default)]
pub struct Shakes {
    rail: Wobble,
    buttons: Wobble,
    page: Wobble,
    /// The target whose hint showed last.
    pub hint_for: Option<Target>,
    /// The last target shaken and how many shakes there were (tests read them).
    pub last: Option<Target>,
    pub count: u32,
}

impl Shakes {
    fn wobble(&self, t: Target) -> &Wobble {
        match t {
            Target::Rail => &self.rail,
            Target::PanelButtons => &self.buttons,
            Target::Page => &self.page,
        }
    }

    /// Starts `t` shaking (or dimming). Both are pulsed; the view binds the one it wants.
    pub fn fire(&mut self, t: Target) {
        let w = self.wobble(t);
        w.x.pulse([AMPLITUDE, 0.0], [0.0, 0.0], HOLD);
        w.dim.pulse(DIM, 1.0, DIM_HOLD);
        // Restarts the hint's hold when it is already showing (no second bubble); another
        // target's hint fades out at once.
        let h = &w.hint;
        h.opacity.pulse(1.0, 0.0, HINT_HOLD);
        h.x.pulse([0.0, 0.0], [-HINT_SLIDE, 0.0], HINT_HOLD);
        h.y.pulse([0.0, 0.0], [0.0, slide_y(t)], HINT_HOLD);
        for o in [Target::Rail, Target::PanelButtons, Target::Page].into_iter().filter(|&o| o != t) {
            let h = &self.wobble(o).hint;
            h.opacity.set(0.0);
            h.x.set([-HINT_SLIDE, 0.0]);
            h.y.set([0.0, slide_y(o)]);
        }
        self.hint_for = Some(t);
        self.last = Some(t);
        self.count += 1;
    }
}

/// Binds `t`'s shake to `e`. It costs nothing while still: the renderer only draws while the
/// pulse's springs move.
pub fn bind(s: &State, t: Target, e: El, reduced: bool) -> El {
    let w = s.ui.shakes.wobble(t);
    if reduced {
        e.live_opacity(w.dim.smooth(DIM_IN).rest(DIM_OUT))
    } else {
        e.live_offset(w.x.smooth(OUT).rest(BACK)).motion_intent(MotionIntent::JIGGLE)
    }
}

/// Why `t` shook.
pub fn hint_text(t: Target) -> &'static str {
    match t {
        Target::Rail => "Widen the window to show the sidebar",
        Target::PanelButtons | Target::Page => "Widen the window to show the side panel",
    }
}

/// The hint bubbles, for the app root (over everything but menus and dialogs): one beside each
/// target, clear until it shakes. None while the window is wide.
/// `win_w`: the window's width; `chrome_top`: the sidebar's traffic-light band (0 off macOS).
pub fn hints(s: &State, win_w: f32, chrome_top: f32, reduced: bool) -> Vec<El> {
    if !crate::theme::is_narrow(win_w) {
        return Vec::new();
    }
    let mut v = vec![hint(s, Target::Rail, win_w, chrome_top, reduced), hint(s, Target::Page, win_w, chrome_top, reduced)];
    if s.playback.song.is_some() {
        v.push(hint(s, Target::PanelButtons, win_w, chrome_top, reduced));
    }
    v
}

fn hint(s: &State, t: Target, win_w: f32, chrome_top: f32, reduced: bool) -> El {
    use crate::theme::{PAL, PLAYER_H, RAIL_W, SMALL13, pop_border, pop_fill, shadow};
    let h = &s.ui.shakes.wobble(t).hint;
    let key = match t {
        Target::Rail => "narrow-hint-rail",
        Target::PanelButtons => "narrow-hint-panel",
        Target::Page => "narrow-hint-page",
    };
    let bubble = el().key(key).h(HINT_H).px(10.0).center().radius(8.0)
        .fill(pop_fill()).border(1.0, pop_border()).shadow(4.0, 16.0, shadow(0.35))
        .child(text(hint_text(t)).font(SMALL13).color(PAL.text2));
    let bubble = match t {
        // Beside the rail's sidebar button (`sidebar::rail_view`: the band or 16 pt of padding,
        // then the 36 pt logo and the 32 pt button 10 pt below it).
        Target::Rail => {
            let band = if chrome_top > 0.0 { chrome_top + 6.0 } else { 16.0 };
            bubble.pin(Some(RAIL_W + HINT_GAP), Some(band + 36.0 + 10.0 + (32.0 - HINT_H) / 2.0), None, None)
        }
        // Above the bar's right end, where Lyrics and Up Next sit (as the volume popover does).
        Target::PanelButtons => {
            let (right, bottom) = if s.settings.player_style == crate::settings::PlayerStyle::Floating {
                super::player_bar::floating_popover_pos(win_w)
            } else {
                (14.0, PLAYER_H + HINT_GAP)
            };
            bubble.pin(None, None, Some(right), Some(bottom))
        }
        // The page's top right, where the panel would open.
        Target::Page => bubble.pin(None, Some(16.0 + chrome_top), Some(16.0), None),
    };
    let bubble = bubble.live_opacity(h.opacity.smooth(HINT_IN).rest(HINT_OUT));
    if reduced {
        bubble
    } else {
        let slide = if t == Target::Rail { &h.x } else { &h.y };
        bubble.live_offset(slide.smooth(HINT_IN).rest(HINT_OUT))
    }
}

// ---- actions that a narrow window refuses ----

/// Ctrl/Cmd+B and the sidebar button: collapse or expand the sidebar; a narrow window keeps the
/// rail and shakes it (the saved setting is left alone).
pub fn toggle_sidebar(s: &mut State) {
    if narrow(s) {
        s.ui.shakes.fire(Target::Rail);
    } else {
        s.toggle_sidebar();
    }
}

/// Ctrl/Cmd+Shift+B: show or hide the right panel; a narrow window has no room for it and shakes
/// the buttons that would open it.
pub fn toggle_right_panel(s: &mut State) {
    if narrow(s) {
        s.ui.shakes.fire(panel_target(s));
    } else {
        s.toggle_right_panel();
    }
}

/// The player bar's Lyrics / Up Next buttons and their shortcuts: the right panel on `tab`; in a
/// narrow window, full-screen Now Playing with that column (nothing to show without a song: shake).
pub fn show_tab(s: &mut State, tab: RightTab) {
    if !narrow(s) {
        s.show_right_tab(tab);
    } else if s.playback.song.is_none() {
        s.ui.shakes.fire(panel_target(s));
    } else {
        s.set_now_playing_column(tab);
        super::open_now_playing(s);
    }
}

fn panel_target(s: &State) -> Target {
    if s.playback.song.is_some() && !s.ui.now_playing { Target::PanelButtons } else { Target::Page }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    /// The album scene (a song is playing) at window width `win_w`.
    fn state(win_w: f32) -> State {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "album").unwrap().1)(&mut s);
        s.ui.win_w = win_w;
        s
    }

    #[test]
    fn the_narrow_line_is_the_sidebar_rail_line() {
        let s = state(crate::theme::AUTO_COLLAPSE_W - 1.0);
        assert!(narrow(&s));
        assert!(super::super::sidebar::is_rail(&s, s.ui.win_w));
        let s = state(crate::theme::AUTO_COLLAPSE_W);
        assert!(!narrow(&s));
        // Before the first layout the window counts as wide.
        assert!(!narrow(&state(0.0)));
    }

    #[test]
    fn ctrl_b_shakes_the_rail_when_narrow_and_toggles_when_wide() {
        let mut s = state(640.0);
        s.settings.sidebar_collapsed = false;
        toggle_sidebar(&mut s);
        assert!(!s.settings.sidebar_collapsed, "a narrow window must not change the setting");
        assert_eq!((s.ui.shakes.last, s.ui.shakes.count), (Some(Target::Rail), 1));

        let mut s = state(1360.0);
        toggle_sidebar(&mut s);
        assert!(s.settings.sidebar_collapsed);
        assert_eq!(s.ui.shakes.count, 0);
    }

    #[test]
    fn ctrl_shift_b_shakes_when_narrow_and_opens_when_wide() {
        let mut s = state(640.0);
        s.settings.right_panel = false;
        assert!(s.playback.song.is_some());
        toggle_right_panel(&mut s);
        assert!(!s.settings.right_panel);
        assert!(!super::super::right_panel::visible(&s));
        assert_eq!(s.ui.shakes.last, Some(Target::PanelButtons));

        // No player bar: the page shakes.
        s.playback.song = None;
        toggle_right_panel(&mut s);
        assert_eq!((s.ui.shakes.last, s.ui.shakes.count), (Some(Target::Page), 2));

        let mut s = state(1360.0);
        s.settings.right_panel = false;
        toggle_right_panel(&mut s);
        assert!(s.settings.right_panel && super::super::right_panel::visible(&s));
        assert_eq!(s.ui.shakes.count, 0);
    }

    #[test]
    fn lyrics_and_up_next_open_now_playing_when_narrow() {
        let mut s = state(640.0);
        s.settings.right_panel = false;
        show_tab(&mut s, RightTab::Lyrics);
        assert!(s.ui.now_playing && !s.settings.right_panel);
        assert!(s.settings.now_playing_lyrics && !s.settings.now_playing_queue);
        super::super::close_now_playing(&mut s);
        show_tab(&mut s, RightTab::Queue);
        assert!(s.ui.now_playing && s.settings.now_playing_queue && !s.settings.right_panel);
        assert_eq!(s.ui.shakes.count, 0);

        // Nothing playing: nothing to open, so it shakes.
        let mut s = state(640.0);
        s.playback.song = None;
        show_tab(&mut s, RightTab::Lyrics);
        assert!(!s.ui.now_playing);
        assert_eq!(s.ui.shakes.count, 1);

        // Wide: the panel, as before.
        let mut s = state(1360.0);
        s.settings.right_panel = false;
        show_tab(&mut s, RightTab::Queue);
        assert!(!s.ui.now_playing && s.settings.right_panel && s.settings.right_tab == RightTab::Queue);
    }

    #[test]
    fn a_panel_left_open_hides_while_narrow_and_returns_when_wide() {
        let mut s = state(1360.0);
        s.settings.right_panel = true;
        assert!(super::super::right_panel::visible(&s));
        s.ui.win_w = 640.0;
        assert!(!super::super::right_panel::visible(&s));
        s.ui.win_w = 1360.0;
        assert!(super::super::right_panel::visible(&s));
    }

    // ---- through the real view and key handling ----

    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    fn rt(size: (f32, f32)) -> Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "album").unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, crate::ui::view, size, Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(crate::ui::on_key);
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    /// Mod+B, or Mod+Shift+B (Ctrl on Linux, which also sets `cmd`; Cmd alone on the Mac).
    fn press_b(rt: &mut Runtime<State>, shift: bool) {
        rt.key(KeyEvent { key: Key::Command('b'), mods: Mods { shift, cmd: true, ctrl: !cfg!(target_os = "macos"), ..Default::default() } });
        rt.frame(1.0 / 60.0);
    }

    fn read<T>(rt: &mut Runtime<State>, f: impl FnOnce(&State) -> T) -> T {
        let mut out = None;
        rt.update(|s, _| out = Some(f(s)));
        out.unwrap()
    }

    #[test]
    fn shortcuts_in_a_narrow_window_shake_instead_of_opening() {
        let mut rt = rt((640.0, 1280.0));
        assert!(read(&mut rt, narrow), "the layout reported the window's width");
        let rest = rt.presented("queue").expect("queue button").0[0];
        press_b(&mut rt, true);
        assert!(read(&mut rt, |s| !s.settings.right_panel && s.ui.shakes.last == Some(Target::PanelButtons)));
        // The buttons swing a few points and come back within about a third of a second.
        let mut xs = Vec::new();
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
            xs.push(rt.presented("queue").unwrap().0[0] - rest);
        }
        let max = xs.iter().cloned().fold(f32::MIN, f32::max);
        let min = xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(max > 3.0 && max <= AMPLITUDE + 0.5, "first swing {max}");
        assert!(min < -0.5, "it swings back past rest: {min}");
        assert!(xs[24..].iter().all(|x| x.abs() < 0.5), "still moving after 0.4 s: {xs:?}");
        assert!(rt.presented("right-panel").is_none());

        press_b(&mut rt, false);
        assert!(read(&mut rt, |s| !s.settings.sidebar_collapsed && s.ui.shakes.last == Some(Target::Rail)));
        // The rail's own button does the same.
        rt.update(|s, _| toggle_sidebar(s));
        assert!(read(&mut rt, |s| !s.settings.sidebar_collapsed && s.ui.shakes.count == 3));
    }

    #[test]
    fn shortcuts_in_a_wide_window_open_as_before() {
        let mut rt = rt((1360.0, 860.0));
        press_b(&mut rt, true);
        assert!(read(&mut rt, |s| s.settings.right_panel && super::super::right_panel::visible(s)));
        press_b(&mut rt, false);
        assert!(read(&mut rt, |s| s.settings.sidebar_collapsed && s.ui.shakes.count == 0));
    }

    /// Runs `secs` of frames at 60 Hz and returns the hint's presented opacity after each.
    fn hint_over(rt: &mut Runtime<State>, key: &str, secs: f32) -> Vec<f32> {
        (0..(secs * 60.0) as usize)
            .map(|_| {
                rt.frame(1.0 / 60.0);
                rt.presented(key).map_or(0.0, |p| p.1)
            })
            .collect()
    }

    #[test]
    fn a_shake_says_why_for_about_two_seconds_and_presses_restart_it() {
        let mut rt = rt((640.0, 1280.0));
        assert!(rt.presented("narrow-hint-panel").is_some_and(|p| p.1 < 0.01), "clear before a shake");
        press_b(&mut rt, true);
        assert_eq!(read(&mut rt, |s| s.ui.shakes.hint_for), Some(Target::PanelButtons));
        let o = hint_over(&mut rt, "narrow-hint-panel", 0.4);
        assert!(o[0] < 0.9, "it fades in: {o:?}");
        assert!(*o.last().unwrap() > 0.98, "{o:?}");
        // Above the bar, over its right end where Lyrics and Up Next sit, inside the window.
        let ([x, y, w, h], _) = rt.presented("narrow-hint-panel").unwrap();
        let lyrics = rt.presented("lyrics").unwrap().0;
        let queue = rt.presented("queue").unwrap().0;
        assert!(y + h <= 1280.0 - crate::theme::PLAYER_H && y + h > 1280.0 - crate::theme::PLAYER_H - 20.0, "y {y} h {h}");
        assert!(x >= 0.0 && x + w <= 640.0 && x < lyrics[0] && x + w > queue[0] + queue[2], "x {x} w {w}, lyrics {lyrics:?}");
        // Held, then pressed again before it fades: it stays one bubble and holds again.
        let o = hint_over(&mut rt, "narrow-hint-panel", 1.0);
        assert!(o.iter().all(|&a| a > 0.98), "held: {o:?}");
        // Held while the window sleeps: no frames needed until it leaves.
        assert!(!rt.frame(1.0 / 60.0), "frames while the hint holds");
        let o = hint_over(&mut rt, "narrow-hint-panel", 0.5);
        assert!(o.iter().all(|&a| a > 0.98), "held: {o:?}");
        press_b(&mut rt, true);
        let o = hint_over(&mut rt, "narrow-hint-panel", 1.8);
        assert!(o.iter().all(|&a| a > 0.98), "restarted: {o:?}");
        let o = hint_over(&mut rt, "narrow-hint-panel", 1.0);
        assert!(*o.last().unwrap() < 0.01, "gone after about 2 s: {o:?}");

        // The rail: beside its sidebar button, with the sidebar's words; the panel's hint goes.
        press_b(&mut rt, false);
        hint_over(&mut rt, "narrow-hint-rail", 0.4);
        let ([x, y, _, h], a) = rt.presented("narrow-hint-rail").unwrap();
        let ([bx, by, bw, bh], _) = rt.presented("sb-toggle").unwrap();
        assert!(a > 0.98 && x >= crate::theme::RAIL_W && x > bx + bw, "x {x}");
        assert!(((y + h / 2.0) - (by + bh / 2.0)).abs() < 1.0, "centred on the button: {y} {by}");
        assert!(rt.presented("narrow-hint-panel").unwrap().1 < 0.01, "the other hint went");
        assert_eq!(hint_text(Target::Rail), "Widen the window to show the sidebar");

        // Settled: nothing more to draw.
        hint_over(&mut rt, "narrow-hint-rail", 3.0);
        assert!(!rt.frame(1.0 / 60.0), "frames once the hint is gone");
    }

    #[test]
    fn the_hint_only_fades_with_reduced_motion_and_never_shows_wide() {
        let mut rt = rt((640.0, 1280.0));
        rt.set_system_reduced_motion(true);
        rt.frame(1.0 / 60.0);
        press_b(&mut rt, false);
        let rest = rt.presented("narrow-hint-rail").unwrap().0;
        for _ in 0..24 {
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.presented("narrow-hint-rail").unwrap().0, rest, "no slide");
        }
        assert!(rt.presented("narrow-hint-rail").unwrap().1 > 0.98, "it still fades in");

        let mut rt = super::tests::rt((1360.0, 860.0));
        press_b(&mut rt, true);
        press_b(&mut rt, false);
        assert!(rt.presented("narrow-hint-panel").is_none() && rt.presented("narrow-hint-rail").is_none());
        assert_eq!(read(&mut rt, |s| s.ui.shakes.count), 0);
    }
}
