//! The lyrics column: synced (depth, glide, word fill, interludes) and plain text.

use rune::{Align, Cursor, Cx, El, Live, ScrollPos, ScrollSpec, TextStyle, Tx, col, el, memo, row, text};

use super::follow::{self, FOCUS, GLIDE};
use super::{lyrics_load, seek_to_line};
use crate::lyrics::{Line, Lyrics};
use crate::models::Song;
use crate::state::{Load, State};
use crate::theme::*;

const LINE: TextStyle = TextStyle::new(32.0, 600.0).lh(40.0);
const PLAIN: TextStyle = TextStyle::new(20.0, 500.0).lh(30.0);
const DOTS_H: f32 = 48.0;

fn frame(narrow: bool) -> El {
    let c = col().key("np-lyrics");
    if narrow { c.w_full().grow(1.0).min_h(0.0) } else { c.grow(1.0).max_w(640.0).h_full() }
}

fn message(narrow: bool, msg: &str) -> El {
    frame(narrow).justify_center().child(text(msg.to_string()).font(TextStyle::new(20.0, 500.0)).color(PAL.lyric_next))
}

pub fn panel(s: &State, song: &Song, cx: &Cx<State>, narrow: bool) -> El {
    let view = cx.layout_size("np-lyrics").unwrap_or([480.0, 560.0]);
    match lyrics_load(s, song) {
        Some(Load::Ready(Some(l))) if !l.lines.is_empty() => {
            if l.synced { synced(s, song, l, cx, view, narrow) } else { plain(l, view, narrow) }
        }
        Some(Load::Idle) | Some(Load::Loading) | None => message(narrow, "Loading lyrics\u{2026}"),
        _ => message(narrow, "No lyrics for this song"),
    }
}

/// Unsynced lyrics: deliberately unlike the synced look: smaller, one even colour, no depth,
/// scrollable, with a small "not synced" label.
fn plain(l: &Lyrics, view: [f32; 2], narrow: bool) -> El {
    let mut body = col().w_full().gap(14.0).px(14.0).pt(24.0).pb((view[1] * 0.4).max(80.0))
        .child(text("Lyrics are not time-synced").font(SECTION11).color(PAL.dim));
    for line in &l.lines {
        body = body.child(text(line.text.clone()).font(PLAIN).color(PAL.text2.alpha(0.85)).wrap());
    }
    frame(narrow).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(40.0).child(body)
}

/// Alpha of the unlit base text of a word-timed line (the fill overlay is full `PAL.text`).
const BASE_DIM: f32 = 0.42;

/// (brightness, blur sigma, scale) for a line `d` away from the current one. Brightness is the
/// line's overall look; `line_el` turns it into the container opacity.
fn style(d: isize, active: bool) -> (f32, f32, f32) {
    if active {
        return (1.0, 0.0, 1.0);
    }
    let ad = d.unsigned_abs().max(1) as f32;
    let mut alpha = (0.36 - 0.045 * (ad - 1.0)).max(0.13);
    if d < 0 {
        alpha *= 0.85;
    }
    let blur = if ad > 12.0 { 0.0 } else { (0.4 * ad).min(1.6) };
    (alpha, blur, 0.95)
}

/// `word_mode`: word-timed lines are laid out word by word (for the fill) — the same for every
/// line, so a line becoming current doesn't change layout. Off: every line is one text run.
///
/// Text colours never change: brightness is the container's opacity (a spring, so it fades with
/// `opacity_motion`), and a word-timed line always has its fill overlays, driven by lives that
/// are 1 for sung lines and 0 for upcoming ones, so a line change adds and removes nothing.
fn line_el(i: usize, l: &Line, d: isize, active: bool, lives: Option<Vec<Live<f32>>>, place_at: Option<f32>, word_mode: bool) -> El {
    let (alpha, blur, scale) = style(d, active);
    let word_row = word_mode && !l.words.is_empty();
    // Past lines have their overlays at 1 (full text); upcoming ones show the dim base text.
    let opacity = if active || !word_row || d <= 0 { alpha } else { (alpha / BASE_DIM).min(1.0) };
    let base = if word_row { PAL.text.alpha(BASE_DIM) } else { PAL.text };
    let second = l.second_voice;
    let body = if !word_row {
        text(l.text.clone()).font(LINE).color(base).wrap().align(if second { Align::End } else { Align::Start })
    } else {
        let mut words = row().w_full().flex_wrap();
        if second {
            words = words.justify_end();
        }
        for (j, w) in l.words.iter().enumerate() {
            let mut cell = el().key(format!("w{j}")).child(text(w.text.clone()).font(LINE).color(base));
            if let Some(lv) = lives.as_ref().and_then(|v| v.get(j)) {
                cell = cell.child(
                    text(w.text.clone()).font(LINE).color(PAL.text)
                        .pin(Some(0.0), Some(0.0), None, None).live_opacity(lv),
                );
            }
            words = words.child(cell);
        }
        words
    };
    let start = l.start;
    let c = col().key(format!("ly-{i}")).w_full().px(14.0).py(8.0).radius(14.0)
        .hover(crate::theme::ov(0.06)).cursor(Cursor::Pointer).role("button").label(l.text.clone())
        .origin(if second { 1.0 } else { 0.0 }, 0.5).scale(scale).blur(blur)
        .opacity(opacity).opacity_motion(follow::FADE)
        .on_press(move |s: &mut State| {
            follow::user_seek();
            seek_to_line(s, start);
        });
    let c = if second { c.items_end() } else { c.items_start() };
    let c = match place_at {
        // First layout of the current line while placement is pending: put it at the focus
        // point in the frame that draws it (later moves are glides from `poll`).
        Some(at) => c.on_layout_tx(move |_s: &mut State, _r, tx: &mut Tx| {
            if follow::take_snap() {
                tx.scroll_key_to(format!("ly-{i}"), at);
            }
        }),
        None => c,
    };
    c.child(body)
}

