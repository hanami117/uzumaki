//! One scrollable Up Next list shared by the sidebar and full-screen player.
//! Confirmed session History sits above Now Playing; initial placement hides it above the top.
//! After current come manually queued songs, the context and (repeat all) the next lap.
//! Stations show an explanation instead of upcoming tracks picked by Apple.
//! Virtualised, since the list context can be thousands of songs.

use rune::{Cursor, El, GrowthSpec, Live, ScrollPos, ScrollSpec, TextStyle, col, el, icon, row, text};

use std::borrow::Cow;

use super::{icons, up_next_anim};
use super::widgets::{Size, art};
use crate::models::Song;
use crate::state::State;
use crate::theme::*;

/// Where the list is drawn.
#[derive(Clone, Copy, PartialEq)]
pub enum Look {
    /// The right sidebar: opaque hover, and the playing song on top.
    Sidebar,
    /// Full-screen Now Playing: the same list over the cover's colours, with translucent hover.
    NowPlaying,
}

/// The section labels, by what follows them.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Label {
    Now,
    Next,
    Continue,
}

enum Item {
    Label(Label),
    Current,
    /// Index in `queue.up_next()`.
    Song(usize),
    /// The "Repeat" divider before the next lap.
    Repeat,
    /// Index in `queue.next_lap_songs()`.
    Lap(usize),
    /// The radio station's header: songs are picked as you listen.
    Station,
    /// Index in `queue.station_played()`.
    Played(usize),
    Gap,
}

/// What row `i` of the list is, given `u` user songs, `rest` context songs and `lap` songs of the
/// next lap (`current`: the list opens with the playing song). `station`: on a radio station,
/// with that many songs played so far.
fn item_at(i: usize, current: bool, u: usize, rest: usize, lap: usize, station: Option<usize>) -> Item {
    let mut k = i;
    if current {
        match k {
            0 => return Item::Label(Label::Now),
            1 => return Item::Current,
            _ => {}
        }
        k -= 2;
    }
    if u > 0 {
        if k == 0 {
            return Item::Label(Label::Next);
        }
        k -= 1;
        if k < u {
            return Item::Song(k);
        }
        k -= u;
    }
    if rest > 0 {
        if k == 0 {
            return Item::Label(Label::Continue);
        }
        k -= 1;
        if k < rest {
            return Item::Song(u + k);
        }
        k -= rest;
    }
    if lap > 0 {
        if k == 0 {
            return Item::Repeat;
        }
        k -= 1;
        if k < lap {
            return Item::Lap(k);
        }
        k -= lap;
    }
    if let Some(played) = station {
        if k == 0 {
            return Item::Station;
        }
        k -= 1;
        if k < played {
            return Item::Played(k);
        }
    }
    Item::Gap
}

/// Row height of the virtual list.
const ROW: f32 = 56.0;
/// How far (points) History must scroll into view before the pill has gone.
const HINT_FADE: f32 = 48.0;
/// The top/bottom fade while History (or anything) is hidden above; edges at rest stay hard.
const EDGE_FADE: f32 = 56.0;
/// Dimmest a History row gets, however old.
const HISTORY_MIN_OPACITY: f32 = 0.45;

/// Live values of the "↑ History · N" pill, set from the list's scroll offset, so scrolling
/// costs no view rebuild.
pub struct HistoryHint {
    opacity: Live<f32>,
    offset: Live<[f32; 2]>,
}

impl Default for HistoryHint {
    fn default() -> Self {
        Self { opacity: Live::new(0.0), offset: Live::new(HINT_AWAY) }
    }
}

/// Where the pill goes once History shows: out of reach of the pointer as well as the eye.
const HINT_AWAY: [f32; 2] = [0.0, -10_000.0];

impl HistoryHint {
    /// `hidden`: the height of History above Now Playing. The pill shows while all of it is
    /// above the viewport and fades (rising a little) over the first `HINT_FADE` points of it.
    fn follow(&self, at: ScrollPos, hidden: f32) {
        let shown = ((hidden - at.offset[1]) / HINT_FADE).clamp(0.0, 1.0);
        self.opacity.set(1.0 - shown);
        self.offset.set(if shown >= 1.0 { HINT_AWAY } else { [0.0, -6.0 * shown] });
    }
}

