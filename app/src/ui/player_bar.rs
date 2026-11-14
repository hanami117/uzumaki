//! Bottom player bar: now playing, transport, seek and volume. The transport pieces are shared
//! with the full-screen Now Playing view.

use std::time::Duration;

use rune::{Cursor, Drag, El, Live, Phase, col, el, icon, row, text};

use super::{icons, marquee};
use super::widgets::{Size, art, fmt_time, slider, slider_hit};
use crate::settings::RightTab;
use crate::state::{Repeat, State};
use crate::theme::*;

/// A round icon button (the caller adds `on_press`).
pub fn icon_button(key: &'static str, label: &'static str, ic: &'static rune::Icon, size: f32, color: rune::Color) -> El {
    el().key(key).role("button").label(label).focusable(true).size(36.0, 36.0).radius(18.0).center()
        .hover(crate::theme::ov(0.08)).press(crate::theme::ov(0.12)).cursor(Cursor::Pointer)
        .child(icon(ic, size).color(color))
}

pub fn shuffle_button(s: &State, key: &'static str) -> El {
    let on = s.playback.shuffle;
    icon_button(key, if on { "Shuffle on" } else { "Shuffle off" }, &icons::SHUFFLE, 18.0, if on { PAL.accent } else { PAL.text2 })
        .on_press(|s: &mut State| s.toggle_shuffle())
}

/// Repeat off / all / one; "one" carries a small "1" badge.
pub fn repeat_button(s: &State, key: &'static str) -> El {
    let rep = s.playback.repeat;
    let (label, color) = match rep {
        Repeat::Off => ("Repeat off", PAL.text2),
        Repeat::All => ("Repeat all", PAL.accent),
        Repeat::One => ("Repeat one", PAL.accent),
    };
    let b = icon_button(key, label, &icons::REPEAT, 18.0, color).on_press(|s: &mut State| s.cycle_repeat());
    if rep == Repeat::One {
        b.child(
            el().key(format!("{key}-badge")).size(12.0, 12.0).radius(6.0).center().fill(PAL.accent).pin(None, None, Some(4.0), Some(4.0))
                .child(text("1").font(rune::TextStyle::new(9.0, 600.0)).color(PAL.ink)),
        )
    } else {
        b
    }
}

pub fn play_pause(s: &State, key: &'static str, size: f32) -> El {
    let playing = s.playback.state.is_active();
    el().key(key).role("button").label(if playing { "Pause" } else { "Play" }).focusable(true)
        .size(size, size).radius(size / 2.0).center().fill(PAL.text).hover(crate::theme::ov_inv(0.08))
        .press(crate::theme::ov_inv(0.16)).cursor(Cursor::Pointer)
        .on_press(|s: &mut State| s.toggle_play())
        .child(icon(if playing { &icons::PAUSE } else { &icons::PLAY }, size * 0.45).color(PAL.ink))
}

fn on_seek(s: &mut State, f: f32, phase: Phase, cancelled: bool, _tx: &mut rune::Tx) {
    match phase {
        Phase::Up if cancelled => s.playback.seek_preview = None,
        Phase::Up => s.seek_to(f as f64),
        _ => s.playback.seek_preview = Some(f as f64),
    }
}

/// A seek bar: 16pt hit area around a 4pt track.
pub fn seek_slider(s: &State, key: &'static str, frac: f32, fill: rune::Color) -> El {
    slider_hit(key, 16.0, frac, on_seek).child(seek_track(s, key, 4.0, frac, fill))
}

/// The floating bar's progress line: 2 px thick, with a taller hit area.
fn thin_seek(s: &State, key: &'static str, frac: f32) -> El {
    slider_hit(key, 10.0, frac, on_seek).child(seek_track(s, "seek-thin", 2.0, frac, PAL.text))
}

// ---- smooth seek fills ----
//
// Time reports come once a second (4× with synced lyrics on screen), so a fill laid out from
// them hopped a couple of points each second. The fill instead rides a `Live` rect that `poll`
// sets from `Playback::position_now()`: it moves sub-pixel every few frames and no view runs
// for it. The rect needs the track's size in points, which its `on_layout` reports.

