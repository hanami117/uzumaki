//! The right sidebar (Ctrl+Shift+B), like Apple Music for Mac's: synced lyrics of the current
//! song, or Up Next, behind a two-way switch. Wide windows put it beside the page; narrow ones
//! lay it over the page from the right.
//!
//! The lyrics follow the song with a small glide of their own (`now_playing/follow.rs` holds
//! global state for the full-screen view, so this keeps a separate, much simpler copy).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use rune::{Cursor, Cx, Edge, El, ScrollSpec, Spring, SpringPair, TextStyle, Transition, Tx, col, el, icon, row, text};

use super::icons;
use super::up_next;
use crate::lyrics::Lyrics;
use crate::settings::RightTab;
use crate::state::{Load, State};
use crate::theme::*;

pub const W: f32 = 340.0;
/// Narrower windows show the panel over the page instead of squeezing it.
pub const OVERLAY_BELOW: f32 = 1000.0;
/// Where the current line sits in the lyrics viewport, as a fraction from the top.
const FOCUS: f32 = 0.33;
const GLIDE: Spring = Spring::new(0.75, 0.0);
const FADE: Spring = Spring::new(0.35, 0.0);
/// At most this many frames in a row a line change waits for a frame that animates.
const HOLD_MAX: u32 = 6;
/// Scrolling by hand pauses auto-follow this long.
const PAUSE: Duration = Duration::from_millis(3000);
const LINE: TextStyle = TextStyle::new(19.0, 600.0).lh(26.0);
const PLAIN: TextStyle = TextStyle::new(16.0, 500.0).lh(24.0);

#[derive(Default)]
struct Follow {
    /// (top of line relative to the viewport's top, in layout space) for lines around the current.
    tops: Vec<(usize, f32)>,
    vp_h: f32,
    manual: Option<Instant>,
    ignore_until: Option<Instant>,
    /// A glide to the current line is owed (a click, or manual scrolling ended).
    resume: bool,
    /// The next placement is an instant jump (panel opened, new song).
    snap: bool,
    /// Frames a line change has waited for a commit that isn't instant (see `poll`).
    held: u32,
    tries: u32,
    /// Song the placement state belongs to.
    song: String,
}

thread_local! {
    static FOLLOW: RefCell<Follow> = RefCell::new(Follow { snap: true, ..Follow::default() });
}

/// Panel state kept in `UiState`.
#[derive(Default)]
pub struct PanelState {
    /// (song id, current line) the last poll saw while the lyrics were showing.
    seen: Option<(String, Option<usize>)>,
}

// ---- what shows ----

/// Whether the panel is on screen. A narrow window has no room for it: a panel left open hides
/// there (the setting stays, so it comes back when the window is wide again, as the sidebar does).
pub fn visible(s: &State) -> bool {
    s.settings.right_panel && !s.ui.now_playing && !super::narrow::narrow(s)
}

enum Body<'a> {
    Message(&'static str),
    Synced(&'a Lyrics),
    Plain(&'a Lyrics),
}

fn body_of<'a>(s: &'a State) -> Body<'a> {
    let Some(song) = s.playback.song.as_ref() else { return Body::Message("Nothing is playing") };
    if song.is_local() {
        return Body::Message("Lyrics aren't available for your own files");
    }
    match s.lyrics_of(song) {
        Some(Load::Idle) | Some(Load::Loading) => Body::Message("Loading lyrics\u{2026}"),
        Some(Load::Ready(Some(l))) if !l.lines.is_empty() => {
            if l.synced { Body::Synced(l) } else { Body::Plain(l) }
        }
        Some(Load::Failed(_)) => Body::Message("Couldn\u{2019}t load lyrics"),
        _ => Body::Message("No lyrics for this song"),
    }
}

fn synced_lyrics(s: &State) -> Option<&Lyrics> {
    match body_of(s) {
        Body::Synced(l) => Some(l),
        _ => None,
    }
}

// ---- poll ----

