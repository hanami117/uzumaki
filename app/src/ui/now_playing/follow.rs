//! Lyrics bookkeeping shared by `poll` and the view: which line is current, the live values that
//! drive word fill and interlude dots without committing the view, and the scroll glide.
//!
//! Everything here is UI-thread state in a `thread_local` (the view is a pure function of `State`,
//! and `State` is not ours to extend); it only mirrors what `position_now()` implies.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use rune::{Live, Spring, Tx};

use crate::lyrics::{Lyrics, Word};

/// Where the current line sits in the lyrics viewport, as a fraction from the top.
pub const FOCUS: f32 = 0.3;
/// Apple-like glide: slow settle, no bounce. Used for the scroll and every line's own style.
pub const GLIDE: Spring = Spring::new(0.75, 0.0);
/// A line's brightness (container opacity) fading as it becomes current or stops being so.
pub const FADE: Spring = Spring::new(0.35, 0.0);
/// Manual scrolling pauses auto-follow this long.
const PAUSE: Duration = Duration::from_millis(3000);
/// Gaps between lines longer than this get the breathing dots.
pub const INTERLUDE: f64 = 4.0;
/// At most this many frames in a row a line change waits for a frame that animates.
pub const HOLD_MAX: u32 = 6;

/// Fill lives of every line of the song (empty for lines without word timing): 1 for lines
/// already sung, 0 for upcoming ones, the word fill for the current one.
struct WordLives {
    song: String,
    epoch: u64,
    lines: Vec<Vec<Live<f32>>>,
}

struct DotLives {
    song: String,
    gap: usize,
    epoch: u64,
    dots: Vec<Live<f32>>,
    breathe: Live<f32>,
}

#[derive(Default)]
struct Np {
    song: String,
    /// What the last poll saw (`None` outer = nothing yet).
    seen: Option<(Option<usize>, Option<usize>)>,
    t: f64,
    manual: Option<Instant>,
    ignore_manual_until: Option<Instant>,
    /// A glide back to the current line is owed.
    resume: bool,
    /// The next placement is an instant jump (opening, new song).
    snap: bool,
    /// Frames a line change has waited for a commit that isn't instant (see `step`).
    held: u32,
    tries: u32,
    tops: Vec<(usize, f32)>,
    vp_h: f32,
    words: Option<WordLives>,
    dots: Option<DotLives>,
    epoch: u64,
}

thread_local! {
    static NP: RefCell<Np> = RefCell::new(Np::default());
}

pub fn reset() {
    NP.with_borrow_mut(|n| {
        n.seen = None;
        n.snap = true;
        n.tries = 0;
        n.manual = None;
        n.resume = false;
        n.tops.clear();
    });
}

pub fn idle() {
    NP.with_borrow_mut(|n| {
        n.seen = None;
        n.dots = None;
    });
}

/// The user scrolled by hand (`on_scroll_offset` saw wheel/drag input).
pub fn mark_manual() {
    NP.with_borrow_mut(|n| {
        if n.ignore_manual_until.is_some_and(|u| Instant::now() < u) {
            return;
        }
        n.manual = Some(Instant::now());
        n.resume = true;
    });
}

/// A pending instant placement is claimed by whoever places the lyrics first (the active line's
/// first layout puts it in the very frame that draws it; `poll` is the fallback).
pub fn take_snap() -> bool {
    NP.with_borrow_mut(|n| {
        let was = n.snap;
        if was {
            n.snap = false;
            n.ignore_manual_until = Some(Instant::now() + Duration::from_millis(400));
        }
        was
    })
}

/// The user clicked a line: follow again at once.
pub fn user_seek() {
    NP.with_borrow_mut(|n| {
        n.manual = None;
        n.resume = true;
    });
}

/// The (line, gap) the last poll showed for `song`; the view draws that, not the clock, so a
/// line change `step` holds back a frame is held back in the view too.
pub fn shown(song: &str) -> Option<(Option<usize>, Option<usize>)> {
    NP.with_borrow(|n| if n.song == song { n.seen } else { None })
}

