//! A dropped pin, frame by frame: where the dropped row and its neighbours are drawn from a few
//! frames before the release to well after it, in the full sidebar and the rail, with and
//! without thumbnails, slow and quick drops, at scale 1 and 2. Real pointer events, and the
//! app's `poll` before each frame, as in the app.

use rune::{Env, Runtime};

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct Case {
    pub rail: bool,
    pub pictures: bool,
    pub quick: bool,
    pub scale: f32,
    /// Rows the bottom pin travels up: 1 or 2 (released 0.6 or 1.6 pitches up).
    pub rows: usize,
}

impl Case {
    pub(super) fn all() -> Vec<Case> {
        let mut v = Vec::new();
        for rail in [false, true] {
            for pictures in [false, true] {
                for quick in [false, true] {
                    for scale in [1.0, 2.0] {
                        for rows in [1, 2] {
                            v.push(Case { rail, pictures, quick, scale, rows });
                        }
                    }
                }
            }
        }
        v
    }
}

/// One frame: per pin (top to bottom, as before the drop) its drawn top, and the dropped one's
/// scale and opacity. `released`: the pointer was let go before this frame.
#[derive(Clone, Debug)]
pub(super) struct Frame {
    pub y: [f32; 3],
    pub scale: f32,
    pub opacity: f32,
    /// The dropped row's fill (r, g, b, a) and its shadow's alpha (0 with none).
    pub fill: [f32; 4],
    pub shadow: f32,
    /// The dropped row's painted top (its lifted fill, after pixel snapping), points; NaN once
    /// the fill has faded.
    pub painted: f32,
    pub released: bool,
}

pub(super) struct Track {
    pub frames: Vec<Frame>,
    /// Where the pointer left the dropped row: its top had it followed the pointer exactly.
    pub left_at: f32,
    /// The pins' rest tops, by slot.
    pub slots: [f32; 3],
    /// The dropped row's index in `y`, and the order after the drop (indices into `y`).
    pub order: [usize; 3],
}

fn rig(c: Case) -> (Runtime<State>, [String; 3]) {
    use crate::models::Artwork;
    let mut s = State::offline();
    (crate::scenes::SCENES.iter().find(|(n, _)| *n == "sidebar-pins").unwrap().1)(&mut s);
    s.settings.sidebar_collapsed = c.rail;
    if c.pictures {
        s.art.test_live(rune::ImageCache::new(std::sync::Arc::new(|| {})));
        let art = |i: usize| Artwork { template: format!("https://is1-ssl.mzstatic.com/image/thumb/pin{i}/{{w}}x{{h}}bb.jpg"), bg: None };
        s.settings.pins = (0..3)
            .map(|i| Pin { kind: PinKind::Playlist, id: format!("p.pin{i}"), title: format!("Pin {i}"), artwork: Some(art(i)) })
            .collect();
        for i in 0..3 {
            s.art.test_arrive(&art(i), crate::art::SMALL_PX, 8, [200, 40 * i as u8, 40]);
        }
    }
    let ids = [0, 1, 2].map(|i| s.settings.pins[i].id.clone());
    // Tall, so the rail doesn't scroll and `find`'s layout positions are where rows are drawn.
    let mut rt = Runtime::new(s, crate::ui::view, (1180.0, 1400.0), Env { system_dark: true, scale: c.scale });
    rt.poll = Some(crate::ui::poll);
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    (rt, ids)
}

/// The pin's button (the virtual list's slot has the same key).
fn button(rt: &Runtime<State>, id: &str) -> rune::ElementInfo {
    let key = format!("pin-{id}");
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str()) && e.role == "button" && !e.exiting).unwrap()
}

fn frame(rt: &Runtime<State>, ids: &[String; 3], k: f32, released: bool) -> Frame {
    let b = button(rt, &ids[2]);
    // The button inside the slot of the same key.
    let shadow = rt.presented_pose(&format!("pin-{0}/pin-{0}", ids[2])).map_or(0.0, |p| p.shadows().first().map_or(0.0, |s| s.color.a));
    let fill = [b.fill.r, b.fill.g, b.fill.b, b.fill.a];
    let painted = rt.instances.iter()
        .filter(|i| i.params[1] == rune::render::KIND_RECT && i.fill[3] > 0.5)
        .filter(|i| (i.rect[0] - b.rect[0] * k).abs() < 1.0 && (i.rect[3] - b.rect[3] * k).abs() < 1.0)
        .map(|i| i.rect[1] / k)
        .filter(|y| (y - b.rect[1]).abs() < 2.0)
        .min_by(|a, c| (a - b.rect[1]).abs().total_cmp(&(c - b.rect[1]).abs()))
        .unwrap_or(f32::NAN);
    Frame { y: [0, 1, 2].map(|i| button(rt, &ids[i]).rect[1]), scale: b.scale, opacity: b.opacity, fill, shadow, painted, released }
}