/// Seek-bar fills, by track: what `poll` moves. Lives in `UiState`.
#[derive(Default)]
pub struct SeekFills {
    fills: Vec<Fill>,
    /// The whole second the time labels show: `poll` re-runs the view when the clock passes it.
    second: Option<u64>,
}

struct Fill {
    track: &'static str,
    live: Live<[f32; 4]>,
    size: [f32; 2],
}

impl SeekFills {
    fn live(&self, track: &str) -> Option<&Live<[f32; 4]>> {
        self.fills.iter().find(|f| f.track == track).map(|f| &f.live)
    }
}

/// Points the widest fill moves between `poll` frames while playing: a fraction of a pixel,
/// so the edge glides, yet a 3-minute song on a 500 pt bar draws ~28 frames a second (no view
/// or layout in them), not the display's rate.
const FILL_STEP: f64 = 0.1;
/// The longest `poll` sleeps while playing (a very long song, or before a track's first layout).
const MAX_WAKE: f64 = 0.25;

fn fill_rect(size: [f32; 2], frac: f32) -> [f32; 4] {
    [0.0, 0.0, size[0] * frac.clamp(0.0, 1.0), size[1]]
}

/// A seek bar's track, `h` tall. Until its first layout the fill is laid out at `frac`; after
/// that it is the live rect `poll` moves.
fn seek_track(s: &State, track: &'static str, h: f32, frac: f32, fill: rune::Color) -> El {
    let frac = frac.clamp(0.0, 1.0);
    let el_track = row().w_full().h(h).radius(h / 2.0).clip_rounded(h / 2.0).fill(PAL.track)
        .on_layout_tx(move |s: &mut State, r: [f32; 4], tx: &mut rune::Tx| {
            let size = [r[2], r[3]];
            let frac = progress(s).0;
            match s.ui.seek.fills.iter_mut().find(|f| f.track == track) {
                Some(f) => {
                    // The view builds the same either way: only the live rect changes.
                    f.size = size;
                    f.live.set(fill_rect(size, frac));
                    tx.live_only();
                }
                // First layout: the view runs again and binds the fill to the new rect.
                None => s.ui.seek.fills.push(Fill { track, live: Live::new(fill_rect(size, frac)), size }),
            }
        });
    match s.ui.seek.live(track) {
        Some(live) => el_track.child(el().w(0.0).h_full().fill(fill).live_rect(live)),
        None => el_track.child(el().grow(frac).h_full().fill(fill)).child(el().grow(1.0 - frac)),
    }
}

/// Runs before every frame (`ui::poll`): moves the seek fills to the clock, and while playing
/// asks for the next frame when the widest fill will have moved `FILL_STEP` (or the time
/// labels' second turns over, if sooner). Returns true when that second changed.
pub fn poll(s: &mut State, t: f64, tx: &mut rune::Tx) -> bool {
    let Some(song) = &s.playback.song else { return false };
    let lines = if s.ui.now_playing {
        // The bar is covered: its lines wait at the start.
        s.ui.marquee.rest_all()
    } else {
        let (title, sub) = (song.title.clone(), subtitle(song));
        let a = s.ui.marquee.step("bar-title", &title, CARD_TITLE, t, tx);
        let b = s.ui.marquee.step("bar-sub", &sub, SMALL13, t, tx);
        a || b
    };
    let (frac, elapsed, dur) = progress(s);
    let mut widest = 0.0f32;
    for f in &s.ui.seek.fills {
        f.live.set(fill_rect(f.size, frac));
        widest = widest.max(f.size[0]);
    }
    let second = elapsed.max(0.0) as u64;
    let changed = s.ui.seek.second.replace(second) != Some(second) || lines;
    let pb = &s.playback;
    if pb.state == crate::bridge::PlayState::Playing && pb.seek_preview.is_none() && dur > 0.0 {
        let step = if widest > 0.0 { FILL_STEP * dur / widest as f64 } else { MAX_WAKE };
        // A hair past the turn of the second, so the label shows the new one.
        let turn = (second + 1) as f64 - elapsed + 0.002;
        tx.wake_after(Duration::from_secs_f64(step.min(turn).clamp(0.0, MAX_WAKE)));
    }
    changed
}