/// From the view: where lines sit in the content (layout space) and the viewport height.
pub fn publish(tops: Vec<(usize, f32)>, vp_h: f32) {
    NP.with_borrow_mut(|n| {
        n.tops = tops;
        n.vp_h = vp_h;
    });
}

/// Start and end of the instrumental gap before line `k` (0 = the intro), if long enough.
pub fn gap_span(lyr: &Lyrics, k: usize) -> Option<(f64, f64)> {
    let next = lyr.lines.get(k)?;
    let start = if k == 0 {
        0.0
    } else {
        let p = &lyr.lines[k - 1];
        let e = p.words.last().map_or(p.end, |w| w.end).max(p.end);
        if e <= p.start {
            return None;
        }
        e
    };
    (next.start - start > INTERLUDE).then_some((start, next.start))
}

/// The gap (index of the line it precedes) that `t` falls in, if any.
pub fn active_gap(lyr: &Lyrics, cur: Option<usize>, t: f64) -> Option<usize> {
    let k = cur.map_or(0, |i| i + 1);
    let (a, b) = gap_span(lyr, k)?;
    (t >= a && t < b).then_some(k)
}

/// 0..1 fill of a word at `t`, eased.
fn fill(w: &Word, t: f64) -> f32 {
    let d = (w.end - w.start).max(0.18);
    let x = ((t - w.start) / d).clamp(0.0, 1.0) as f32;
    x * x * (3.0 - 2.0 * x)
}

/// Sets every line's lives for time `t` with `cur` current.
fn set_all(wl: &WordLives, lyr: &Lyrics, cur: Option<usize>, t: f64) {
    for (i, (lives, line)) in wl.lines.iter().zip(&lyr.lines).enumerate() {
        let v = match cur {
            Some(c) if i == c => None,
            Some(c) if i < c => Some(1.0),
            _ => Some(0.0),
        };
        for (l, w) in lives.iter().zip(&line.words) {
            l.set(v.unwrap_or_else(|| fill(w, t)));
        }
    }
}

fn set_dots(d: &DotLives, span: (f64, f64), t: f64) {
    let p = ((t - span.0) / (span.1 - span.0)).clamp(0.0, 1.0) as f32;
    for (j, l) in d.dots.iter().enumerate() {
        l.set((p * 3.0 - j as f32).clamp(0.0, 1.0));
    }
    // Slow breathing, with a gentle swell in as the gap opens.
    let swell = (((t - span.0) / 0.6).clamp(0.0, 1.0)) as f32;
    let phase = (t * std::f64::consts::TAU / 2.6).sin() as f32;
    d.breathe.set((0.55 + 0.45 * swell) * (1.0 + 0.07 * phase));
}

/// Makes the fill lives of all lines of the song (once per song; initialised for `t`).
pub fn ensure_lives(song: &str, lyr: &Lyrics, t: f64) {
    NP.with_borrow_mut(|n| {
        if n.words.as_ref().is_some_and(|w| w.song == song && w.lines.len() == lyr.lines.len()) {
            return;
        }
        n.epoch += 1;
        let lines = lyr.lines.iter().map(|l| l.words.iter().map(|_| Live::new(0.0)).collect()).collect();
        let wl = WordLives { song: song.into(), epoch: n.epoch, lines };
        set_all(&wl, lyr, lyr.line_at(t), t);
        n.words = Some(wl);
    });
}

/// Generation of the lives: memoised lines rebuild when it changes.
pub fn lives_epoch() -> u64 {
    NP.with_borrow(|n| n.words.as_ref().map_or(0, |w| w.epoch))
}

pub fn line_lives(i: usize) -> Vec<Live<f32>> {
    NP.with_borrow(|n| n.words.as_ref().and_then(|w| w.lines.get(i).cloned()).unwrap_or_default())
}

pub fn dot_lives(song: &str, gap: usize, span: (f64, f64), t: f64) -> (u64, Vec<Live<f32>>, Live<f32>) {
    NP.with_borrow_mut(|n| dots_of(n, song, gap, span, t))
}