/// History row `d` places above Now Playing (1 = the song just before): older ones dimmer.
fn history_opacity(d: usize) -> f32 {
    (1.0 - 0.09 * d as f32).max(HISTORY_MIN_OPACITY)
}

/// The pill over the list's top edge: "↑ History · N", scrolls History into view.
fn history_pill(s: &State, look: Look, n: usize, target: f32) -> El {
    let (fill, hover) = if look == Look::Sidebar { (PAL.chip, PAL.chip_hover) } else { (crate::theme::ov(0.12), crate::theme::ov(0.2)) };
    let pill = row().key("un-history-pill").role("button").label(format!("Show History, {n} songs")).focusable(true)
        .h(26.0).px(11.0).gap(5.0).center_y().radius(13.0).fill(fill).hover(hover).cursor(Cursor::Pointer)
        .on_press_tx(move |_: &mut State, tx: &mut rune::Tx| tx.scroll_to("un-scroll", 0.0, target))
        .child(icon(&icons::ARROW_UP, 12.0).color(PAL.muted))
        .child(text(format!("History \u{00b7} {n}")).font(TextStyle::new(12.0, 500.0)).color(PAL.muted));
    let hint = &s.ui.up_next_hint;
    row().key("un-history-hint").pin(Some(0.0), Some(8.0), Some(0.0), None).justify_center()
        .live_opacity(&hint.opacity).live_offset(&hint.offset).child(pill)
}

fn label(t: String) -> El {
    row().w_full().px(20.0).pt(16.0).pb(6.0).child(text(t).font(SECTION11).color(PAL.dim).truncate().min_w(0.0))
}

/// The "Repeat" divider: a section label with the repeat icon.
fn repeat_label() -> El {
    row().w_full().px(20.0).pt(16.0).pb(6.0).gap(6.0).center_y()
        .child(icon(&icons::REPEAT, 13.0).color(PAL.dim))
        .child(text("Repeat").font(SECTION11).color(PAL.dim).truncate().min_w(0.0))
}

/// The radio station's header, in place of the Up Next the station doesn't have.
fn station_label(name: &str) -> El {
    row().key("un-station").w_full().px(20.0).pt(16.0).pb(6.0).gap(8.0)
        .child(icon(&icons::RADIO, 15.0).color(PAL.accent))
        .child(text(format!("{name} \u{2014} songs are picked as you listen")).font(SMALL13).color(PAL.muted).wrap().grow(1.0).min_w(0.0))
}

/// `lap`: a song of the next lap or of the station's history: dimmed, no remove button, and its
/// menu has no "Remove". Then `up_next_index` is the number `play_up_next` takes.
fn song_row(s: &State, song: &Song, key: String, up_next_index: Option<usize>, lap: bool, hover: rune::Color) -> El {
    let cur = key == "un-now";
    let (title_color, artist_color) = if cur { (PAL.accent, PAL.muted) } else if lap { (PAL.dim, PAL.dim) } else { (PAL.text, PAL.muted) };
    // The song tables' star: filled when favourite, outline on hover when it can be.
    let star = super::tracks::star_slot(s, song, &key);
    let mut r = row().key(key).role("button").label(song.title.clone()).w_full().min_h(52.0).px(12.0).gap(10.0).center_y()
        .radius(8.0).hover_group().cursor(Cursor::Pointer)
        .child(art(s, song.artwork.as_ref(), &song.album, Size::Small(40.0), 6.0, 0.0))
        .child(
            col().grow(1.0).min_w(0.0).gap(1.0)
                .child({
                    let title = text(song.title.clone()).font(TRACK_TITLE).color(title_color).truncate().min_w(0.0).shrink();
                    let mut r = row().w_full().min_w(0.0).gap(6.0).center_y().child(title);
                    if song.is_local() {
                        r = r.child(icon(&icons::LOCAL_MARK, 13.0).color(PAL.dim));
                    }
                    r
                })
                .child(text(song.artist.clone()).font(SMALL13).color(artist_color).truncate()),
        )
        .child(star);
    let menu_song = song.clone();
    let context = match up_next_index {
        Some(i) if !lap => super::menu::MenuContext::UpNext(i),
        _ => super::menu::MenuContext::NowPlaying,
    };
    r = r.on_secondary_press(move |s: &mut State, p, _tx: &mut rune::Tx| super::menu::open_song(s, menu_song.clone(), context, [p.x, p.y]));
    if lap {
        // Plays the song (and makes its lap the current one, or replays it from the station's
        // history); nothing to remove or move here.
        let i = up_next_index.unwrap_or(0);
        r = r.hover(hover).focusable(true).on_press_tx(move |s: &mut State, tx: &mut rune::Tx| {
            s.play_up_next(i);
            tx.instant();
        });
    } else if let Some(i) = up_next_index {
        r = r.hover(hover).focusable(true).on_press_tx(move |s: &mut State, tx: &mut rune::Tx| {
            s.play_up_next(i);
            tx.instant();
        }).child(
            el().key(format!("un-x-{i}")).role("button").label("Remove from Up Next").focusable(true).size(28.0, 28.0).radius(14.0).center()
                .hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer).group_opacity(0.0, 1.0)
                .on_press_tx(move |s: &mut State, tx: &mut rune::Tx| {
                    s.remove_up_next(i);
                    tx.instant();
                })
                .child(icon(&icons::CLOSE, 14.0).color(PAL.muted)),
        );
    }
    if lap || up_next_index.is_none() {
        // The remove button's slot, empty, so every row's star lines up.
        r = r.child(el().w(28.0));
    }
    r
}

