//! Drag and drop: reorder pins in the sidebar, and drag songs onto playlists or the queue.
//!
//! Rune delivers no hover to other elements while a press is held (hover is limited to the
//! captured element), but it does hand `on_pointer_move` to every element under the pointer,
//! captured or not, from the element's *presented* rect (scroll, springs and live offsets
//! included). So each drop target, built only while a drag is on, reports "the pointer is over
//! me"; the dragged row's own `on_drag` runs right after them in the same event and clears the
//! target when none reported. The release does the same once more, so the drop lands on what is
//! under the pointer even after an auto-scroll.
//!
//! Cheap by construction: the ghost and the lifted pin ride `Live` values (no view run per pointer
//! move), and every handler says `tx.live_only()` unless something the view reads changed
//! (drag started or ended, the target changed, a neighbouring pin stepped aside).

use std::time::Duration;

use rune::{Drag, El, Live, Phase, PointerEvent, ScrollPos, Spring, Tx, el, row, text};

use super::widgets::{Size, art};
use super::{lists, select};
use crate::models::Song;
use crate::settings::{Pin, PinKind};
use crate::state::State;
use crate::theme::*;

/// Pointer travel before a press on a song row becomes a drag (clicks keep working).
const SONG_SLOP: f32 = 5.0;
/// Pointer travel before a press on a pin becomes a drag.
const PIN_SLOP: f32 = 4.0;
/// Auto-scroll zone at the sidebar list's top and bottom edge, and its top speed (points / s).
const EDGE: f32 = 48.0;
const MAX_SPEED: f32 = 1100.0;
const TICK: Duration = Duration::from_millis(16);
/// A dropped or cancelled pin settles into place on this.
const SETTLE: Spring = Spring::new(0.3, 0.0);
/// How long a settling pin stays lifted above its neighbours.
const SETTLE_SECS: f64 = 0.45;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A library playlist, by id (a sidebar row or a pin).
    Playlist(String),
    /// The play queue (Up Next drawer, the player bar's queue button).
    Queue,
}

/// What the pointer is over during a song drag, and whether a drop there would do anything.
#[derive(Clone, Debug, PartialEq)]
pub struct Over {
    pub target: Target,
    pub ok: bool,
}

pub struct PinDrag {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub pitch: f32,
}

enum Settle {
    None,
    /// Next poll: let the dropped pin glide from where it was released to its slot.
    Kick,
    /// Stop lifting it at this time.
    Clear(f64),
}

pub struct Dnd {
    /// A song drag is on (past the threshold).
    pub active: bool,
    songs: Vec<Song>,
    pub over: Option<Over>,
    /// A target heard this pointer move.
    seen: bool,
    /// The ghost's top-left, window points.
    pub ghost: Live<[f32; 2]>,
    /// The sidebar list's `[offset, max offset, viewport height, 0]`.
    scroll: Live<[f32; 4]>,
    sb_seen: bool,
    scroll_v: f32,
    last_t: f64,
    /// The drag's press must not also act as a click (cleared two polls later if nothing took it).
    swallow: bool,
    swallow_aged: bool,
    /// Bumped whenever the view must rebuild sidebar rows.
    pub rev: u64,
    pub pin: Option<PinDrag>,
    /// The pin that is lifted (dragged or settling).
    lifted: Option<String>,
    pin_dy: Live<[f32; 2]>,
    settle: Settle,
    /// Drops that would have been sent (tests have no bridge).
    #[cfg(test)]
    pub log: Vec<(String, usize)>,
}

impl Default for Dnd {
    fn default() -> Self {
        Self {
            active: false,
            songs: Vec::new(),
            over: None,
            seen: false,
            ghost: Live::new([0.0, 0.0]),
            scroll: Live::new([0.0; 4]),
            sb_seen: false,
            scroll_v: 0.0,
            last_t: 0.0,
            swallow: false,
            swallow_aged: false,
            rev: 0,
            pin: None,
            lifted: None,
            pin_dy: Live::new([0.0, 0.0]),
            settle: Settle::None,
            #[cfg(test)]
            log: Vec::new(),
        }
    }
}

