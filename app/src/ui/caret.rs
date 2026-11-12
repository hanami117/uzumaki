//! The text fields' caret and selection. Rune draws both in the window's foreground, white
//! unless told otherwise, so on the light theme's white fields neither showed. Every field
//! takes its colours from [`ink`]: a coral caret and a soft coral selection, in both themes.
//!
//! Rune's caret is steady; ours blinks while a field has the keyboard, the way GTK's does:
//! solid after every edit or caret move, then on and off every [`HALF`] s, and solid again
//! after [`IDLE`] s without a change (no wake-ups at all from then on). With no field focused
//! nothing runs, and each blink is one `poll` returning true: no redraws in between.

use std::cell::Cell;
use std::time::Duration;

use rune::{Color, El, TextEdit, Tx};

use crate::state::State;
use crate::theme::{PAL, is_light};

/// One half of a blink (on, then off), in seconds.
pub const HALF: f64 = 0.53;
/// Seconds without a change after which the caret stops blinking and stays on.
pub const IDLE: f64 = 10.0;

/// The selection highlight's base colour: the coral accent of the dark palette, in both themes.
fn selection_base() -> Color {
    rune::color::hex("#ff7a8a").expect("hex")
}

thread_local! {
    /// Reduce Motion, from the last view (`poll` doesn't see it): a steady caret.
    static REDUCED: Cell<bool> = const { Cell::new(false) };
}

/// What restarts the blink: which field has the keyboard, its text, its caret and selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sig {
    field: u8,
    text: u64,
    anchor: usize,
    focus: usize,
}

impl Sig {
    fn of(field: u8, e: &TextEdit) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        e.text.hash(&mut h);
        Sig { field, text: h.finish(), anchor: e.anchor, focus: e.focus }
    }
}

#[derive(Debug)]
pub struct Blink {
    sig: Option<Sig>,
    since: f64,
    on: bool,
}

impl Default for Blink {
    fn default() -> Self {
        Blink { sig: None, since: 0.0, on: true }
    }
}

impl Blink {
    /// Whether the caret shows in this build.
    pub fn on(&self) -> bool {
        self.on
    }

    /// Steps the blink at poll time `t` for the focused field `sig` (None: no field has the
    /// keyboard). Returns whether the caret's look changed (the view must run) and when the
    /// next poll is due, if one is.
    pub fn step(&mut self, sig: Option<Sig>, t: f64, reduced: bool) -> (bool, Option<f64>) {
        if sig != self.sig {
            self.sig = sig;
            self.since = t;
            let was = std::mem::replace(&mut self.on, true);
            // Unfocused, Rune draws no caret, so its colour doesn't matter.
            let changed = sig.is_some() && !was;
            return (changed, (sig.is_some() && !reduced).then_some(HALF));
        }
        if sig.is_none() {
            return (false, None);
        }
        let elapsed = (t - self.since).max(0.0);
        if reduced || elapsed >= IDLE {
            return (!std::mem::replace(&mut self.on, true), None);
        }
        let phase = (elapsed / HALF) as u64;
        let on = phase % 2 == 0;
        let changed = on != self.on;
        self.on = on;
        let next = ((phase + 1) as f64 * HALF).min(IDLE) - elapsed;
        (changed, Some(next.max(0.0)))
    }
}

/// The field that has the keyboard, if one of ours does.
fn focused(s: &State) -> Option<Sig> {
    let [name, desc] = super::new_playlist::edits(&s.ui.new_pl);
    [&s.ui.search, &s.ui.filter.edit, &s.ui.settings.query, name, desc]
        .into_iter()
        .enumerate()
        .find(|(_, e)| e.focused)
        .map(|(i, e)| Sig::of(i as u8, e))
}

/// Blinks the caret of the focused field (`ui::poll`). True when the view must run.
pub fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let sig = focused(s);
    let (changed, next) = s.ui.caret.step(sig, t, REDUCED.get());
    if let Some(w) = next {
        tx.wake_after(Duration::from_secs_f64(w));
    }
    changed
}

/// Records Reduce Motion for `poll` (`ui::view`).
pub fn set_reduced(on: bool) {
    REDUCED.set(on);
}