/// The list's shape for one drawing: what row `i` is and its key.
struct Rows {
    current: bool,
    u: usize,
    rest: usize,
    lap: usize,
    station: Option<usize>,
    /// History rows (its label included) above Now Playing.
    prefix: usize,
    /// Indices in `s.history.tracks`, oldest first.
    history: Vec<usize>,
    /// Rows before the trailing blank ones.
    filled: usize,
    /// Rows in all, blank ones included.
    count: usize,
    /// A song id repeats among the queued songs / the list: their keys then carry the place.
    user_dups: bool,
    context_dups: bool,
}

thread_local! {
    /// Whether the context's ids repeat, for the (address, length) of the list it was found for.
    static CONTEXT_DUPS: std::cell::Cell<(usize, usize, bool)> = const { std::cell::Cell::new((0, 0, false)) };
}

fn has_dups<'a>(ids: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = std::collections::HashSet::new();
    ids.into_iter().any(|id| !seen.insert(id))
}

impl Rows {
    fn new(s: &State, viewport_h: f32) -> Self {
        let current = true;
        let u = s.queue.user.len();
        let rest = s.queue.up_next_len().saturating_sub(u);
        let lap = s.queue.next_lap_songs().len();
        let station = s.queue.station.as_ref().map(|_| 0);
        let history = s.history_indices();
        let h = history.len();
        let prefix = if h > 0 { h + 1 } else { 0 };
        let filled = prefix + if current { 2 } else { 0 }
            + if u > 0 { 1 + u } else { 0 }
            + if rest > 0 { 1 + rest } else { 0 }
            + if lap > 0 { 1 + lap } else { 0 }
            + station.map_or(0, |p| 1 + p);
        // A short station/empty upcoming list still needs enough trailing space to put current
        // at the top. Virtual blank rows cost no off-screen layout and keep row geometry exact.
        let count = (filled + 1).max(prefix + (viewport_h / ROW).ceil() as usize);
        let user_dups = u > 1 && has_dups(s.queue.user.iter().map(|x| x.id.as_str()));
        let ctx = &s.queue.context;
        let at = (ctx.as_ptr() as usize, ctx.len());
        let context_dups = CONTEXT_DUPS.with(|c| {
            let (p, n, dups) = c.get();
            if (p, n) == at {
                return dups;
            }
            let dups = has_dups(ctx.iter().map(|x| x.id.as_str()));
            c.set((at.0, at.1, dups));
            dups
        });
        Rows { current, u, rest, lap, station, prefix, history, filled, count, user_dups, context_dups }
    }

    fn item(&self, i: usize) -> Item {
        item_at(i - self.prefix, self.current, self.u, self.rest, self.lap, self.station)
    }