impl Dnd {
    /// Rows of the sidebar depend on this.
    pub fn version(&self) -> (u64, bool, bool) {
        (self.rev, self.active, self.lifted.is_some())
    }

    /// Call from a click handler: true when the click is the tail of a drag and must be ignored.
    pub fn swallow_click(&mut self) -> bool {
        std::mem::take(&mut self.swallow)
    }

    /// The label and lead song of the ghost.
    fn label(&self) -> String {
        match self.songs.as_slice() {
            [one] => one.title.clone(),
            many => format!("{} songs", many.len()),
        }
    }
}

fn dist(d: &Drag) -> f32 {
    d.dx.hypot(d.dy)
}

// ---- pins ----

/// Gives a pin row its drag-to-reorder: lifted above its neighbours following the pointer, the
/// neighbours stepping aside.
pub fn pin_row(s: &State, e: El, k: usize, pin: &Pin) -> El {
    let dd = &s.ui.dnd;
    let id = pin.id.clone();
    let mut e = e.on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| pin_drag(s, d, tx, &id, k));
    if dd.lifted.as_deref() == Some(pin.id.as_str()) {
        e = e.raise().fill(PAL.nav_active).shadow(4.0, 14.0, crate::theme::shadow(0.45)).cursor(rune::Cursor::Grabbing)
            .live_offset(dd.pin_dy.smooth(SETTLE));
    } else if let Some(p) = &dd.pin {
        let shift = if p.from < k && k <= p.to {
            -p.pitch
        } else if p.to <= k && k < p.from {
            p.pitch
        } else {
            0.0
        };
        e = e.offset(0.0, shift);
    }
    e
}

fn pin_drag(s: &mut State, d: &mut Drag, tx: &mut Tx, id: &str, k: usize) {
    let n = s.settings.pins.len();
    match d.phase {
        Phase::Down => {
            s.ui.dnd.swallow = false;
            tx.live_only();
        }
        Phase::Move => {
            let dd = &mut s.ui.dnd;
            let Some(p) = dd.pin.as_mut() else {
                if dist(d) < PIN_SLOP || n < 2 {
                    tx.live_only();
                    return;
                }
                dd.pin = Some(PinDrag { id: id.to_string(), from: k, to: k, pitch: d.h.max(1.0) });
                dd.lifted = Some(id.to_string());
                dd.settle = Settle::None;
                dd.pin_dy.snap([0.0, d.dy]);
                dd.rev += 1;
                tx.instant();
                return;
            };
            dd.pin_dy.snap([0.0, d.dy]);
            let to = (p.from as f32 + (d.dy / p.pitch).round()).clamp(0.0, (n - 1) as f32) as usize;
            if to != p.to {
                p.to = to;
                dd.rev += 1;
                tx.animated();
            } else {
                tx.live_only();
            }
        }
        Phase::Up => {
            let Some(p) = s.ui.dnd.pin.take() else {
                tx.live_only();
                return;
            };
            let moved = !d.cancelled() && p.to != p.from;
            // Where the row sits relative to the slot it lands in, so it glides in from there.
            let residual = if moved { d.dy - (p.to as f32 - p.from as f32) * p.pitch } else { d.dy };
            if moved {
                s.move_pin_to(&p.id, p.to);
            }
            let dd = &mut s.ui.dnd;
            dd.pin_dy.snap([0.0, residual]);
            dd.settle = Settle::Kick;
            dd.swallow = true;
            dd.swallow_aged = false;
            dd.rev += 1;
            tx.animated();
            tx.wake_after(TICK);
        }
    }
}

// ---- songs ----

