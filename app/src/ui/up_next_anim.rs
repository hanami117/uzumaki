//! Smooth Up Next edits (FLIP). Rows of the virtual list land in their new places at once (every
//! bridge message commits `instant`, and the list sets `instant_changes`), so a song queued with
//! Play Next, a removed row or the next track starting would make the rows jump. Instead, the view
//! compares the rows around the viewport with those it drew last time: a row that moved is drawn
//! at its old place for one frame (a `Live` offset snapped there) and then springs to its new
//! one; a new row stays in its place and fades in once the rows below have mostly made room
//! (`FADE_DELAY`), so the gap opens first and nothing shows through it. Rows that left
//! are gone at once and the rows below slide up over their place.
//!
//! The springs run render-side (`Live::smooth`): no commit and no view run while they move, and
//! nothing at all once they rest. `poll` only releases the snapped values on the next frame.
//! First opens, replacements (a new queue or station) and edits too big to follow just show.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use rune::{Live, Spring};

/// Ease-out, no overshoot.
pub const MOTION: Spring = Spring::new(0.24, 0.0);
/// Rows above and below the viewport compared with the last drawing (more than any slide).
pub const MARGIN: usize = 8;
/// More new rows on screen than this, and the change is shown as it is.
const MAX_NEW: usize = 4;
/// More rows than this between a row's old and new place: no slide either.
const MAX_SHIFT: i64 = 6;
/// Distinct slides at once (insert: 1, reorder: 2).
const MAX_LANES: usize = 3;
/// A new row starts fading in this long after the rows below start making room for it.
const FADE_DELAY: f64 = 0.08;
/// After this, rows lose their bindings again (the springs are long done).
const SETTLED: Duration = Duration::from_millis(600);

/// How row `key` moves in the current animation.
#[derive(Clone)]
pub struct RowMotion {
    pub offset: Option<Live<[f32; 2]>>,
    pub fade: Option<Live<f32>>,
}

/// The keys of the rows around the viewport, as last drawn (or scrolled to).
#[derive(Default)]
struct Drawn {
    /// Index of `keys[0]` in the list.
    start: usize,
    keys: Vec<String>,
}

#[derive(Default)]
struct Inner {
    drawn: Option<Drawn>,
    rows: HashMap<String, RowMotion>,
    /// Snapped this frame; `poll` sends them to rest on the next one.
    offsets: Vec<Live<[f32; 2]>>,
    fades: Vec<Live<f32>>,
    /// Poll time at which `fades` go to 1 (set by the first poll after the change).
    fade_at: Option<f64>,
    started: Option<Instant>,
    /// Animations planned so far (for tests and `--trace` reading).
    planned: u64,
}

/// Up Next's row motion. Lives in `UiState`; the view writes it through `&State`.
#[derive(Default)]
pub struct UpNextAnim {
    inner: RefCell<Inner>,
    /// Rows visible in the viewport, from `on_visible_rows`.
    pub visible: Range<usize>,
    /// The viewport's height at the last view (for the handler that has no `Cx`).
    pub viewport_h: std::cell::Cell<f32>,
}

/// What the comparison decided.
#[derive(Debug, PartialEq)]
enum Plan {
    Nothing,
    /// Too big or unrelated: show it as it is.
    Show,
    /// key → (rows moved, as old minus new place relative to the anchor; new row).
    Move(Vec<(usize, i64, bool)>),
}

impl UpNextAnim {
    /// Forget what was drawn: the next drawing is a first one (the list opened).
    pub fn reset(&mut self) {
        let inner = self.inner.get_mut();
        inner.drawn = None;
        inner.rows.clear();
        inner.offsets.clear();
        inner.fades.clear();
        self.visible = 0..0;
    }

    /// The window around the viewport (`visible` ± `MARGIN`) after scrolling: the rows as they
    /// are now, with no motion.
    pub fn remember(&mut self, start: usize, keys: Vec<String>) {
        self.inner.get_mut().drawn = Some(Drawn { start, keys });
    }