    /// Row `i`'s key. Stable for a song while it stays in its part of the list, wherever it
    /// moves in it, so `up_next_anim` can follow it (and the list keeps its measurements).
    fn key<'a>(&self, s: &'a State, i: usize) -> Cow<'a, str> {
        if i < self.prefix {
            return if i == 0 { "history-label".into() } else { format!("history-{}", s.history.tracks[self.history[i - 1]].0).into() };
        }
        if i >= self.filled {
            return format!("gap-{}", i - self.filled).into();
        }
        match self.item(i) {
            Item::Label(Label::Now) => "label-now".into(),
            Item::Label(Label::Next) => "label-next".into(),
            Item::Label(Label::Continue) => "label-continue".into(),
            Item::Current => "current".into(),
            Item::Song(j) if j < self.u => {
                let id = s.queue.user.get(j).map_or("", |x| x.id.as_str());
                if self.user_dups { format!("u{j}-{id}").into() } else { format!("u-{id}").into() }
            }
            Item::Song(j) => {
                let c = s.queue.pos + 1 + (j - self.u);
                let id = s.queue.context.get(c).map_or("", |x| x.id.as_str());
                if self.context_dups { format!("c{c}-{id}").into() } else { format!("c-{id}").into() }
            }
            Item::Repeat => "label-repeat".into(),
            Item::Lap(k) => format!("l{k}-{}", s.queue.next_lap_songs().get(k).map_or("", |x| x.id.as_str())).into(),
            Item::Station => "label-station".into(),
            Item::Played(k) => format!("p-{}", s.queue.station_played().get(k).map_or("", |x| x.id.as_str())).into(),
            Item::Gap => format!("gap-{}", i - self.filled).into(),
        }
    }

    /// Keys of the rows around `visible` (± `up_next_anim::MARGIN`), and the first one's index.
    fn window(&self, s: &State, visible: &std::ops::Range<usize>) -> (usize, Vec<String>) {
        let lo = visible.start.saturating_sub(up_next_anim::MARGIN).min(self.count);
        let hi = (visible.end.max(visible.start + 1) + up_next_anim::MARGIN).min(self.count);
        (lo, (lo..hi).map(|i| self.key(s, i).into_owned()).collect())
    }
}