/// Runs before every frame (from `ui::poll`). Sets the panel's fine-time request and glides the
/// lyrics to the current line; returns true only when the current line changed (or a scroll was
/// issued), so an idle or paused window draws nothing.
///
/// `landed`: this frame's commit is instant (`State::landing`); a line change found then waits
/// for the next frame (at most `HOLD_MAX`), or its glide and fade would land as a jump.
pub fn poll(s: &mut State, landed: bool, tx: &mut Tx) -> bool {
    let showing = visible(s) && s.settings.right_tab == RightTab::Lyrics;
    let t = s.playback.position_now();
    let cur = if showing { synced_lyrics(s).map(|l| l.line_at(t)) } else { None };
    let playing = matches!(s.playback.state, crate::bridge::PlayState::Playing);
    s.set_panel_fine_time(cur.is_some() && playing);
    let Some(cur) = cur else {
        if s.ui.right.seen.take().is_some() {
            FOLLOW.with_borrow_mut(|f| f.snap = true);
        }
        return false;
    };

    let song_id = s.playback.song.as_ref().map(|x| x.id.clone()).unwrap_or_default();
    let seen = Some((song_id.clone(), cur));
    let changed = s.ui.right.seen != seen;
    let same_song = s.ui.right.seen.as_ref().is_some_and(|(id, _)| *id == song_id);
    let hold = FOLLOW.with_borrow_mut(|f| {
        let hold = changed && landed && same_song && !f.snap && f.held < HOLD_MAX;
        f.held = if hold { f.held + 1 } else { 0 };
        hold
    });
    if hold {
        tx.wake_after(Duration::ZERO);
        return false;
    }
    s.ui.right.seen = seen;

    let mut dirty = changed;
    FOLLOW.with_borrow_mut(|f| {
        if f.song != song_id {
            f.song = song_id;
            f.snap = true;
            f.manual = None;
            f.tries = 0;
        }
        match f.manual.and_then(|m| PAUSE.checked_sub(m.elapsed())) {
            Some(left) => {
                tx.wake_after(left + Duration::from_millis(20));
                return;
            }
            None => {
                if f.manual.take().is_some() {
                    f.resume = true;
                }
            }
        }
        if !(changed || f.resume || f.snap) {
            return;
        }
        let want = cur.unwrap_or(0);
        let top = if cur.is_none() { Some(0.0) } else { f.tops.iter().find(|(k, _)| *k == want).map(|(_, y)| *y) };
        match top {
            Some(y) if f.vp_h > 0.0 || cur.is_none() => {
                let y = (y - FOCUS * f.vp_h).max(0.0);
                if f.snap {
                    f.ignore_until = Some(Instant::now() + Duration::from_millis(400));
                    tx.scroll_to_instant("rp-lyrics", 0.0, y);
                } else {
                    f.ignore_until = Some(Instant::now() + Duration::from_millis(150));
                    tx.scroll_to_with("rp-lyrics", 0.0, y, GLIDE);
                }
                f.snap = false;
                f.resume = false;
                f.tries = 0;
                dirty = true;
            }
            _ => {
                // Layout isn't known yet (or the line isn't laid out): look again shortly.
                f.resume = true;
                f.tries += 1;
                if f.tries < 60 {
                    tx.wake_after(Duration::from_millis(30));
                    dirty = true;
                }
            }
        }
    });
    if playing {
        // The next line change is found by the next poll; ask for it just after the clock ticks.
        tx.wake_after(Duration::from_millis(100));
    }
    dirty
}

fn mark_manual() {
    FOLLOW.with_borrow_mut(|f| {
        if f.ignore_until.is_some_and(|u| Instant::now() < u) {
            return;
        }
        f.manual = Some(Instant::now());
        f.resume = true;
    });
}

// ---- view ----

fn segment(key: &'static str, label: &'static str, on: bool) -> El {
    let (bg, fg) = if on { (crate::theme::seg_on(), PAL.text) } else { (rune::Color::TRANSPARENT, PAL.muted) };
    let e = el().key(key).role("button").label(label).focusable(true).h(28.0).grow(1.0).radius(8.0).center().fill(bg)
        .cursor(Cursor::Pointer);
    let e = if on { e } else { e.hover(crate::theme::ov(0.05)) };
    e.child(text(label).font(TextStyle::new(13.0, 500.0)).color(fg))
}

/// The switch's segments: a tab other than the shown one (never closes the panel).
fn pick(s: &mut State, tab: RightTab) {
    if s.settings.right_tab != tab {
        s.show_right_tab(tab);
    }
}

fn header(s: &State) -> El {
    let tab = s.settings.right_tab;
    let switch = row().key("rp-switch").grow(1.0).padding(3.0).gap(2.0).radius(11.0).fill(PAL.chip)
        .child(segment("rp-tab-lyrics", "Lyrics", tab == RightTab::Lyrics)
            .on_press(|s: &mut State| pick(s, RightTab::Lyrics)))
        .child(segment("rp-tab-queue", "Up Next", tab == RightTab::Queue)
            .on_press(|s: &mut State| pick(s, RightTab::Queue)));
    row().w_full().px(14.0).h(56.0).gap(8.0).center_y().child(switch).child(
        el().key("rp-close").role("button").label("Close sidebar").focusable(true).size(32.0, 32.0).radius(16.0).center()
            .hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
            .on_press(|s: &mut State| s.toggle_right_panel())
            .child(icon(&icons::CLOSE, 16.0).color(PAL.muted)),
    )
}

fn message(msg: &'static str) -> El {
    col().key("rp-lyrics").grow(1.0).min_h(0.0).w_full().px(24.0).center()
        .child(text(msg).font(TextStyle::new(15.0, 500.0)).color(PAL.dim).wrap().align(rune::Align::Center))
}