    /// The rows around the viewport (`start` is the index of `keys[0]`) as this view draws them.
    /// Compares them with the last drawing and starts the motion. `row` is the row height.
    pub fn observe(&self, start: usize, keys: Vec<String>, row: f32) {
        let mut inner = self.inner.borrow_mut();
        let visible = self.visible.clone();
        if inner.started.is_some_and(|t| t.elapsed() > SETTLED) {
            inner.started = None;
            inner.rows.clear();
        }
        let plan = match &inner.drawn {
            Some(d) if d.start == start && d.keys == keys => Plan::Nothing,
            Some(d) => plan(d, start, &keys, &visible),
            None => Plan::Nothing,
        };
        match plan {
            Plan::Nothing => {}
            Plan::Show => {
                // Any running motion lands where it was going.
                for r in inner.rows.values() {
                    if let Some(o) = &r.offset { o.snap([0.0, 0.0]); }
                    if let Some(f) = &r.fade { f.snap(1.0); }
                }
                inner.rows.clear();
                inner.offsets.clear();
                inner.fades.clear();
            }
            Plan::Move(moves) => {
                let mut lanes: Vec<(i64, Live<[f32; 2]>)> = Vec::new();
                let fade = moves.iter().any(|m| m.2).then(|| Live::new(0.0f32));
                let mut rows = HashMap::new();
                for (i, delta, new) in moves {
                    let key = &keys[i];
                    let offset = (delta != 0 && !new).then(|| match lanes.iter().find(|l| l.0 == delta) {
                        Some(l) => l.1.clone(),
                        None => {
                            let live = Live::new([0.0, delta as f32 * row]);
                            lanes.push((delta, live.clone()));
                            live
                        }
                    });
                    // A row still fading in from the last change keeps fading.
                    let fade = if new { fade.clone() } else { inner.rows.get(key).and_then(|r| r.fade.clone()) };
                    if offset.is_some() || fade.is_some() {
                        rows.insert(key.clone(), RowMotion { offset, fade });
                    }
                }
                inner.offsets = lanes.into_iter().map(|l| l.1).collect();
                inner.fades = fade.into_iter().collect();
                inner.fade_at = None;
                inner.rows = rows;
                inner.started = Some(Instant::now());
                inner.planned += 1;
            }
        }
        inner.drawn = Some(Drawn { start, keys });
    }

    /// No row moves (the build can skip looking its key up).
    pub fn idle(&self) -> bool {
        self.inner.borrow().rows.is_empty()
    }

    /// How row `key` moves now, if it does.
    pub fn row(&self, key: &str) -> Option<RowMotion> {
        self.inner.borrow().rows.get(key).cloned()
    }

    /// Before each frame (`t`: poll time): rows drawn at their old places last frame now spring
    /// to their new ones, and new rows fade in a moment later. Never asks for a view run (the
    /// springs are render-side); a wake only while a fade waits.
    pub fn poll(&mut self, t: f64, tx: &mut rune::Tx) {
        if let Some(wait) = self.step(t) {
            tx.wake_after(wait);
        }
    }

    /// `poll` without its `Tx`: how long until the fade waiting should start, if one waits.
    pub fn step(&mut self, t: f64) -> Option<Duration> {
        let inner = self.inner.get_mut();
        for o in inner.offsets.drain(..) {
            o.set([0.0, 0.0]);
        }
        if inner.fades.is_empty() {
            return None;
        }
        let at = *inner.fade_at.get_or_insert(t + FADE_DELAY);
        if t + 1e-6 < at {
            return Some(Duration::from_secs_f64(at - t));
        }
        inner.fade_at = None;
        for f in inner.fades.drain(..) {
            f.set(1.0);
        }
        None
    }

    /// Something is waiting for `poll` (a frame).
    #[cfg(test)]
    pub fn pending(&self) -> bool {
        let inner = self.inner.borrow();
        !inner.offsets.is_empty() || !inner.fades.is_empty()
    }

    /// How many animations started so far.
    #[cfg(test)]
    pub fn planned(&self) -> u64 {
        self.inner.borrow().planned
    }
}