/// The scrolling list; the right sidebar wraps it in the queue drop target.
pub fn list(s: &State, look: Look, cx: &rune::Cx<State>) -> El {
    let hover = if look == Look::Sidebar { PAL.row_hover } else { crate::theme::ov(0.08) };
    let viewport_h = cx.layout_size("un-scroll").unwrap_or([340.0, 860.0])[1];
    let rows = std::rc::Rc::new(Rows::new(s, viewport_h));
    let (prefix, count, h) = (rows.prefix, rows.count, rows.history.len());
    let history_version: Vec<u64> = rows.history.iter().map(|&i| s.history.tracks[i].0).collect();
    let hidden = prefix as f32 * ROW;

    // Rows that moved since the last drawing slide from their old places (`up_next_anim.rs`).
    let anim = &s.ui.up_next_anim;
    anim.viewport_h.set(viewport_h);
    let (start, keys) = rows.window(s, &anim.visible);
    anim.observe(start, keys, ROW);

    let user_ids = {
        use std::hash::{Hash, Hasher};
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        for x in &s.queue.user { x.id.hash(&mut hs); }
        hs.finish()
    };
    let version = (history_version, count, rows.u, user_ids, s.queue.pos, s.queue.lap_gen, rows.station.is_some(), rows.context_dups, s.queue.source.clone(), s.playback.song.as_ref().map(|x| x.id.clone()), s.favourites_rev());
    let key_rows = rows.clone();
    let list = col().key("un-rows").growth(GrowthSpec::default()).instant_changes().virtual_rows(
        count,
        ROW,
        move |s: &State, i| key_rows.key(s, i),
        move |s: &State, i: usize| {
            // Rows are built when the list lays out, after `dark_scope` has ended, so Now Playing's own
            // rows ask for the dark palette again (Now Playing is a dark stage in both themes).
            let build = || {
            let content = if i < rows.prefix {
                if i == 0 { row().px(20.0).gap(6.0).center_y().child(icon(&icons::CLOCK, 13.0).color(PAL.dim)).child(text("History").font(SECTION11).color(PAL.dim)) }
                else {
                    let k = rows.history[i-1];
                    let song = &s.history.tracks[k].1;
                    // Fades with distance from Now Playing; hovering shows it whole.
                    let row_el = song_row(s, song, format!("history-song-{}", s.history.tracks[k].0), None, false, hover)
                        .group_opacity(history_opacity(rows.prefix - i), 1.0)
                        .on_press_tx(move |s: &mut State, tx: &mut rune::Tx| { s.play_history(k); tx.instant(); });
                    row().key(format!("history-wrap-{}", s.history.tracks[k].0)).w_full().px(8.0).hover_group().child(row_el)
                }
            } else if i >= rows.filled {
                col().h(24.0)
            } else { match rows.item(i) {
                Item::Label(Label::Now) => label("Now Playing".into()),
                Item::Label(Label::Next) => label("Playing Next".into()),
                Item::Label(Label::Continue) => label(if s.queue.source.is_empty() { "Up Next".to_string() } else { format!("Continue playing from {}", s.queue.source) }),
                // The song playing (a Play Next song isn't `queue.current()`, the list's place).
                Item::Current => match s.playback.song.as_ref() {
                    Some(song) => row().w_full().px(8.0).py(1.0).child(song_row(s, song, "un-now".into(), None, false, hover)),
                    None => col(),
                },
                Item::Song(j) => match s.queue.up_next().nth(j) {
                    Some(song) => row().w_full().px(8.0).py(1.0).child(song_row(s, song, format!("un{j}-{}", song.id), Some(j), false, hover)),
                    None => col().h(1.0),
                },
                Item::Repeat => repeat_label(),
                Item::Lap(k) => match s.queue.next_lap_songs().get(k) {
                    Some(song) => row().w_full().px(8.0).py(1.0).child(song_row(s, song, format!("unl{k}-{}", song.id), Some(s.queue.up_next_len() + k), true, hover)),
                    None => col().h(1.0),
                },
                Item::Station => station_label(&s.queue.station.as_ref().map_or(String::new(), |r| r.name.clone())),
                Item::Played(k) => match s.queue.station_played().get(k) {
                    Some(song) => {
                        let n = s.queue.up_next_len() + s.queue.next_lap_songs().len() + k;
                        row().w_full().px(8.0).py(1.0).child(song_row(s, song, format!("unp{k}-{}", song.id), Some(n), true, hover))
                    }
                    None => col().h(1.0),
                },
                Item::Gap => col().h(24.0),
            }};
            let mut r = col().h(ROW).min_h(ROW).w_full().justify_center().child(content);
            let anim = &s.ui.up_next_anim;
            if !anim.idle() && let Some(m) = anim.row(&rows.key(s, i)) {
                if let Some(o) = &m.offset { r = r.live_offset(o.smooth(up_next_anim::MOTION)); }
                if let Some(f) = &m.fade { r = r.live_opacity(f.smooth(up_next_anim::MOTION)); }
            }
            r
            };
            if look == Look::NowPlaying { crate::theme::dark_scope(build) } else { build() }
        },
    )
    .virtual_rows_version(version)
    .on_visible_rows(|s: &mut State, r| {
        // Scrolling: compare the next edit with the rows on screen now.
        s.ui.up_next_anim.visible = r.clone();
        let rows = Rows::new(s, s.ui.up_next_anim.viewport_h.get());
        let (start, keys) = rows.window(s, &r);
        s.ui.up_next_anim.remember(start, keys);
        false
    });
    let scroll = col().key("un-scroll").grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false });
    // A fade only shows on an edge with something hidden under it, so the wide one is the
    // "there's more above" cue once History exists.
    let fade = if h > 0 { EDGE_FADE } else { 12.0 };
    let mut scroll = scroll.scroll_anchor().scroll_edge_fade(fade).on_layout_tx(move |s: &mut State, _, tx: &mut rune::Tx| {
        if !s.ui.up_next_placed {
            s.ui.up_next_placed = true;
            tx.scroll_to_instant("un-scroll", 0.0, hidden);
        }
    });
    let wrap = col().key("un-wrap").grow(1.0).min_h(0.0).w_full();
    if h == 0 {
        return wrap.child(scroll.child(list));
    }
    let lives = HistoryHint { opacity: s.ui.up_next_hint.opacity.clone(), offset: s.ui.up_next_hint.offset.clone() };
    scroll = scroll.on_scroll_offset(move |at| lives.follow(at, hidden));
    // The pill reveals the most recent History, leaving Now Playing and its label in view.
    let target = (hidden - (viewport_h - 2.0 * ROW - 16.0)).max(0.0);
    wrap.child(scroll.child(list)).child(history_pill(s, look, h, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(n: usize, current: bool, u: usize, rest: usize, lap: usize, station: Option<usize>) -> String {
        (0..n)
            .map(|i| match item_at(i, current, u, rest, lap, station) {
                Item::Label(_) => 'L',
                Item::Current => 'C',
                Item::Song(_) => 's',
                Item::Repeat => 'R',
                Item::Lap(_) => 'l',
                Item::Station => 'S',
                Item::Played(_) => 'p',
                Item::Gap => '.',
            })
            .collect()
    }

    #[test]
    fn the_repeat_divider_and_the_next_lap_follow_the_rest_of_this_lap() {
        // Sidebar: Now Playing label + song, 1 queued, 2 left of this lap, 3 in the next one.
        assert_eq!(kinds(14, true, 1, 2, 3, None), "LCLsLssRlll...");
        // Last song of the lap: nothing left, the divider comes straight after the playing song.
        assert_eq!(kinds(8, false, 0, 0, 3, None), "Rlll....");
        // Repeat off: no divider.
        assert_eq!(kinds(5, false, 0, 2, 0, None), "Lss..");
    }

    #[test]
    fn a_station_header_and_its_played_songs_follow_what_was_queued_by_hand() {
        // Now Playing + song, 2 queued by hand, then the header and 3 played songs.
        assert_eq!(kinds(11, true, 2, 0, 0, Some(3)), "LCLssSppp..");
        // Nothing played yet: the header alone. Nothing queued: it comes right after the playing song.
        assert_eq!(kinds(5, true, 0, 0, 0, Some(0)), "LCS..");
        assert_eq!(kinds(6, false, 0, 0, 0, Some(2)), "Spp...");
    }
}

#[cfg(test)]
mod history_view_tests {
    use super::*;
    use rune::{Cx, Env, Runtime};
    fn view(s: &State, cx: &mut Cx<State>) -> El {
        col().h_full().w_full().child(list(s, Look::Sidebar, cx))
    }
    #[test]
    fn a_short_station_also_opens_with_history_above_the_viewport() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n,_)| *n == "now-playing-queue-station").unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, view, (340.0, 900.0), Env { system_dark: true, scale: 1.0 });
        for _ in 0..5 { rt.frame(1.0/60.0); }
        let y = rt.presented("un-now").unwrap().0[1];
        assert!(y < 90.0, "short station cannot expose history initially: {y}");
    }
    #[test]
    fn the_history_pill_shows_while_history_is_hidden_and_leaves_once_it_is_revealed() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n,_)| *n == "up-next-long-history").unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, view, (340.0, 600.0), Env { system_dark: true, scale: 1.0 });
        for _ in 0..5 { rt.frame(1.0/60.0); }
        let y = rt.presented("un-history-pill").unwrap().0[1];
        assert!((0.0..40.0).contains(&y), "pill at the top while History is hidden: {y}");
        rt.update(|_, tx| tx.scroll_to_instant("un-scroll", 0.0, 0.0));
        for _ in 0..3 { rt.frame(1.0/60.0); }
        let y = rt.presented("un-history-pill").map_or(-1e9, |p| p.0[1]);
        assert!(y < -1000.0, "pill out of reach once History shows: {y}");
        // Older History rows are dimmer, down to the floor.
        assert!(history_opacity(1) > history_opacity(4) && history_opacity(40) == HISTORY_MIN_OPACITY);
    }
    #[test]
    fn a_star_on_an_up_next_row_toggles_the_favourite_and_nothing_else() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n,_)| *n == "up-next-stars").unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, view, (340.0, 900.0), Env { system_dark: true, scale: 1.0 });
        for _ in 0..5 { rt.frame(1.0/60.0); }
        let up_next = rt.state.queue.up_next_len();
        let playing = rt.state.playback.song.as_ref().map(|x| x.id.clone());
        let star = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect);
        // Starred in the scene: filled, and pressing it un-stars only that song.
        let user1 = rt.state.queue.user[1].clone();
        assert!(rt.state.is_favourite(&user1));
        let r = star(&rt, "un1-user-1-star").expect("the starred row shows its star");
        rt.pointer_down(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_up(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        for _ in 0..3 { rt.frame(1.0/60.0); }
        assert!(!rt.state.is_favourite(&user1), "un-starred");
        assert_eq!(rt.state.queue.up_next_len(), up_next, "the row stays queued");
        assert_eq!(rt.state.playback.song.as_ref().map(|x| x.id.clone()), playing, "and doesn't play");
        // The list rebuilt: the same star now stars it again.
        let r = star(&rt, "un1-user-1-star").expect("outline star, on hover");
        rt.pointer_down(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_up(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        for _ in 0..3 { rt.frame(1.0/60.0); }
        assert!(rt.state.is_favourite(&user1), "starred again");
    }
    #[test]
    fn no_history_no_pill() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n,_)| *n == "up-next-no-history").unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, view, (340.0, 600.0), Env { system_dark: true, scale: 1.0 });
        for _ in 0..4 { rt.frame(1.0/60.0); }
        assert!(rt.presented("un-history-pill").is_none());
        assert!(rt.presented("un-now").unwrap().0[1] < 60.0);
    }
    #[test]
    fn opens_at_current_and_keeps_history_position_when_another_track_is_confirmed() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n,_)| *n == "right-panel-queue").unwrap().1)(&mut s);
        s.history = Default::default();
        let mut song = s.queue.context[0].clone();
        for i in 0..55 {
            song.id = format!("history-test-{i}"); song.catalog_id = None;
            s.history.heard(&song);
        }
        let mut rt = Runtime::new(s, view, (340.0, 400.0), Env { system_dark: true, scale: 1.0 });
        for _ in 0..4 { rt.frame(1.0/60.0); }
        let current_y = rt.presented("un-now").unwrap().0[1];
        assert!(current_y < 90.0, "current starts near top: {current_y}");
        rt.update(|_, tx| tx.scroll_to_instant("un-scroll", 0.0, 560.0));
        for _ in 0..3 { rt.frame(1.0/60.0); }
        let key = "history-song-15";
        let before = rt.presented(key).unwrap().0[1];
        rt.update(|s, tx| {
            let mut song = s.queue.context[0].clone(); song.id = "new-confirmed".into(); song.catalog_id = None;
            s.history.heard(&song); tx.instant();
        });
        for _ in 0..3 { rt.frame(1.0/60.0); }
        let after = rt.presented(key).unwrap().0[1];
        assert!((before-after).abs() < 1.0, "history moved: {before} -> {after}");
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        col().h_full().w_full().child(list(s, Look::Sidebar, cx))
    }
    fn open(scene: &str) -> Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, view, (340.0, 700.0), Env { system_dark: true, scale: 1.0 });
        for _ in 0..6 { rt.frame(1.0 / 60.0); }
        rt
    }
    fn y(rt: &Runtime<State>, key: &str) -> f32 {
        rt.presented(key).unwrap_or_else(|| panic!("{key} not drawn")).0[1]
    }
    fn opacity(rt: &Runtime<State>, key: &str) -> f32 {
        rt.presented(key).unwrap().1
    }
    /// One frame, as the app runs them: `ui::poll` first (`t`: the test's own clock).
    fn frame(rt: &mut Runtime<State>, dt: f64) {
        let t = CLOCK.with(|c| { c.set(c.get() + dt); c.get() });
        rt.update_if(|s, tx| { s.ui.up_next_anim.poll(t, tx); false });
        rt.frame(dt);
    }
    thread_local! { static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) }; }
    fn planned(rt: &Runtime<State>) -> u64 {
        rt.state.ui.up_next_anim.planned()
    }

    #[test]
    fn play_next_opens_a_row_and_slides_the_rows_below_down() {
        let mut rt = open("up-next");
        let before = y(&rt, "u-user-0");
        let below = y(&rt, "u-user-1");
        rt.update(|s, tx| {
            let mut song = s.queue.context[7].clone();
            song.id = "fresh".into();
            s.play_next(&[song]);
            tx.instant();
        });
        rt.frame(1.0 / 60.0);
        // The commit landed the layout, but the rows are still drawn where they were.
        assert!((y(&rt, "u-user-0") - before).abs() < 0.5, "drawn at its old place first");
        assert!(opacity(&rt, "u-fresh") < 0.05, "the new row starts invisible");
        for _ in 0..6 { frame(&mut rt, 1.0 / 60.0); }
        let mid = y(&rt, "u-user-0");
        assert!(mid > before + 5.0 && mid < before + ROW - 2.0, "slides down: {before} -> {mid}");
        assert!(opacity(&rt, "u-fresh") < 0.2, "the gap opens before the row shows");
        for _ in 0..4 { frame(&mut rt, 1.0 / 60.0); }
        let o = opacity(&rt, "u-fresh");
        assert!(o > 0.2 && o < 0.99, "fades in: {o}");
        for _ in 0..40 { frame(&mut rt, 1.0 / 60.0); }
        assert!((y(&rt, "u-user-0") - (before + ROW)).abs() < 0.5);
        assert!((y(&rt, "u-user-1") - (below + ROW)).abs() < 0.5);
        assert!((y(&rt, "u-fresh") - before).abs() < 0.5 && opacity(&rt, "u-fresh") > 0.99);
        // The new row never moved: it fades in where it belongs.
        assert!(rt.state.ui.up_next_anim.row("u-fresh").is_none_or(|m| m.offset.is_none()));
        assert_eq!(planned(&rt), 1);
    }

    #[test]
    fn idle_and_unrelated_commits_start_nothing() {
        let mut rt = open("up-next");
        assert_eq!(planned(&rt), 0, "opening never animates");
        // A commit that re-runs the view without touching the queue (a clock tick, say).
        rt.update(|s, _| s.toast = Some("hi".into()));
        for _ in 0..3 { frame(&mut rt, 1.0 / 60.0); }
        assert_eq!(planned(&rt), 0);
        assert!(!rt.state.ui.up_next_anim.pending());
        assert!(rt.state.ui.up_next_anim.idle());
    }

    #[test]
    fn removing_a_row_slides_the_rows_below_up() {
        let mut rt = open("up-next");
        let gone = y(&rt, "u-user-1");
        let below = y(&rt, "u-user-2");
        rt.update(|s, tx| { s.remove_up_next(1); tx.instant(); });
        rt.frame(1.0 / 60.0);
        assert!(rt.presented("u-user-1").is_none(), "gone at once");
        assert!((y(&rt, "u-user-2") - below).abs() < 0.5);
        for _ in 0..6 { frame(&mut rt, 1.0 / 60.0); }
        let mid = y(&rt, "u-user-2");
        assert!(mid < below - 5.0 && mid > gone + 2.0, "slides up: {mid}");
        for _ in 0..40 { frame(&mut rt, 1.0 / 60.0); }
        assert!((y(&rt, "u-user-2") - gone).abs() < 0.5);
    }

    #[test]
    fn the_next_track_slides_the_list_up() {
        let mut rt = open("up-next");
        let now = y(&rt, "un-now");
        let next = y(&rt, "u-user-1");
        rt.update(|s, tx| {
            let song = s.queue.advance(s.playback.repeat).unwrap();
            s.history.heard(&song);
            s.playback.song = Some(song);
            tx.instant();
        });
        rt.frame(1.0 / 60.0);
        assert!((y(&rt, "un-now") - now).abs() < 0.5, "Now Playing keeps its place");
        assert!((y(&rt, "u-user-1") - next).abs() < 0.5, "drawn where it was");
        for _ in 0..40 { frame(&mut rt, 1.0 / 60.0); }
        assert!((y(&rt, "u-user-1") - (next - ROW)).abs() < 0.5, "one row up");
        assert_eq!(planned(&rt), 1);
    }

    #[test]
    fn a_new_queue_just_shows() {
        let mut rt = open("up-next");
        rt.update(|s, tx| {
            let list: Vec<Song> = (0..20).map(|i| { let mut t = s.queue.context[0].clone(); t.id = format!("new-{i}"); t }).collect();
            s.queue = crate::state::PlayQueue::start(&list, 0, false, "Elsewhere", Default::default());
            s.playback.song = s.queue.current().cloned();
            tx.instant();
        });
        rt.frame(1.0 / 60.0);
        assert_eq!(planned(&rt), 0);
        let at = y(&rt, "c-new-1");
        frame(&mut rt, 1.0 / 60.0);
        assert!((y(&rt, "c-new-1") - at).abs() < 0.01, "lands at once");
    }
}