/// Press-and-drag on song row `key`: drags the selection when the row is in it, else just the
/// row. A song selected in two rows is dragged once.
pub fn song_drag(s: &mut State, d: &mut Drag, tx: &mut Tx, list: &str, key: &str) {
    match d.phase {
        Phase::Down => {
            s.ui.dnd.swallow = false;
            tx.live_only();
        }
        Phase::Move => {
            if !s.ui.dnd.active {
                if dist(d) >= SONG_SLOP && start(s, d, list, key) {
                    tx.instant();
                } else {
                    tx.live_only();
                }
                return;
            }
            let [x, y] = d.window_point();
            let dd = &mut s.ui.dnd;
            dd.ghost.set([x + 14.0, y + 12.0]);
            let mut changed = false;
            if !dd.seen && dd.over.is_some() {
                dd.over = None;
                dd.rev += 1;
                changed = true;
            }
            dd.seen = false;
            if !dd.sb_seen {
                dd.scroll_v = 0.0;
            }
            dd.sb_seen = false;
            if changed {
                tx.instant();
            } else {
                tx.live_only();
            }
        }
        Phase::Up => {
            let dd = &mut s.ui.dnd;
            if !dd.active {
                tx.live_only();
                return;
            }
            let (over, songs) = (dd.over.take(), std::mem::take(&mut dd.songs));
            dd.active = false;
            dd.seen = false;
            dd.sb_seen = false;
            dd.scroll_v = 0.0;
            dd.swallow = true;
            dd.swallow_aged = false;
            dd.rev += 1;
            tx.wake_after(Duration::from_millis(50));
            if !d.cancelled()
                && let Some(Over { target, ok: true }) = over
            {
                drop_on(s, &target, &songs);
            }
            tx.instant();
        }
    }
}

fn start(s: &mut State, d: &Drag, list: &str, key: &str) -> bool {
    if lists::list_id(s).as_deref() != Some(list) {
        return false;
    }
    if !s.ui.sel.has(list, key) {
        select::click(s, list, key, rune::Mods::default());
    }
    let songs = select::selected(s);
    if songs.is_empty() {
        return false;
    }
    let [x, y] = d.window_point();
    let dd = &mut s.ui.dnd;
    dd.songs = songs;
    dd.active = true;
    dd.over = None;
    dd.seen = false;
    dd.sb_seen = false;
    dd.scroll_v = 0.0;
    dd.ghost.snap([x + 14.0, y + 12.0]);
    dd.rev += 1;
    true
}

fn drop_on(s: &mut State, target: &Target, songs: &[Song]) {
    match target {
        Target::Queue => s.add_to_queue(songs),
        Target::Playlist(id) => {
            if let Some(c) = s.library_playlists.items.iter().find(|c| c.id == *id && c.editable).cloned() {
                #[cfg(test)]
                s.ui.dnd.log.push((c.id.clone(), songs.len()));
                s.add_to_playlist(&c, songs);
            }
        }
    }
}

fn hover(target: Target, ok: bool) -> impl Fn(&mut State, PointerEvent, &mut Tx) + 'static {
    move |s: &mut State, _p: PointerEvent, tx: &mut Tx| {
        let dd = &mut s.ui.dnd;
        if !dd.active {
            tx.live_only();
            return;
        }
        dd.seen = true;
        let now = Over { target: target.clone(), ok };
        if dd.over.as_ref() != Some(&now) {
            dd.over = Some(now);
            dd.rev += 1;
            tx.instant();
        } else {
            tx.live_only();
        }
    }
}

/// Is `id` a library playlist songs can be added to?
pub fn playlist_ok(s: &State, id: &str) -> bool {
    s.library_playlists.items.iter().any(|c| c.id == id && c.editable)
}