/// Lifts the bottom pin, drags it up and releases it; `c.quick` lets go right after it crosses
/// into the new slot, with the pointer still moving and the neighbours still sliding.
pub(super) fn track(c: Case) -> Track {
    let (mut rt, ids) = rig(c);
    let slots = [0, 1, 2].map(|i| button(&rt, &ids[i]).rect[1]);
    let pitch = slots[1] - slots[0];
    let y0 = slots[2];
    let (x, y) = rt.find(&format!("pin-{}", ids[2])).unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    let travel = pitch * (c.rows as f32 - 0.4);
    let mut frames = Vec::new();
    let mut at = 0.0;
    if c.quick {
        // ~7 pt a frame; lets go with the last move, as it crosses into the new slot's half.
        let steps = (travel / 7.0).ceil() as usize;
        for i in 1..=steps {
            at = travel * i as f32 / steps as f32;
            rt.pointer_move(x, y - at);
            if i == steps {
                rt.pointer_up(x, y - at);
            }
            rt.frame(1.0 / 60.0);
            frames.push(frame(&rt, &ids, c.scale, i == steps));
        }
    } else {
        for i in 1..=10 {
            at = travel * i as f32 / 10.0;
            rt.pointer_move(x, y - at);
            rt.frame(1.0 / 60.0);
        }
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
            frames.push(frame(&rt, &ids, c.scale, false));
        }
        rt.pointer_up(x, y - at);
    }
    let r = frames.iter().position(|f| f.released).unwrap_or(frames.len());
    frames.drain(..r.saturating_sub(5));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
        frames.push(frame(&rt, &ids, c.scale, true));
    }
    let mut ids_after = Vec::new();
    rt.update(|s, _| ids_after = s.settings.pins.iter().map(|p| p.id.clone()).collect());
    let order = [0, 1, 2].map(|k| ids.iter().position(|i| *i == ids_after[k]).unwrap());
    Track { frames, left_at: y0 - at, slots, order }
}

/// What a track measures. All in points. A bump is a jump of 0.5 pt or more between frames,
/// or a step back.
#[derive(Debug, Default)]
pub(super) struct Bumps {
    /// The dropped row's drawn top on the release's frame and the drop's (the next `poll`), less
    /// where the pointer left it; the largest of the two (positive: below).
    pub at_release: f32,
    /// The same for its painted top (after pixel snapping).
    pub painted_at_release: f32,
    /// The largest step the dropped row takes against its way home, or past its slot (drawn,
    /// and painted).
    pub dropped_back: f32,
    pub painted_back: f32,
    /// From the release on, the dropped row's largest change of step between frames, and what
    /// one spring from rest over that distance takes at most (`glide_jolt` is a jolt beyond that).
    pub glide_jolt: f32,
    pub glide_bound: f32,
    /// A neighbour's largest step against its way to its new slot, or past it.
    pub neighbour_back: f32,
    /// Slow drops: how far a neighbour moves after the release (it already rests in its slot).
    pub neighbour_drift: f32,
    /// Quick drops: a neighbour's largest change of step, and one spring's bound over a pitch.
    pub neighbour_jolt: f32,
    pub neighbour_bound: f32,
    /// The dropped row's largest scale or opacity change between frames.
    pub pose: f32,
    /// Its largest fill or shadow alpha change between frames (the lift ending).
    pub lift: f32,
    /// Where each row ends, less its slot.
    pub end: f32,
}

/// Steps against the way to `to` from where the row was at the release (`from`), or past `to`.
fn backwards(ys: &[f32], from: f32, to: f32) -> f32 {
    let dir = (to - from).signum();
    let mut worst = 0.0f32;
    for w in ys.windows(2) {
        let step = (w[1] - w[0]) * dir;
        if dir != 0.0 && step < 0.0 {
            worst = worst.max(-step);
        }
    }
    for &y in ys {
        if dir != 0.0 && (y - to) * dir > 0.0 {
            worst = worst.max((y - to) * dir);
        }
    }
    worst
}

/// The largest change of step between consecutive frames.
fn jolt(ys: &[f32]) -> f32 {
    ys.windows(3).map(|w| ((w[2] - w[1]) - (w[1] - w[0])).abs()).fold(0.0, f32::max)
}

/// The value of largest size.
fn worst(ds: impl Iterator<Item = f32>) -> f32 {
    ds.fold(0.0f32, |a, d| if d.abs() > a.abs() { d } else { a })
}

/// A critically damped spring from rest changes its step by at most ~6% of the distance a
/// frame at 60 Hz (`SETTLE` and Rune's layout spring alike); a little room for rounding.
fn spring_bound(distance: f32) -> f32 {
    0.07 * distance.abs() + 0.1
}