fn dots(k: usize, active: Option<(Vec<Live<f32>>, Live<f32>)>) -> El {
    let r = row().key(format!("dots-{k}")).w_full().h(DOTS_H).px(14.0).items_center();
    let Some((lives, breathe)) = active else { return r };
    let mut d = row().key("dots").gap(9.0).origin(0.0, 0.5).live_scale(&breathe);
    for l in &lives {
        d = d.child(
            el().size(12.0, 12.0).radius(6.0).fill(PAL.text.alpha(0.22)).child(
                el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(6.0).fill(PAL.text).live_opacity(l),
            ),
        );
    }
    r.child(d)
}

fn synced(s: &State, song: &Song, l: &Lyrics, cx: &Cx<State>, view: [f32; 2], narrow: bool) -> El {
    let t = s.playback.position_now();
    // What `poll` shows (it may hold a line change back a frame), else the clock's.
    let (cur, gap) = follow::shown(&song.id).unwrap_or_else(|| {
        let cur = l.line_at(t);
        (cur, follow::active_gap(l, cur, t))
    });
    let cur_i: isize = cur.map_or(-1, |c| c as isize);
    let wbucket = view[0] as u32;

    // Where lines sit (layout space), so `poll` can glide to the current one.
    if let Some(vp) = cx.layout("np-lyrics") {
        let from = cur_i.max(1) as usize - 1;
        let tops = (from..=from + 4)
            .filter_map(|k| cx.layout(&format!("ly-{k}")).map(|r| (k, r[1] - vp[1])))
            .collect();
        follow::publish(tops, vp[3]);
    }

    if s.settings.lyrics_words {
        follow::ensure_lives(&song.id, l, t);
    }
    let mut content = col().key("ly-content").w_full().px(8.0).pt(view[1] * FOCUS).pb(view[1] * 0.6);
    for (i, line) in l.lines.iter().enumerate() {
        if follow::gap_span(l, i).is_some() {
            let lives = (gap == Some(i)).then(|| {
                let span = follow::gap_span(l, i).unwrap();
                let (_, d, b) = follow::dot_lives(&song.id, i, span, t);
                (d, b)
            });
            content = content.child(dots(i, lives));
        }
        let d = i as isize - cur_i;
        // A line that has finished inside an interlude stops being "current".
        let active = d == 0 && gap.is_none();
        let word_mode = s.settings.lyrics_words;
        let epoch = if word_mode { follow::lives_epoch() } else { 0 };
        let dc = d.clamp(-13, 13);
        let id = format!("ly-{i}");
        let sid = song.id.clone();
        let place_at = (d == 0).then_some(view[1] * FOCUS);
        let hbucket = view[1] as u32;
        content = content.child(memo(id, (sid.clone(), i, dc, active, epoch, word_mode, wbucket, hbucket, crate::theme::is_light()), move || {
            line_el(i, line, dc, active, word_mode.then(|| follow::line_lives(i)), place_at, word_mode)
        }));
    }

    frame(narrow)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(48.0)
        .motion(GLIDE)
        .on_scroll_offset(|_at: ScrollPos| {
            if rune::scroll_watch_state().0 {
                follow::mark_manual();
            }
        })
        .child(content)
}