/// 0 when no song drag, else 1 + 2 * (over this row) + (not allowed): the part of a row's look
/// that depends on the drag, for memo versions.
pub fn row_state(s: &State, id: &str, ok: bool) -> u8 {
    let dd = &s.ui.dnd;
    if !dd.active {
        return 0;
    }
    let me = dd.over.as_ref().is_some_and(|o| o.target == Target::Playlist(id.to_string()));
    1 + 2 * u8::from(me) + u8::from(!ok)
}

/// Makes a sidebar row a drop target for the playlist `id` while songs are dragged: reports the
/// pointer, lights up with an accent outline when it can take them, and shows "not allowed"
/// when it can't.
pub fn playlist_target(s: &State, e: El, id: &str) -> El {
    let dd = &s.ui.dnd;
    if !dd.active {
        return e;
    }
    let ok = playlist_ok(s, id);
    let target = Target::Playlist(id.to_string());
    let me = dd.over.as_ref().is_some_and(|o| o.target == target);
    let e = e.on_pointer_move_local(hover(target, ok));
    match (ok, me) {
        (true, true) => e.fill(crate::theme::drop_fill(0.2)).border(1.5, PAL.accent),
        (true, false) => e,
        (false, true) => e.opacity(0.5).border(1.0, PAL.danger_text.alpha(0.7)).cursor(rune::Cursor::NotAllowed),
        (false, false) => e.opacity(0.5),
    }
}

/// The queue as a drop target (Up Next drawer, the queue button): `lit` styles it for the
/// element's own look.
pub fn queue_target(s: &State, e: El, lit: impl FnOnce(El) -> El) -> El {
    let dd = &s.ui.dnd;
    if !dd.active {
        return e;
    }
    let me = dd.over.as_ref().is_some_and(|o| o.target == Target::Queue);
    let e = e.on_pointer_move_local(hover(Target::Queue, true));
    if me { lit(e) } else { e }
}

/// The sidebar list's scroll viewport during a song drag: reports its scroll position, and
/// auto-scrolls when the pointer is near its top or bottom edge.
pub fn sidebar_scroll(s: &State, e: El) -> El {
    let live = s.ui.dnd.scroll.clone();
    let e = e.on_scroll_offset(move |at: ScrollPos| {
        live.set([at.offset[1], at.max[1], at.viewport[1], 0.0]);
    });
    if !s.ui.dnd.active {
        return e;
    }
    e.on_pointer_move_local(|s: &mut State, p: PointerEvent, tx: &mut Tx| {
        let dd = &mut s.ui.dnd;
        if !dd.active {
            tx.live_only();
            return;
        }
        dd.sb_seen = true;
        let h = dd.scroll.get()[2];
        let ramp = |depth: f32| (depth / EDGE).clamp(0.0, 1.0).powf(1.5) * MAX_SPEED;
        let v = if p.y < EDGE {
            -ramp(EDGE - p.y)
        } else if h > 0.0 && h - p.y < EDGE {
            ramp(EDGE - (h - p.y))
        } else {
            0.0
        };
        let was = dd.scroll_v != 0.0;
        dd.scroll_v = v;
        if v != 0.0 && !was {
            tx.wake_after(TICK);
        }
        tx.live_only();
    })
}

/// `AppSpec::poll`'s share: auto-scroll ticks and a dropped pin settling. True when the view
/// must rebuild.
pub fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let dt = (t - s.ui.dnd.last_t).clamp(0.0, 0.05) as f32;
    s.ui.dnd.last_t = t;
    let dd = &mut s.ui.dnd;
    if dd.swallow && std::mem::replace(&mut dd.swallow_aged, true) {
        dd.swallow = false;
        dd.swallow_aged = false;
    }
    let mut dirty = false;
    if dd.active && dd.scroll_v != 0.0 {
        let [off, max, _, _] = dd.scroll.get();
        let to = (off + dd.scroll_v * dt.max(0.001)).clamp(0.0, max);
        if (to - off).abs() > 0.01 {
            tx.scroll_to_instant("sb-scroll", 0.0, to);
            // Rows slide under a still pointer: nothing is "over" until it moves again.
            if dd.over.take().is_some() {
                dd.rev += 1;
                dirty = true;
            }
        }
        tx.wake_after(TICK);
    }
    match dd.settle {
        Settle::Kick => {
            dd.pin_dy.set([0.0, 0.0]);
            dd.settle = Settle::Clear(t + SETTLE_SECS);
            tx.wake_after(Duration::from_secs_f64(SETTLE_SECS + 0.02));
        }
        Settle::Clear(at) if t >= at => {
            dd.settle = Settle::None;
            dd.lifted = None;
            dd.rev += 1;
            dirty = true;
        }
        Settle::Clear(_) => tx.wake_after(Duration::from_millis(100)),
        Settle::None => {}
    }
    dirty
}