/// The caret and selection colours of the current palette, the caret hidden in a blink's off
/// half.
pub fn colors(on: bool) -> (Color, Color) {
    let caret = if on { PAL.accent } else { PAL.accent.alpha(0.0) };
    (caret, selection_base().alpha(if is_light() { 0.32 } else { 0.38 }))
}

/// `field` (a `text_input`) with our caret and selection.
pub fn ink(field: El, s: &State) -> El {
    let (caret, selection) = colors(s.ui.caret.on() || REDUCED.get());
    field.caret(caret, selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(focus: usize) -> Option<Sig> {
        Some(Sig { field: 0, text: 1, anchor: focus, focus })
    }

    #[test]
    fn nothing_runs_without_a_focused_field() {
        let mut b = Blink::default();
        for k in 0..50 {
            assert_eq!(b.step(None, k as f64 * 0.1, false), (false, None));
        }
        assert!(b.on());
    }

    #[test]
    fn it_blinks_while_focused_and_asks_only_for_the_flips() {
        let mut b = Blink::default();
        assert_eq!(b.step(sig(3), 1.0, false), (false, Some(HALF)));
        // A poll between flips changes nothing and asks for the next flip.
        let (c, next) = b.step(sig(3), 1.2, false);
        assert!(!c && b.on());
        assert!((next.unwrap() - (HALF - 0.2)).abs() < 1e-9);
        let (c, _) = b.step(sig(3), 1.0 + HALF + 0.001, false);
        assert!(c && !b.on());
        let (c, _) = b.step(sig(3), 1.0 + 2.0 * HALF + 0.001, false);
        assert!(c && b.on());
    }

    #[test]
    fn an_edit_or_caret_move_shows_it_at_once() {
        let mut b = Blink::default();
        b.step(sig(3), 0.0, false);
        b.step(sig(3), HALF + 0.01, false);
        assert!(!b.on());
        let (c, next) = b.step(sig(4), HALF + 0.02, false);
        assert!(c && b.on());
        assert_eq!(next, Some(HALF));
        // Select all: anchor 0, focus at the end.
        b.step(sig(4), 2.0 * HALF + 0.03, false);
        assert!(!b.on());
        let (c, _) = b.step(Some(Sig { field: 0, text: 1, anchor: 0, focus: 4 }), 2.0 * HALF + 0.04, false);
        assert!(c && b.on());
    }

    #[test]
    fn it_stops_blinking_after_a_while_and_wakes_no_more() {
        let mut b = Blink::default();
        b.step(sig(1), 0.0, false);
        let mut t = 0.0;
        let mut wakes = 0;
        while let (_, Some(w)) = b.step(sig(1), t, false) {
            t += w + 1e-6;
            wakes += 1;
            assert!(wakes < 100, "never settles");
        }
        assert!(t >= IDLE && b.on());
        // About two polls a second for IDLE seconds, then none.
        assert!(wakes <= (IDLE / HALF) as usize + 2, "{wakes} wakes");
        assert_eq!(b.step(sig(1), t + 30.0, false), (false, None));
    }

    #[test]
    fn reduce_motion_keeps_it_steady() {
        let mut b = Blink::default();
        assert_eq!(b.step(sig(1), 0.0, true), (false, None));
        assert_eq!(b.step(sig(1), HALF + 0.1, true), (false, None));
        assert!(b.on());
    }

    #[test]
    fn losing_focus_stops_the_wakes() {
        let mut b = Blink::default();
        b.step(sig(1), 0.0, false);
        b.step(sig(1), HALF + 0.01, false);
        assert_eq!(b.step(None, HALF + 0.02, false), (false, None));
        assert!(b.on());
        assert_eq!(b.step(None, 5.0, false), (false, None));
    }

    #[test]
    fn the_colours_are_coral_and_the_caret_hides_when_off() {
        let (on, sel) = colors(true);
        let (off, sel2) = colors(false);
        assert_eq!(on, PAL.accent);
        assert_eq!(off.a, 0.0);
        assert_eq!(sel, sel2);
        assert!(sel.a > 0.2 && sel.a < 0.5);
        assert_eq!((sel.r, sel.g, sel.b), { let b = selection_base(); (b.r, b.g, b.b) });
    }
}