pub(super) fn measure(t: &Track, quick: bool) -> Bumps {
    let r = t.frames.iter().position(|f| f.released).unwrap();
    let col = |k: usize| t.frames.iter().map(|f| f.y[k]).collect::<Vec<_>>();
    let slot_of = |k: usize| t.slots[t.order.iter().position(|&o| o == k).unwrap()];
    let mut b = Bumps::default();
    let dropped = col(2);
    b.at_release = worst(dropped[r..r + 2].iter().map(|y| y - t.left_at));
    b.dropped_back = backwards(&dropped[r..], t.left_at, slot_of(2));
    b.glide_jolt = jolt(&dropped[r..]);
    b.glide_bound = spring_bound(t.left_at - slot_of(2));
    let painted: Vec<f32> = t.frames.iter().map(|f| f.painted).take_while(|y| !y.is_nan()).collect();
    b.painted_at_release = worst(painted[r..r + 2].iter().map(|y| y - t.left_at));
    b.painted_back = backwards(&painted[r..], t.left_at, slot_of(2));
    b.neighbour_bound = spring_bound(t.slots[1] - t.slots[0]);
    for k in 0..2 {
        let ys = col(k);
        b.neighbour_back = b.neighbour_back.max(backwards(&ys[r - 1..], ys[r - 1], slot_of(k)));
        if quick {
            b.neighbour_jolt = b.neighbour_jolt.max(jolt(&ys[r - 2..]));
        } else {
            b.neighbour_drift = b.neighbour_drift.max(ys[r..].iter().map(|y| (y - ys[r - 1]).abs()).fold(0.0, f32::max));
        }
    }
    for w in t.frames.windows(2) {
        b.pose = b.pose.max((w[1].scale - w[0].scale).abs()).max((w[1].opacity - w[0].opacity).abs());
        let fill = (0..4).map(|k| (w[1].fill[k] - w[0].fill[k]).abs()).fold(0.0, f32::max);
        b.lift = b.lift.max(fill).max((w[1].shadow - w[0].shadow).abs());
    }
    let last = t.frames.last().unwrap();
    b.end = (0..3).map(|k| (last.y[k] - slot_of(k)).abs()).fold(0.0, f32::max);
    b
}

/// The bumps a case shows, in words (empty: none).
pub(super) fn bumps(c: Case, b: &Bumps) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |bad: bool, what: &str| if bad { out.push(format!("{what}: {b:?}")) };
    // Pixel snapping may move a resting row by up to half a device pixel.
    check(b.at_release.abs() >= 0.5, "the dropped row is not drawn where the pointer left it");
    check(b.painted_at_release.abs() >= 0.5 / c.scale + 0.01, "the dropped row is not painted where the pointer left it");
    check(b.dropped_back > 0.05 || b.painted_back > 0.05, "the dropped row steps back or past its slot");
    check(b.glide_jolt > b.glide_bound, "the dropped row lurches (more than one spring from rest)");
    check(b.neighbour_back > 0.05, "a neighbour steps back or past its slot");
    check(b.neighbour_drift >= 0.05, "a neighbour moves after a slow drop");
    check(b.neighbour_jolt > b.neighbour_bound, "a neighbour lurches across a quick drop");
    check(b.pose > 0.001, "the dropped row's scale or opacity snaps");
    check(b.lift > 0.2, "the lift's fill or shadow snaps off");
    check(b.end > 0.05, "a row doesn't end in its slot");
    out
}

/// Released, a dragged pin is drawn (and painted) where the pointer left it, then glides to its
/// slot on one spring, never back or past it; its neighbours don't jump or creep, and its lift
/// (fill, shadow) fades. The full sidebar and the rail, pins with and without thumbnails, slow
/// and quick drops, one or two rows, at scale 1 and 2.
#[test]
fn a_dropped_pin_glides_home_without_a_bump() {
    let mut failed = Vec::new();
    for c in Case::all() {
        let b = measure(&track(c), c.quick);
        failed.extend(bumps(c, &b).into_iter().map(|w| format!("{c:?}: {w}")));
    }
    assert!(failed.is_empty(), "{} bumps:\n{}", failed.len(), failed.join("\n"));
}

/// Prints every case's track and measures (`cargo test -p uzumaki pin_drop_report -- --ignored --nocapture`).
#[test]
#[ignore]
fn pin_drop_report() {
    for c in Case::all() {
        let t = track(c);
        let b = measure(&t, c.quick);
        println!("{c:?}\n  {b:?}\n  bumps {:?}\n  left at {:.2}, slots {:?}", bumps(c, &b).iter().map(|w| w.split(':').next().unwrap_or("")).collect::<Vec<_>>(), t.left_at, t.slots);
        for f in &t.frames {
            println!("    {} y {:7.2} {:7.2} {:7.2}  scale {:.3} opacity {:.3} fill {:.3?} shadow {:.3} painted {:7.2}", if f.released { "R" } else { " " }, f.y[0], f.y[1], f.y[2], f.scale, f.opacity, f.fill, f.shadow, f.painted);
        }
    }
}