/// The drag ghost: a small cover and "3 songs" or the title, following the pointer by a live
/// offset (no view run per move).
pub fn ghost(s: &State) -> El {
    let dd = &s.ui.dnd;
    let lead = dd.songs.first();
    let cover = match lead {
        Some(song) => art(s, song.artwork.as_ref(), &song.album, Size::Small(26.0), 5.0, 0.0),
        None => el().size(26.0, 26.0),
    };
    row().key("dnd-ghost").absolute(0.0, 0.0).raise().live_offset(&dd.ghost).max_w(280.0).gap(8.0).center_y()
        .pl(5.0).pr(13.0).py(5.0).radius(10.0).fill(crate::theme::pop_fill()).border(1.0, crate::theme::pop_border()).opacity(0.97)
        .shadow(8.0, 22.0, crate::theme::shadow(0.5))
        .child(cover)
        .child(text(dd.label()).font(TRACK_TITLE).color(PAL.text).truncate().min_w(0.0))
}

// ---- scenes ----

/// Scene: pin `from` lifted `dy` points, dropping at `to`.
pub fn scene_drag_pin(s: &mut State, from: usize, to: usize, dy: f32) {
    let id = s.settings.pins[from].id.clone();
    let dd = &mut s.ui.dnd;
    dd.pin = Some(PinDrag { id: id.clone(), from, to, pitch: 34.0 });
    dd.lifted = Some(id);
    dd.pin_dy.snap([0.0, dy]);
    dd.rev += 1;
}

/// Scene: the selected songs mid-drag, ghost at `at`, over `over`.
pub fn scene_drag_songs(s: &mut State, at: [f32; 2], over: Option<Over>) {
    let songs = select::selected(s);
    let dd = &mut s.ui.dnd;
    dd.songs = songs;
    dd.active = true;
    dd.over = over;
    dd.ghost.snap(at);
    dd.rev += 1;
}

pub fn is_pin_playlist(p: &Pin) -> bool {
    p.kind == PinKind::Playlist
}


#[cfg(test)]
mod tests {
    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    use super::*;

    fn rt(scene: &str) -> Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, crate::ui::view, (1180.0, 900.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt
    }

    fn step(rt: &mut Runtime<State>) {
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
    }

    /// Presses on `from`, moves in a few steps to `to` (x, y), and releases there unless `hold`.
    fn drag(rt: &mut Runtime<State>, from: &str, to: (f32, f32), release: bool) {
        let (x, y) = rt.find(from).unwrap_or_else(|| panic!("no element {from}"));
        rt.pointer_move(x, y);
        step(rt);
        rt.pointer_down(x, y);
        step(rt);
        for i in 1..=8 {
            let f = i as f32 / 8.0;
            rt.pointer_move(x + (to.0 - x) * f, y + (to.1 - y) * f);
            step(rt);
        }
        if release {
            rt.pointer_up(to.0, to.1);
            step(rt);
        }
    }

    fn center(rt: &Runtime<State>, key: &str) -> (f32, f32) {
        rt.find(key).unwrap_or_else(|| panic!("no element {key}"))
    }