pub fn volume_slider(s: &State, key: &'static str, fill: rune::Color) -> El {
    slider(key, s.playback.volume as f32, fill, |s, f, phase, cancelled, _tx| {
        if !(phase == Phase::Up && cancelled) {
            s.set_volume(f as f64);
        }
    })
}

/// Wheel points per 1% of volume. A notched mouse wheel moves 40 points a notch in Rune, so a
/// notch is 5% (as Ctrl+↑/↓); a trackpad's small deltas add up until they make a whole percent.
const WHEEL_POINTS_PER_STEP: f32 = 8.0;

/// Adds a wheel delta to `acc` and returns the whole 1% steps it now makes, keeping the rest.
/// A change of direction starts again from zero, so turning back answers at once.
pub fn wheel_steps(acc: &mut f32, d: f32) -> i32 {
    if *acc * d < 0.0 {
        *acc = 0.0;
    }
    *acc += d;
    let n = (*acc / WHEEL_POINTS_PER_STEP).trunc();
    *acc -= n * WHEEL_POINTS_PER_STEP;
    n as i32
}

/// The volume after `steps` whole percents from `volume`, on the 1% grid, within 0..1.
pub fn stepped_volume(volume: f64, steps: i32) -> f64 {
    (((volume * 100.0).round() + steps as f64) / 100.0).clamp(0.0, 1.0)
}

/// Wheel over the volume control changes the volume instead of scrolling anything: up (or
/// right) is louder. Through `set_volume`, as a drag; only whole percents are sent, so a
/// trackpad sends at most one message per percent, and none at the ends. A trackpad flick's
/// coasting is swallowed (it would keep turning the volume after the fingers lift).
pub fn wheel_volume(el: El) -> El {
    el.on_wheel(|s: &mut State, e: rune::WheelEvent, _tx: &mut rune::Tx| {
        if !e.momentum {
            let d = if e.dy.abs() >= e.dx.abs() { e.dy } else { e.dx };
            wheel_volume_by(s, d);
        }
        true
    })
}

pub fn wheel_volume_by(s: &mut State, d: f32) {
    let steps = wheel_steps(&mut s.ui.volume_wheel, d);
    if steps == 0 {
        return;
    }
    let v = stepped_volume(s.playback.volume, steps);
    if v == s.playback.volume {
        s.ui.volume_wheel = 0.0; // at an end: nothing to send, and nothing saved up past it
        return;
    }
    s.set_volume(v);
}

/// The bar's second line: "Artist — Album", as in Music's player.
fn subtitle(song: &crate::models::Song) -> String {
    match (song.artist.is_empty(), song.album.is_empty()) {
        (_, true) => song.artist.clone(),
        (true, false) => song.album.clone(),
        _ => format!("{} \u{2014} {}", song.artist, song.album),
    }
}

/// The playing song's title and subtitle, each scrolling when it doesn't fit.
fn now_lines(s: &State, song: &crate::models::Song, sub: bool, still: bool) -> El {
    let mut c = col().grow(1.0).min_w(0.0).gap(2.0).justify_center()
        .child(marquee::line(s, "bar-title", song.title.clone(), CARD_TITLE, PAL.text, still));
    if sub {
        c = c.child(marquee::line(s, "bar-sub", subtitle(song), SMALL13, PAL.muted, still));
    }
    c
}

/// (seek fraction, elapsed, duration) for the seek bars, from the smooth playback clock (or
/// the drag preview).
pub fn progress(s: &State) -> (f32, f64, f64) {
    let pb = &s.playback;
    let dur = if pb.duration > 0.0 { pb.duration } else { pb.song.as_ref().map_or(0.0, |x| x.duration) };
    let pos = pb.position_now();
    let frac = pb.seek_preview.unwrap_or(if dur > 0.0 { pos / dur } else { 0.0 });
    let elapsed = if pb.seek_preview.is_some() { frac * dur } else { pos };
    (frac as f32, elapsed, dur)
}