fn dots_of(n: &mut Np, song: &str, gap: usize, span: (f64, f64), t: f64) -> (u64, Vec<Live<f32>>, Live<f32>) {
    if let Some(d) = &n.dots {
        if d.song == song && d.gap == gap {
            return (d.epoch, d.dots.clone(), d.breathe.clone());
        }
    }
    n.epoch += 1;
    let d = DotLives { song: song.into(), gap, epoch: n.epoch, dots: (0..3).map(|_| Live::new(0.0)).collect(), breathe: Live::new(1.0) };
    set_dots(&d, span, t);
    let out = (d.epoch, d.dots.clone(), d.breathe.clone());
    n.dots = Some(d);
    out
}

/// One poll step. Updates live values, and issues the glide to the current line when it changed
/// (not while the user is scrolling by hand). True when the view must re-run.
///
/// `landed`: this frame's commit is instant (a bridge message landed, `State::landing`), which
/// would turn the glide and the lines' fades into a jump. A line change found then waits for
/// the next frame (at most `HOLD_MAX`); the first placement and a new song snap anyway.
pub fn step(song: &str, lyr: &Lyrics, t: f64, playing: bool, landed: bool, tx: &mut Tx) -> bool {
    NP.with_borrow_mut(|n| {
        if n.song != song {
            n.song = song.into();
            n.seen = None;
            n.snap = true;
            n.tries = 0;
            n.manual = None;
        }
        let cur = lyr.line_at(t);
        let gap = active_gap(lyr, cur, t);
        let mut dirty = n.seen != Some((cur, gap));
        if dirty && landed && n.seen.is_some() && !n.snap && n.held < HOLD_MAX {
            n.held += 1;
            tx.wake_after(Duration::ZERO);
            return false;
        }
        n.held = 0;
        let line_changed = n.seen.is_none_or(|(c, _)| c != cur);
        n.seen = Some((cur, gap));

        if (t - n.t).abs() > 1e-4 || dirty {
            n.t = t;
            if let Some(w) = n.words.as_ref().filter(|w| w.song == song && w.lines.len() == lyr.lines.len()) {
                if line_changed {
                    // Sung lines to 1, upcoming (and seeked-back-over) ones to 0, current = fill.
                    set_all(w, lyr, cur, t);
                } else if let Some(i) = cur {
                    for (l, wd) in w.lines[i].iter().zip(&lyr.lines[i].words) {
                        l.set(fill(wd, t));
                    }
                }
            }
            if let Some(k) = gap {
                if let Some(span) = gap_span(lyr, k) {
                    dots_of(n, song, k, span, t);
                    if let Some(d) = &n.dots {
                        set_dots(d, span, t);
                    }
                }
            }
        }

        let manual_left = n.manual.and_then(|m| PAUSE.checked_sub(m.elapsed()));
        if let Some(left) = manual_left {
            tx.wake_after(left + Duration::from_millis(20));
        } else {
            if n.manual.take().is_some() {
                n.resume = true;
            }
            if line_changed || n.resume || n.snap {
                let want = cur.unwrap_or(0);
                let top = if cur.is_none() { Some(0.0) } else { n.tops.iter().find(|(k, _)| *k == want).map(|(_, y)| *y) };
                match top {
                    Some(y) if n.vp_h > 0.0 || cur.is_none() => {
                        let y = (y - FOCUS * n.vp_h).max(0.0);
                        if n.snap {
                            n.ignore_manual_until = Some(Instant::now() + Duration::from_millis(400));
                            tx.scroll_to_instant("np-lyrics", 0.0, y);
                        } else {
                            // A glide asked for while the offset is within half a point of it
                            // lands as an instant jump, which Rune reports like a hand's: don't
                            // let our own scroll pause auto-follow.
                            n.ignore_manual_until = Some(Instant::now() + Duration::from_millis(150));
                            tx.scroll_to_with("np-lyrics", 0.0, y, GLIDE);
                        }
                        n.snap = false;
                        n.resume = false;
                        n.tries = 0;
                        dirty = true;
                    }
                    _ => {
                        // Layout isn't known yet (or the target line hasn't been laid out): retry.
                        n.resume = true;
                        n.tries += 1;
                        if n.tries < 60 {
                            tx.wake_after(Duration::from_millis(30));
                        }
                    }
                }
            }
        }
        let _ = playing;
        dirty
    })
}