/// Each new row's `(index in keys, rows moved, new)`, or Show/Nothing. `visible`: the rows on
/// screen before the change (old indices).
fn plan(old: &Drawn, start: usize, keys: &[String], visible: &Range<usize>) -> Plan {
    if visible.is_empty() {
        return Plan::Show;
    }
    let old_at: HashMap<&str, usize> = old.keys.iter().enumerate().map(|(i, k)| (k.as_str(), old.start + i)).collect();
    let new_at: HashMap<&str, usize> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), start + i)).collect();
    // The virtual list keeps its first visible row in place: positions are relative to it.
    let Some((old_anchor, new_anchor)) = visible.clone()
        .filter_map(|o| old.keys.get(o.checked_sub(old.start)?))
        .find_map(|k| Some((old_at[k.as_str()], *new_at.get(k.as_str())?)))
    else {
        return Plan::Show;
    };
    // What was on screen should mostly still be there.
    let shown: Vec<&str> = visible.clone().filter_map(|o| old.keys.get(o.checked_sub(old.start)?)).map(|k| k.as_str()).collect();
    let kept = shown.iter().filter(|k| new_at.contains_key(*k)).count();
    if kept * 2 < shown.len() {
        return Plan::Show;
    }
    let mut moves: Vec<(usize, Option<i64>)> = keys.iter().enumerate().map(|(i, k)| {
        let rel_new = (start + i) as i64 - new_anchor as i64;
        (i, old_at.get(k.as_str()).map(|&o| (o as i64 - old_anchor as i64) - rel_new))
    }).collect();
    // On screen after the change: the anchor keeps its place.
    let on_screen = new_anchor..new_anchor + visible.len();
    let fresh = |i: usize| on_screen.contains(&(start + i));
    let new_on_screen = moves.iter().filter(|m| m.1.is_none() && fresh(m.0)).count();
    if new_on_screen > MAX_NEW {
        return Plan::Show;
    }
    // New rows ride with the rows after them (or before them, at the end).
    let known: Vec<Option<i64>> = moves.iter().map(|m| m.1).collect();
    for (i, m) in moves.iter_mut().enumerate() {
        if m.1.is_none() {
            m.1 = known[i..].iter().flatten().next().or_else(|| known[..i].iter().rev().flatten().next()).copied().or(Some(0));
        }
    }
    let mut lanes: Vec<i64> = moves.iter().filter_map(|m| m.1).filter(|&d| d != 0).collect();
    lanes.sort_unstable();
    lanes.dedup();
    if lanes.len() > MAX_LANES || lanes.iter().any(|d| d.abs() > MAX_SHIFT) {
        return Plan::Show;
    }
    let out: Vec<(usize, i64, bool)> = moves.into_iter().zip(known)
        .map(|((i, d), k)| (i, d.unwrap_or(0), k.is_none() && fresh(i)))
        .filter(|m| m.1 != 0 || m.2)
        .collect();
    if out.is_empty() { Plan::Nothing } else { Plan::Move(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(start: usize, keys: &[&str]) -> Drawn {
        Drawn { start, keys: keys.iter().map(|k| k.to_string()).collect() }
    }
    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn play_next_slides_the_rows_below_down_and_fades_the_new_one_in() {
        let old = drawn(0, &["now", "cur", "next", "a", "b", "c"]);
        let new = keys(&["now", "cur", "next", "x", "a", "b", "c"]);
        let Plan::Move(m) = plan(&old, 0, &new, &(0..6)) else { panic!() };
        // x is new and rides with a; a, b, c were one row higher.
        assert_eq!(m, vec![(3, -1, true), (4, -1, false), (5, -1, false), (6, -1, false)]);
    }

    #[test]
    fn the_next_track_slides_the_list_up() {
        let old = drawn(5, &["now", "cur", "next", "a", "b", "c"]);
        // History grew by one above (the anchor stays put), a became the current song.
        let new = keys(&["h9", "now", "cur", "next", "b", "c", "d"]);
        let Plan::Move(m) = plan(&old, 5, &new, &(5..11)) else { panic!() };
        assert!(m.contains(&(4, 1, false)) && m.contains(&(5, 1, false)), "{m:?}");
        assert!(!m.iter().any(|x| x.0 <= 3), "rows above stay: {m:?}");
    }

    #[test]
    fn a_new_queue_just_shows() {
        let old = drawn(0, &["now", "cur", "c-1", "c-2", "c-3", "c-4"]);
        let new = keys(&["now", "cur", "d-1", "d-2", "d-3", "d-4"]);
        assert_eq!(plan(&old, 0, &new, &(0..6)), Plan::Show);
    }

    #[test]
    fn an_album_queued_at_once_just_shows() {
        let old = drawn(0, &["now", "cur", "next", "a", "b", "c", "d", "e"]);
        let new = keys(&["now", "cur", "next", "1", "2", "3", "4", "5", "6"]);
        assert_eq!(plan(&old, 0, &new, &(0..8)), Plan::Show);
    }

    #[test]
    fn idle_redraws_plan_nothing_and_ask_for_no_frame() {
        let mut a = UpNextAnim { visible: 0..4, ..Default::default() };
        a.observe(0, keys(&["now", "cur", "a", "b"]), 56.0);
        a.observe(0, keys(&["now", "cur", "a", "b"]), 56.0);
        assert!(!a.pending() && a.planned() == 0);
        // Scrolling (`on_visible_rows`) remembers the new window, and the next view moves nothing.
        a.visible = 1..5;
        a.remember(1, keys(&["cur", "a", "b", "c"]));
        a.observe(1, keys(&["cur", "a", "b", "c"]), 56.0);
        assert!(!a.pending() && a.planned() == 0);
        a.observe(1, keys(&["cur", "x", "a", "b"]), 56.0);
        assert!(a.pending() && a.planned() == 1);
        assert!(a.row("x").is_some_and(|r| r.fade.is_some()) && a.row("a").is_some_and(|r| r.offset.is_some()));
        assert_eq!(a.row("a").unwrap().offset.unwrap().get(), [0.0, -56.0]);
        assert!(a.step(1.0).is_some(), "the new row waits a moment");
        assert!(a.pending());
        assert!(a.step(1.0 + FADE_DELAY).is_none());
        assert!(!a.pending());
        assert_eq!(a.row("a").unwrap().offset.unwrap().get(), [0.0, 0.0]);
        // The first drawing after a reset (the list opening) never animates.
        a.reset();
        a.visible = 0..4;
        a.observe(0, keys(&["now", "cur", "q", "r"]), 56.0);
        assert!(!a.pending() && a.planned() == 1);
    }

    #[test]
    fn a_removed_row_lets_the_rows_below_slide_up() {
        // d was in the margin below the viewport.
        let old = drawn(0, &["now", "cur", "a", "b", "c", "d"]);
        let new = keys(&["now", "cur", "a", "c", "d"]);
        let Plan::Move(m) = plan(&old, 0, &new, &(0..5)) else { panic!() };
        assert_eq!(m, vec![(3, 1, false), (4, 1, false)]); // both were a row lower
    }
}