fn plain(l: &Lyrics) -> El {
    let mut body = col().w_full().gap(12.0).px(20.0).pt(8.0).pb(40.0)
        .child(text("Lyrics are not time-synced").font(SECTION11).color(PAL.dim));
    for line in &l.lines {
        body = body.child(text(line.text.clone()).font(PLAIN).color(PAL.text2.alpha(0.9)).wrap());
    }
    col().key("rp-lyrics").grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(32.0).child(body)
}

fn synced(s: &State, l: &Lyrics, cx: &Cx<State>) -> El {
    let t = s.playback.position_now();
    // What `poll` shows (it may hold a line change back a frame), else the clock's.
    let song_id = s.playback.song.as_ref().map(|x| x.id.as_str());
    let cur = match &s.ui.right.seen {
        Some((id, cur)) if Some(id.as_str()) == song_id => *cur,
        _ => l.line_at(t),
    };
    let view = cx.layout_size("rp-lyrics").unwrap_or([W, 560.0]);

    // Where lines sit (layout space), so `poll` can glide to the current one.
    if let Some(vp) = cx.layout("rp-lyrics") {
        let from = cur.unwrap_or(0).saturating_sub(1);
        let tops = (from..=from + 6)
            .filter_map(|k| cx.layout(&format!("rp-ly-{k}")).map(|r| (k, r[1] - vp[1])))
            .collect();
        FOLLOW.with_borrow_mut(|f| {
            f.tops = tops;
            f.vp_h = vp[3];
        });
    }

    let mut content = col().key("rp-content").w_full().px(10.0).pt(view[1] * FOCUS).pb(view[1] * 0.6);
    for (i, line) in l.lines.iter().enumerate() {
        let active = cur == Some(i);
        let start = line.start;
        content = content.child(
            col().key(format!("rp-ly-{i}")).w_full().px(10.0).py(7.0).radius(10.0).role("button").label(line.text.clone())
                .hover(crate::theme::ov(0.06)).cursor(Cursor::Pointer)
                .opacity(if active { 1.0 } else { 0.38 }).opacity_motion(FADE)
                .on_press(move |s: &mut State| {
                    FOLLOW.with_borrow_mut(|f| {
                        f.manual = None;
                        f.resume = true;
                    });
                    s.seek_seconds(start);
                })
                .child(text(line.text.clone()).font(LINE).color(PAL.text).wrap()),
        );
    }
    col().key("rp-lyrics").grow(1.0).min_h(0.0).w_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(40.0)
        .motion(GLIDE)
        .on_scroll_offset(|_at: rune::ScrollPos| {
            if rune::scroll_watch_state().0 {
                mark_manual();
            }
        })
        .child(content)
}

fn lyrics_tab(s: &State, cx: &Cx<State>) -> El {
    match body_of(s) {
        Body::Message(m) => message(m),
        Body::Plain(l) => plain(l),
        Body::Synced(l) => synced(s, l, cx),
    }
}

fn queue_tab(s: &State, cx: &Cx<State>) -> El {
    // Songs dropped anywhere on the list are queued.
    let body = if s.playback.song.is_none() {
        col().key("up-next").grow(1.0).min_h(0.0).w_full().child(message("Nothing is playing"))
    } else {
        col().key("up-next").grow(1.0).min_h(0.0).w_full().child(up_next::list(s, up_next::Look::Sidebar, cx))
    };
    super::dnd::queue_target(s, body, |e| e.border(2.0, PAL.accent))
}

/// The panel. `over` lays it over the page (pinned to the right, above the player bar).
pub fn panel(s: &State, cx: &Cx<State>, over: bool) -> El {
    let tab = match s.settings.right_tab {
        RightTab::Lyrics => lyrics_tab(s, cx),
        RightTab::Queue => queue_tab(s, cx),
    };
    let inner = col().grow(1.0).min_w(0.0).h_full().child(header(s)).child(tab);
    let e = row().key("right-panel").w(W).h_full().fill(PAL.sidebar)
        .transition(Transition::move_edge(Edge::Trailing).springs(SpringPair::new(Spring::new(0.5, 0.0), Spring::new(0.4, 0.0))))
        // Presses on the panel's empty space don't reach the page below it.
        .on_press(|_s: &mut State| {})
        .child(el().w(1.0).h_full().fill(PAL.divider))
        .child(inner);
    if over {
        let bottom = match (s.playback.song.is_some(), s.settings.player_style) {
            (false, _) => 0.0,
            (true, crate::settings::PlayerStyle::Docked) => PLAYER_H,
            (true, crate::settings::PlayerStyle::Floating) => super::player_bar::FLOAT_STRIP_H,
        };
        e.shadow(0.0, 30.0, crate::theme::shadow(0.45)).pin(None, Some(0.0), Some(0.0), Some(bottom))
    } else {
        e
    }
}