/// Volume as a popover above the bar's volume button (compact bars).
/// `right` / `bottom` place it relative to the window corner.
pub fn volume_popover(s: &State, right: f32, bottom: f32) -> El {
    el().key("vol-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(
            el().key("vol-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .on_press(|s: &mut State| s.ui.volume_open = false)
                .on_secondary_press(|s: &mut State, _p, _tx: &mut rune::Tx| s.ui.volume_open = false),
        )
        .child(wheel_volume(
            row().key("vol-pop").w(180.0).h(44.0).px(14.0).gap(10.0).center_y().radius(12.0).fill(crate::theme::pop_fill())
                .border(1.0, crate::theme::pop_border()).shadow(8.0, 28.0, crate::theme::shadow(0.5))
                .pin(None, None, Some(right), Some(bottom))
                .child(icon(icons::volume(s.playback.volume), 16.0).color(PAL.muted))
                .child(el().grow(1.0).child(volume_slider(s, "volume", PAL.text2))),
        ))
}

/// The Up Next and Lyrics buttons: the right panel on that tab (lit while it shows), or in a
/// narrow window full-screen Now Playing on that column (`narrow::show_tab`). They shake when
/// Ctrl+Shift+B can't open the panel.
fn panel_buttons(s: &State, still: bool) -> (El, El) {
    use super::narrow::{Target, bind, show_tab};
    let lit = |tab: RightTab| super::right_panel::visible(s) && s.settings.right_tab == tab;
    let queue = icon_button("queue", "Up Next", &icons::QUEUE, 18.0, if lit(RightTab::Queue) { PAL.accent } else { PAL.text2 })
        .on_press(|s: &mut State| show_tab(s, RightTab::Queue));
    let lyrics = icon_button("lyrics", "Lyrics", &icons::LYRICS, 18.0, if lit(RightTab::Lyrics) { PAL.accent } else { PAL.text2 })
        .on_press(|s: &mut State| show_tab(s, RightTab::Lyrics));
    (bind(s, Target::PanelButtons, queue, still), bind(s, Target::PanelButtons, lyrics, still))
}

/// Initials on the cover tile only for a song with no artwork (as the sidebar's thumbnails draw
/// them); a cover still loading keeps the plain tile, so a song change doesn't flash letters.
fn initials_pt(song: &crate::models::Song, size: f32) -> f32 {
    if song.artwork.is_none() { size * 0.4 } else { 0.0 }
}

pub fn player_bar(s: &State, compact: bool, still: bool) -> El {
    let Some(song) = &s.playback.song else { return col() };
    let (frac, elapsed, dur) = progress(s);

    // Art and title open Now Playing.
    let cover = if compact { 44.0 } else { 52.0 };
    let menu_song = song.clone();
    let left = row().key("bar-open").role("button").label("Open Now Playing").focusable(true).grow(1.0).min_w(0.0).gap(12.0)
        .center_y().cursor(Cursor::Pointer).on_press(super::open_now_playing)
        .on_secondary_press(move |s: &mut State, p, _tx: &mut rune::Tx| {
            super::menu::open_song(s, menu_song.clone(), super::menu::MenuContext::NowPlaying, [p.x, p.y])
        })
        .child(art(s, song.artwork.as_ref(), song.tile_name(), Size::Small(cover), 6.0, initials_pt(song, cover)))
        .child(now_lines(s, song, true, still));

    let time = |t: String, end: bool| {
        let r = row().w(40.0);
        let r = if end { r } else { r.justify_end() };
        r.child(text(t).font(SMALL12).color(PAL.muted))
    };
    let seek = row().w_full().max_w(if compact { 4000.0 } else { 520.0 }).gap(10.0).center_y()
        .child(time(fmt_time(elapsed), false))
        .child(el().grow(1.0).min_w(0.0).child(seek_slider(s, "seek", frac, PAL.text)))
        .child(time(fmt_time(dur), true));

    let mut transport = row().gap(6.0).center_y();
    if !compact {
        transport = transport.child(shuffle_button(s, "shuffle"));
    }
    transport = transport
        .child(icon_button("prev", "Previous", &icons::PREV, 20.0, PAL.text2).on_press(|s: &mut State| s.previous()))
        .child(play_pause(s, "play-pause", 40.0))
        .child(icon_button("next", "Next", &icons::NEXT, 20.0, PAL.text2).on_press(|s: &mut State| s.next()));
    if !compact {
        transport = transport.child(repeat_button(s, "repeat"));
    }

    let (queue, lyrics) = panel_buttons(s, still);
    // Songs dropped on the queue button go to Up Next.
    let queue = super::dnd::queue_target(s, queue, |e| e.fill(crate::theme::drop_fill(0.25)));

    if compact {
        // Seek on top, then cover and title | transport | queue and a volume button.
        let right = row().justify_end().center_y().gap(2.0).child(super::airplay::button(s, "airplay", PAL.text2)).child(lyrics).child(queue).child(
            wheel_volume(
                icon_button("volume-button", "Volume", icons::volume(s.playback.volume), 18.0, if s.ui.volume_open { PAL.accent } else { PAL.text2 })
                    .on_press(|s: &mut State| {
                        super::airplay::close(s);
                        s.ui.volume_open = !s.ui.volume_open;
                    }),
            ),
        );
        return col().key("player-bar").w_full().h(PLAYER_H).fill(PAL.player)
            .child(el().w_full().h(1.0).fill(PAL.divider))
            .child(row().w_full().px(16.0).pt(4.0).child(seek))
            .child(row().grow(1.0).min_h(0.0).px(16.0).gap(8.0).center_y().child(left).child(transport).child(right));
    }

    let centre = col().grow(2.0).min_w(0.0).items_center().gap(4.0).justify_center().child(transport).child(seek);
    let right = row().grow(1.0).min_w(0.0).justify_end().center_y().gap(4.0)
        .child(super::airplay::button(s, "airplay", PAL.text2))
        .child(lyrics)
        .child(queue)
        .child(wheel_volume(
            row().center_y()
                .child(icon(icons::volume(s.playback.volume), 18.0).color(PAL.muted).ml(6.0))
                .child(el().w(96.0).ml(2.0).child(volume_slider(s, "volume", PAL.text2))),
        ));

    col().key("player-bar").w_full().h(PLAYER_H).fill(PAL.player)
        .child(el().w_full().h(1.0).fill(PAL.divider))
        .child(row().grow(1.0).min_h(0.0).px(20.0).gap(16.0).center_y().child(left).child(centre).child(right))
}

/// Floating style: the capsule plus its margins (12 px all round).
pub const FLOAT_H: f32 = 64.0;
pub const FLOAT_MAX_W: f32 = 760.0;
const FLOAT_MARGIN: f32 = 12.0;
/// Height of the strip reserved at the bottom of the window.
pub const FLOAT_STRIP_H: f32 = FLOAT_H + 2.0 * FLOAT_MARGIN;
/// Below this capsule width the album line and the "..." button are dropped.
const FLOAT_NARROW_W: f32 = 700.0;

/// Where the volume popover goes above a floating capsule in a window `win_w` wide.
pub fn floating_popover_pos(win_w: f32) -> (f32, f32) {
    let w = (win_w - 2.0 * FLOAT_MARGIN).min(FLOAT_MAX_W);
    ((win_w - w) / 2.0 + 16.0, FLOAT_STRIP_H + 2.0)
}

/// The floating bar: a rounded capsule centred in a transparent strip, so the page background
/// (and the background picture) shows around it.
pub fn floating_bar(s: &State, win_w: f32, still: bool) -> El {
    let Some(song) = &s.playback.song else { return col() };
    let (frac, elapsed, dur) = progress(s);
    let w = (win_w - 2.0 * FLOAT_MARGIN).min(FLOAT_MAX_W);
    let narrow = w < FLOAT_NARROW_W;

    let transport = row().gap(2.0).center_y()
        .child(shuffle_button(s, "shuffle"))
        .child(icon_button("prev", "Previous", &icons::PREV, 20.0, PAL.text2).on_press(|s: &mut State| s.previous()))
        .child(play_pause(s, "play-pause", 44.0))
        .child(icon_button("next", "Next", &icons::NEXT, 20.0, PAL.text2).on_press(|s: &mut State| s.next()))
        .child(repeat_button(s, "repeat"));

    let menu_song = song.clone();
    let lines = now_lines(s, song, !narrow, still);
    let open = row().key("bar-open").role("button").label("Open Now Playing").focusable(true).w_full().min_w(0.0).gap(10.0)
        .center_y().cursor(Cursor::Pointer).on_press(super::open_now_playing)
        .on_secondary_press(move |s: &mut State, p, _tx: &mut rune::Tx| {
            super::menu::open_song(s, menu_song.clone(), super::menu::MenuContext::NowPlaying, [p.x, p.y])
        })
        .child(art(s, song.artwork.as_ref(), song.tile_name(), Size::Small(36.0), 6.0, initials_pt(song, 36.0)))
        .child(lines);
    let seek = if narrow {
        thin_seek(s, "seek", frac)
    } else {
        // Elapsed and remaining time ride the same row as the thin bar (tabular digits, so they don't jitter).
        let time = |key: &'static str, t: String, end: bool| {
            let r = row().key(key).min_w(34.0);
            let r = if end { r } else { r.justify_end() };
            r.child(text(t).font(SMALL12.features("'tnum' 1")).color(PAL.muted))
        };
        row().w_full().gap(8.0).center_y()
            .child(time("bar-elapsed", fmt_time(elapsed), false))
            .child(el().grow(1.0).min_w(0.0).child(thin_seek(s, "seek", frac)))
            .child(time("bar-remaining", format!("\u{2212}{}", fmt_time((dur - elapsed).max(0.0))), true))
    };
    let now = col().grow(1.0).min_w(0.0).gap(2.0).justify_center().child(open).child(seek);

    let (queue, lyrics) = panel_buttons(s, still);
    let queue = super::dnd::queue_target(s, queue, |e| e.fill(crate::theme::drop_fill(0.25)));
    let volume = wheel_volume(
        icon_button("volume-button", "Volume", icons::volume(s.playback.volume), 18.0, if s.ui.volume_open { PAL.accent } else { PAL.text2 })
            .on_press(|s: &mut State| {
                        super::airplay::close(s);
                        s.ui.volume_open = !s.ui.volume_open;
                    }),
    );
    let mut right = row().center_y().gap(2.0);
    if !narrow {
        let more_song = song.clone();
        right = right.child(
            icon_button("bar-more", "More", &icons::MORE, 18.0, PAL.text2).on_drag(
                move |s: &mut State, d: &mut Drag, _tx: &mut rune::Tx| {
                    if d.phase == Phase::Up && !d.cancelled() {
                        let at = d.window_point();
                        super::menu::open_song(s, more_song.clone(), super::menu::MenuContext::NowPlaying, at);
                    }
                },
            ),
        );
    }
    right = right.child(super::airplay::button(s, "airplay", PAL.text2)).child(lyrics).child(queue).child(volume);

    let capsule = row().key("player-bar").w_full().max_w(FLOAT_MAX_W).h(FLOAT_H).px(14.0).gap(14.0).center_y().radius(FLOAT_H / 2.0)
        .fill(PAL.player.alpha(0.92)).border(1.0, crate::theme::ov(0.08))
        .shadow(6.0, 24.0, crate::theme::shadow(0.4))
        .child(transport).child(now).child(right);

    row().key("player-strip").w_full().h(FLOAT_STRIP_H).px(FLOAT_MARGIN).center().child(capsule)
}