    fn read<T>(rt: &mut Runtime<State>, f: impl FnOnce(&State) -> T) -> T {
        let mut out = None;
        rt.update(|s, _| out = Some(f(s)));
        out.unwrap()
    }

    fn pin_ids(rt: &mut Runtime<State>) -> Vec<String> {
        read(rt, |s| s.settings.pins.iter().map(|p| p.id.clone()).collect())
    }

    #[test]
    fn dragging_the_third_pin_to_the_top_reorders_and_does_not_open_it() {
        let mut rt = rt("sidebar-pins");
        assert_eq!(pin_ids(&mut rt), ["a2", "ar12", "ra.3"]);
        let (x, y) = center(&rt, "pin-ra.3");
        drag(&mut rt, "pin-ra.3", (x, y - 80.0), true);
        assert_eq!(pin_ids(&mut rt), ["ra.3", "a2", "ar12"]);
        // Neither the drag's click nor the station started.
        assert!(read(&mut rt, |s| !s.ui.dnd.swallow));
        assert!(read(&mut rt, |s| s.ui.dnd.pin.is_none()));
    }

    /// The sidebar is often the collapsed rail: pins reorder there too. (A tall
    /// window, so the rail doesn't scroll: `Runtime::find` gives layout positions.)
    #[test]
    fn pins_reorder_in_the_collapsed_rail_too() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "sidebar-pins").unwrap().1)(&mut s);
        s.settings.sidebar_collapsed = true;
        let mut rt = Runtime::new(s, crate::ui::view, (1180.0, 1400.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        step(&mut rt);
        assert_eq!(pin_ids(&mut rt), ["a2", "ar12", "ra.3"]);
        let (x, y) = center(&rt, "pin-ra.3");
        drag(&mut rt, "pin-ra.3", (x, y - 80.0), true);
        assert_eq!(pin_ids(&mut rt), ["ra.3", "a2", "ar12"]);
        assert!(read(&mut rt, |s| s.ui.dnd.pin.is_none()));
    }

    /// The drawn y of a pin row, frame by frame (`rect` is where it paints, offset included).
    /// The virtual list's slot has the same key; the row itself is the button.
    fn pin_y(rt: &Runtime<State>, id: &str) -> f32 {
        let key = format!("pin-{id}");
        rt.elements().iter().find(|e| e.key.as_deref() == Some(key.as_str()) && e.role == "button" && !e.exiting).map(|e| e.rect[1]).unwrap()
    }

    /// Three pinned playlists whose thumbnails have landed (as in a real sidebar), in a window
    /// with live covers.
    fn rt_pinned_pictures() -> Runtime<State> {
        use crate::models::Artwork;
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "sidebar-pins").unwrap().1)(&mut s);
        s.art.test_live(rune::ImageCache::new(std::sync::Arc::new(|| {})));
        let art = |i: usize| Artwork { template: format!("https://is1-ssl.mzstatic.com/image/thumb/pin{i}/{{w}}x{{h}}bb.jpg"), bg: None };
        s.settings.pins = (0..3)
            .map(|i| Pin { kind: PinKind::Playlist, id: format!("p.pin{i}"), title: format!("Pin {i}"), artwork: Some(art(i)) })
            .collect();
        for i in 0..3 {
            s.art.test_arrive(&art(i), crate::art::SMALL_PX, 8, [200, 40 * i as u8, 40]);
        }
        let mut rt = Runtime::new(s, crate::ui::view, (1180.0, 900.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        // Past the thumbnails' first fade-in.
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    /// While a pin is dragged past its neighbours, they slide aside over several frames
    /// instead of jumping to their new slot, with and without loaded thumbnails (`Art::held`).
    #[test]
    fn neighbours_slide_aside_while_a_pin_is_dragged() {
        for pictures in [false, true] {
            let (mut rt, ids) = if pictures {
                (rt_pinned_pictures(), ["p.pin0", "p.pin1", "p.pin2"])
            } else {
                (rt("sidebar-pins"), ["a2", "ar12", "ra.3"])
            };
            let [first, middle, last] = ids.map(|i| format!("pin-{i}"));
            let (x, y) = center(&rt, &last);
            let pitch = center(&rt, &middle).1 - center(&rt, &first).1;
            rt.pointer_move(x, y);
            step(&mut rt);
            rt.pointer_down(x, y);
            // Lift it (past the slop), still in its own slot.
            rt.pointer_move(x, y - 8.0);
            step(&mut rt);
            let start = pin_y(&rt, ids[1]);
            // Over the row above: the middle one steps down by one pitch.
            rt.pointer_move(x, y - pitch * 0.9);
            let mut ys = Vec::new();
            for _ in 0..30 {
                rt.frame(1.0 / 60.0);
                ys.push(pin_y(&rt, ids[1]) - start);
            }
            assert!((ys[29] - pitch).abs() < 0.5, "pictures {pictures}: the middle pin reaches its new slot: {ys:?}");
            let between = ys.iter().filter(|d| **d > 0.5 && **d < pitch - 0.5).count();
            assert!(between >= 4, "pictures {pictures}: the middle pin slides over several frames, not a jump: {ys:?}");
            rt.pointer_up(x, y - pitch * 0.9);
            step(&mut rt);
        }
    }

    #[test]
    fn escape_puts_a_dragged_pin_back() {
        let mut rt = rt("sidebar-pins");
        let (x, y) = center(&rt, "pin-ra.3");
        drag(&mut rt, "pin-ra.3", (x, y - 80.0), false);
        // It is lifted, and its target is the top slot (the others step aside for it).
        assert_eq!(read(&mut rt, |s| s.ui.dnd.pin.as_ref().map(|p| (p.from, p.to))), Some((2, 0)));
        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        step(&mut rt);
        assert_eq!(pin_ids(&mut rt), ["a2", "ar12", "ra.3"]);
    }

    #[test]
    fn a_click_without_a_drag_selects_and_a_pin_click_still_opens() {
        let mut rt = rt("playlist");
        let (x, y) = center(&rt, "t2-pl-2");
        rt.pointer_move(x, y);
        step(&mut rt);
        rt.pointer_down(x, y);
        rt.pointer_move(x + 2.0, y + 1.0);
        rt.pointer_up(x + 2.0, y + 1.0);
        step(&mut rt);
        assert_eq!(read(&mut rt, |s| s.ui.sel.keys.iter().cloned().collect::<Vec<_>>()), ["pl-2"]);
        assert!(read(&mut rt, |s| !s.ui.dnd.active));

        let mut rt = self::rt("sidebar-pins");
        let (x, y) = center(&rt, "pin-a2");
        rt.pointer_move(x, y);
        step(&mut rt);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        step(&mut rt);
        assert!(read(&mut rt, |s| matches!(&s.route, crate::state::Route::Collection { id, .. } if id == "a2")));
    }

    fn two_selected(rt: &mut Runtime<State>) {
        rt.update(|s, _| crate::ui::scene_select(s, &["pl-4", "pl-5"], None));
        step(rt);
    }

    #[test]
    fn dropping_two_selected_rows_on_an_editable_playlist_adds_them() {
        let mut rt = rt("selection");
        two_selected(&mut rt);
        let to = center(&rt, "pl-p.1");
        drag(&mut rt, "t5-pl-5", to, false);
        assert!(read(&mut rt, |s| s.ui.dnd.active));
        assert_eq!(read(&mut rt, |s| s.ui.dnd.label()), "2 songs");
        assert_eq!(read(&mut rt, |s| s.ui.dnd.over.clone()), Some(Over { target: Target::Playlist("p.1".into()), ok: true }));
        rt.pointer_up(to.0, to.1);
        step(&mut rt);
        assert_eq!(read(&mut rt, |s| s.ui.dnd.log.clone()), [("p.1".to_string(), 2)]);
        assert!(read(&mut rt, |s| !s.ui.dnd.active && s.ui.dnd.over.is_none()));
        // Selection survives; the drop was not a click that reselected one row.
        assert_eq!(read(&mut rt, |s| s.ui.sel.keys.len()), 2);
    }

    #[test]
    fn an_unselected_row_drags_alone_and_selects_itself() {
        let mut rt = rt("selection");
        let to = center(&rt, "pl-p.2");
        drag(&mut rt, "t1-pl-1", to, true);
        assert_eq!(read(&mut rt, |s| s.ui.dnd.log.clone()), [("p.2".to_string(), 1)]);
        assert_eq!(read(&mut rt, |s| s.ui.sel.keys.iter().cloned().collect::<Vec<_>>()), ["pl-1"]);
    }

    #[test]
    fn a_drop_on_a_non_editable_playlist_or_nowhere_is_ignored() {
        let mut rt = rt("selection");
        two_selected(&mut rt);
        let to = center(&rt, "pl-p.90");
        drag(&mut rt, "t5-pl-5", to, false);
        assert_eq!(read(&mut rt, |s| s.ui.dnd.over.clone()), Some(Over { target: Target::Playlist("p.90".into()), ok: false }));
        rt.pointer_up(to.0, to.1);
        step(&mut rt);
        assert!(read(&mut rt, |s| s.ui.dnd.log.is_empty()));

        // Off every target.
        drag(&mut rt, "t5-pl-5", (700.0, 300.0), true);
        assert!(read(&mut rt, |s| s.ui.dnd.log.is_empty() && !s.ui.dnd.active));
    }

    #[test]
    fn escape_cancels_a_song_drag() {
        let mut rt = rt("selection");
        two_selected(&mut rt);
        let to = center(&rt, "pl-p.1");
        drag(&mut rt, "t5-pl-5", to, false);
        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        step(&mut rt);
        rt.pointer_up(to.0, to.1);
        step(&mut rt);
        assert!(read(&mut rt, |s| s.ui.dnd.log.is_empty() && !s.ui.dnd.active));
    }

    #[test]
    fn dropping_on_the_up_next_tab_or_queue_button_queues_the_songs() {
        for target in ["up-next", "queue"] {
            let mut rt = rt("selection");
            two_selected(&mut rt);
            rt.update(|s, _| {
                s.settings.right_panel = true;
                s.settings.right_tab = crate::settings::RightTab::Queue;
            });
            // The panel slides in; let it settle.
            for _ in 0..40 {
                step(&mut rt);
            }
            let before = read(&mut rt, |s| s.queue.user.len());
            let to = center(&rt, target);
            drag(&mut rt, "t5-pl-5", to, true);
            assert_eq!(read(&mut rt, |s| s.queue.user.len()), before + 2, "{target}");
        }
    }

    #[test]
    fn holding_near_the_bottom_edge_scrolls_the_sidebar_and_drops_on_what_scrolled_in() {
        let mut rt = rt("sidebar-pins");
        let top = |rt: &Runtime<State>| rt.presented("pl-p.0").unwrap().0[1];
        let before = top(&rt);
        drag(&mut rt, "t5-pl-5", (110.0, 805.0), false);
        let mut t = 1.0;
        for _ in 0..90 {
            t += 1.0 / 60.0;
            rt.update_if(|s, tx| crate::ui::poll(s, t, tx));
            rt.frame(1.0 / 60.0);
        }
        assert!(top(&rt) < before - 200.0, "scrolled {}", before - top(&rt));
        rt.pointer_up(110.0, 805.0);
        step(&mut rt);
        // Dropped on whichever editable row was under the pointer after scrolling.
        let log = read(&mut rt, |s| s.ui.dnd.log.clone());
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(log[0].0.starts_with("p.1") && log[0].0 != "p.1", "{log:?}");
    }
}
