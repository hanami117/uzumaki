//! Full-screen Now Playing: tinted background, cover, transport, and synced lyrics or Up Next.
//!
//! Motion is Rune's: the overlay enters and exits with a bottom-edge transition on springs, layout
//! changes (lyrics on/off, resizing) spring by key, and the lyrics glide on a scroll spring (see
//! `lines.rs`). `poll` asks for frames only while lyrics are on screen, synced and playing.

mod lines;
mod follow;

use rune::{Cx, Edge, El, Spring, SpringPair, TextStyle, Transition, Tx, col, el, icon, row, text};

use super::icons;
use super::player_bar::{icon_button, play_pause, progress, repeat_button, seek_slider, shuffle_button, volume_slider, wheel_volume};
use super::widgets::{Size, art, fmt_time};
use crate::art::{NOW_PLAYING_PX, SMALL_PX};
use crate::lyrics::Lyrics;
use crate::models::Song;
use crate::state::{Load, State};
use crate::theme::*;

/// Opens Now Playing; the overlay slides up from the bottom.
pub fn open(s: &mut State) {
    if !s.ui.now_playing {
        crate::framelog::watch("now playing opens", std::time::Duration::from_millis(900));
    }
    s.ui.now_playing = true;
    follow::reset();
}

pub fn close(s: &mut State) {
    if s.ui.now_playing {
        crate::framelog::watch("now playing closes", std::time::Duration::from_millis(700));
    }
    s.ui.now_playing = false;
    s.set_fine_time(false);
    // The clip holds its frame (no decoding) while the overlay slides away, then goes
    // (`MotionState::park`); hiding the window stops it at once (`hide_on_close`).
    s.motion.park();
}

/// Catalog id lyrics are keyed by.
fn lyrics_key(song: &Song) -> Option<&str> {
    match &song.catalog_id {
        Some(c) => Some(c),
        None if song.is_local() => None, // lyrics are Apple's only
        None if !song.id.starts_with("i.") => Some(&song.id),
        None => None,
    }
}

fn lyrics_load<'a>(s: &'a State, song: &Song) -> Option<&'a Load<Option<Lyrics>>> {
    s.lyrics.get(lyrics_key(song)?)
}

/// The current song's lyrics, when loaded and non-empty.
fn lyrics_of(s: &State) -> Option<&Lyrics> {
    let song = s.playback.song.as_ref()?;
    match lyrics_load(s, song) {
        Some(Load::Ready(Some(l))) if !l.lines.is_empty() => Some(l),
        _ => None,
    }
}

/// The column beside the cover holds the lyrics (not switched off, and Up Next isn't there).
fn lyrics_on(s: &State) -> bool {
    s.settings.now_playing_lyrics && !s.settings.now_playing_queue
}

/// True while the lyrics column shows time-synced lyrics (what `poll` animates).
fn synced_visible(s: &State) -> bool {
    s.ui.now_playing && lyrics_on(s) && lyrics_of(s).is_some_and(|l| l.synced)
}

/// The lyrics button: lyrics on or off, or back from Up Next. Lyrics coming back are placed on
/// the current line at once, as when Now Playing opens.
fn toggle_lyrics(s: &mut State) {
    s.toggle_now_playing_lyrics();
    if lyrics_on(s) {
        follow::reset();
    }
}

/// The queue button: Up Next in the column, or back to what was there.
fn toggle_queue(s: &mut State) {
    s.toggle_now_playing_queue();
    if lyrics_on(s) {
        follow::reset();
    }
}

/// Now Playing is on screen (open, with a song to show).
pub fn is_open(s: &State) -> bool {
    s.ui.now_playing && s.playback.song.is_some()
}

/// What the column beside the cover holds now, if anything (the lyrics button's "on" state
/// counts, even for a song with no lyrics: the button is lit and the hint shows).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // the macOS menu bar and the tests
pub fn column(s: &State) -> Option<crate::settings::RightTab> {
    use crate::settings::RightTab;
    if s.settings.now_playing_queue {
        Some(RightTab::Queue)
    } else if lyrics_on(s) {
        Some(RightTab::Lyrics)
    } else {
        None
    }
}

/// The lyrics and Up Next shortcuts while Now Playing is open: they press its own buttons, so
/// the key and the button can't disagree. False when Now Playing isn't open.
pub fn key_column(s: &mut State, tab: crate::settings::RightTab) -> bool {
    if !is_open(s) {
        return false;
    }
    match tab {
        crate::settings::RightTab::Lyrics => toggle_lyrics(s),
        crate::settings::RightTab::Queue => toggle_queue(s),
    }
    true
}

fn hex_rgb(h: &str) -> Option<[u8; 3]> {
    let h = h.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// The cover's colour: Apple's `bg` for catalog art, else the average colour of the loaded image
/// (small cover first, it is the one already loaded), else a neutral dark.
pub fn cover_tint(s: &State, song: &Song) -> [u8; 3] {
    const NEUTRAL: [u8; 3] = [0x2a, 0x2a, 0x32];
    let Some(a) = &song.artwork else { return NEUTRAL };
    a.bg.as_deref()
        .and_then(hex_rgb)
        .or_else(|| s.art.tint(&a.url(SMALL_PX)))
        .or_else(|| s.art.tint(&a.url(NOW_PLAYING_PX)))
        .unwrap_or(NEUTRAL)
}

/// `rgb` mixed `k` of the way from the Now Playing base (#0b0b0e) in sRGB.
fn mix_base(rgb: [u8; 3], k: f32) -> rune::Color {
    const BASE: [f32; 3] = [11.0, 11.0, 14.0];
    let c: Vec<u8> = (0..3).map(|i| (BASE[i] + (rgb[i] as f32 - BASE[i]) * k).round() as u8).collect();
    rune::color::hex(&format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).unwrap_or(PAL.np_base)
}

/// The big cover. It always requests `NOW_PLAYING_PX` (never a size derived from layout, which
/// would change the URL while the window or the overlay moves) and draws it at whatever size layout
/// gives; the small, already-loaded cover shows scaled up underneath until it is ready.
/// A song with no artwork gets the tinted tile with initials the rest of the app draws.
fn cover(s: &State, song: &Song, size: f32) -> El {
    let radius = (size * 0.03).clamp(8.0, 16.0);
    let key = format!("cover-{}", song.id);
    let Some(a) = &song.artwork else {
        let tile = art(s, None, song.tile_name(), Size::Fill, 0.0, (size * 0.18).max(14.0)).key(key);
        return frame(size, radius).child(tile);
    };
    let mut stack = el().key(key).w_full().h_full().fill(PAL.card_hover);
    let fill = |key: &'static str, img: rune::Image| {
        el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).pixels_fit(img.pixels(), rune::SurfaceFit::Fill)
    };
    if let Some(img) = s.art.get(a, SMALL_PX) {
        stack = stack.child(fill("small", img));
    }
    if let Some(img) = s.art.get(a, NOW_PLAYING_PX) {
        stack = stack.child(fill("big", img));
    }
    let framed = frame(size, radius).child(stack);
    match motion_layer(s, song, size) {
        Some(layer) => framed.child(layer),
        None => framed,
    }
}

/// Animated artwork over the still (`state::motion`): fades in once its first frame is drawn,
/// in the same box, so nothing moves (before that, the Mac's clip draws nearly transparent so
/// its first frame gets drawn at all). Kept outside the per-song stack so the next song of the
/// same album keeps the clip running. Not on the small cover of a narrow window.
///
/// It leaves at once, without a fade: closing Now Playing stops the clip that same frame, and
/// a fading copy of a stopped video doesn't follow the overlay sliding away (it was cut off
/// and smeared over the page on a quick Cmd+Shift+F). The still under it moves with the overlay.
fn motion_layer(s: &State, song: &Song, size: f32) -> Option<El> {
    if size < 160.0 {
        return None;
    }
    let album = s.motion_album(song);
    if let Some(layer) = s.motion.layer_for(album) {
        return Some(layer.key("np-motion").transition(motion_transition()));
    }
    Some(s.motion.warm_layer_for(album)?.key("np-motion-warm").transition(Transition::none()))
}

/// The clip layer's transition (Now Playing and album pages): fades in, gone at once.
pub(crate) fn motion_transition() -> Transition {
    Transition::asymmetric(Transition::opacity(), Transition::none())
}

/// The cover's rounded, shadowed box.
fn frame(size: f32, radius: f32) -> El {
    el().key("np-cover").size(size, size).radius(radius).clip_rounded(radius)
        .shadow(14.0, 40.0, crate::theme::shadow(0.45)).fill(PAL.card_hover)
}

/// Title and artist, with the favourite star beside them when the song can have one.
fn meta(s: &State, song: &Song) -> El {
    let text_block = col().gap(4.0).grow(1.0).min_w(0.0)
        .child(text(song.title.clone()).font(TextStyle::new(22.0, 600.0)).color(PAL.text).truncate())
        .child(
            text(if song.album.is_empty() { song.artist.clone() } else { format!("{} \u{2014} {}", song.artist, song.album) })
                .font(TextStyle::new(16.0, 400.0)).color(PAL.muted).truncate(),
        );
    let mut r = row().w_full().gap(8.0).center_y().child(text_block);
    if s.can_favourite(song) {
        let on = s.is_favourite(song);
        let (glyph, color) = if on { (&icons::STAR_FILLED, PAL.accent) } else { (&icons::STAR, PAL.text2) };
        r = r.child(icon_button("np-star", if on { "Undo Favourite" } else { "Favourite" }, glyph, 20.0, color)
            .on_press(|s: &mut State| {
                if let Some(song) = s.playback.song.clone() {
                    s.toggle_favourite_songs(&[song]);
                }
            }));
    }
    r
}

fn seek_row(s: &State) -> El {
    let (frac, elapsed, dur) = progress(s);
    let time = |t: String, end: bool| {
        let r = row().w(44.0);
        let r = if end { r } else { r.justify_end() };
        r.child(text(t).font(SMALL12).color(PAL.muted))
    };
    row().w_full().gap(10.0).center_y()
        .child(time(fmt_time(elapsed), false))
        .child(el().grow(1.0).min_w(0.0).child(seek_slider(s, "np-seek", frac, PAL.text)))
        .child(time(fmt_time(dur), true))
}

/// Shuffle, previous, play, next, repeat, and AutoPlay's ∞ after repeat where AutoPlay applies
/// (`autoplay_offered`). The two ends are always laid out, a button or an empty cell of the same
/// size, so the row is symmetric and nothing moves when a station starts or ends and the ∞
/// comes or goes: play stays under the middle of the cover and the scrubber.
fn transport(s: &State) -> El {
    let offered = s.autoplay_offered();
    let end_cell = |b: Option<El>| el().size(TRANSPORT_BUTTON, TRANSPORT_BUTTON).children(b);
    row().w_full().justify_center().center_y().gap(10.0)
        .child(end_cell(None))
        .child(shuffle_button(s, "np-shuffle"))
        .child(icon_button("np-prev", "Previous", &icons::PREV, 22.0, PAL.text).on_press(|s: &mut State| s.previous()))
        .child(play_pause(s, "np-play-pause", 56.0))
        .child(icon_button("np-next", "Next", &icons::NEXT, 22.0, PAL.text).on_press(|s: &mut State| s.next()))
        .child(repeat_button(s, "np-repeat"))
        .child(end_cell(offered.then(|| autoplay_button(s))))
}

/// The size of a transport button (`player_bar::icon_button`).
const TRANSPORT_BUTTON: f32 = 36.0;

/// ∞ like shuffle and repeat: the accent while AutoPlay is on, dimmed while it is on but paused
/// by Repeat, the quiet grey while off. The same setting and tooltips as Up Next's ∞.
fn autoplay_button(s: &State) -> El {
    let color = match (s.settings.autoplay, s.playback.repeat) {
        (false, _) => PAL.text2,
        (true, crate::state::Repeat::Off) => PAL.accent,
        (true, _) => PAL.accent.alpha(0.45),
    };
    icon_button("np-autoplay", "AutoPlay", &icons::INFINITY, 18.0, color)
        .role("switch")
        .help(super::up_next::autoplay_help(s))
        .on_press_tx(|s: &mut State, tx: &mut rune::Tx| {
            s.toggle_autoplay();
            tx.instant();
        })
}

/// The seek row's time columns: the volume row's speakers sit in the same slots, so its slider
/// lines up with the scrubber.
const TIME_W: f32 = 44.0;
const TIME_GAP: f32 = 10.0;

/// One cell of the stacked button row: fixed, so two buttons (Linux, no Words) sit close to the
/// middle instead of drifting to the edges, and four fill about a phone's width.
const BUTTON_CELL_W: f32 = 76.0;

/// The volume slider (with the wheel on its whole row).
fn volume(s: &State) -> El {
    el().grow(1.0).min_w(0.0).child(volume_slider(s, "np-volume", PAL.text2))
}

/// Volume on its own row, as wide as the scrubber: a quiet speaker on the left (crossed out when
/// muted), a loud one on the right (both only marks; the slider and the wheel set the volume).
fn volume_row(s: &State) -> El {
    let slot = |ic: &'static rune::Icon, end: bool| {
        let r = row().w(TIME_W).center_y();
        let r = if end { r } else { r.justify_end() };
        r.child(icon(ic, 18.0).color(PAL.muted))
    };
    wheel_volume(
        row().key("np-volume-row").w_full().gap(TIME_GAP).center_y()
            .child(slot(if s.playback.volume < 0.005 { &icons::VOLUME_MUTE } else { &icons::VOLUME_LOW }, false))
            .child(volume(s))
            .child(slot(&icons::VOLUME_HIGH, true)),
    )
}

/// The small buttons, in order: lyrics (with Words right after it while it applies), AirPlay
/// (only where it exists), Up Next. Each lit while it has the column beside the cover (lyrics and
/// Up Next take turns).
fn secondary_buttons(s: &State) -> Vec<El> {
    let lyrics = lyrics_on(s);
    let queue = s.settings.now_playing_queue;
    let mut v = vec![
        icon_button("np-lyrics-toggle", if lyrics { "Hide lyrics" } else { "Show lyrics" }, &LYRICS_ICON, 18.0,
            if lyrics { PAL.accent } else { PAL.text2 })
            .on_press(toggle_lyrics),
    ];
    v.extend(word_mode_button(s));
    v.extend(super::airplay::button(s, "np-airplay", PAL.text2));
    v.push(
        icon_button("np-queue-toggle", if queue { "Hide Up Next" } else { "Show Up Next" }, &icons::QUEUE, 18.0,
            if queue { PAL.accent } else { PAL.text2 })
            .on_press(toggle_queue),
    );
    v
}

/// Under the transport. `stacked` (a narrow window): volume on its own row, then the small
/// buttons spread evenly in equal cells across the same width. Otherwise one row: the volume,
/// lined up with the scrubber's left end, then the buttons at an even spacing.
fn bottom_rows(s: &State, stacked: bool) -> El {
    if stacked {
        let cells = secondary_buttons(s).into_iter().map(|b| row().w(BUTTON_CELL_W).justify_center().child(b));
        return col().w_full().gap(14.0)
            .child(volume_row(s))
            .child(row().w_full().justify_center().child(row().key("np-buttons").center_y().children(cells)));
    }
    row().w_full().center_y()
        .child(wheel_volume(
            row().key("np-volume-row").grow(1.0).min_w(0.0).gap(TIME_GAP).center_y()
                .child(row().w(TIME_W).justify_end().child(icon(icons::volume(s.playback.volume), 18.0).color(PAL.muted)))
                .child(volume(s)),
        ))
        .child(row().key("np-buttons").ml(20.0).gap(6.0).center_y().children(secondary_buttons(s)))
}

/// Word-by-word / line-by-line highlight, a round button like its neighbours (lit for word by
/// word). Only shown with lyrics on and only when the song's lyrics carry word timing: while
/// loading, with no lyrics, or with line timing only (Apple has word timing for some songs; on
/// the others both modes look the same) there is nothing to switch, so it is absent. The setting
/// itself is untouched and applies to the next song.
fn word_mode_button(s: &State) -> Option<El> {
    if !lyrics_on(s) || !has_word_timing(s) {
        return None;
    }
    let words = s.settings.lyrics_words;
    let help = if words { "Highlighting word by word (click for whole lines)" } else { "Highlighting whole lines (click for word by word)" };
    Some(
        icon_button("np-word-mode", "Lyrics highlight: word by word or line by line", &WORDS_ICON, 18.0,
            if words { PAL.accent } else { PAL.text2 })
            .help(help)
            .on_press(|s: &mut State| s.toggle_lyrics_words()),
    )
}

/// The current song's loaded lyrics are synced with timing for single words.
fn has_word_timing(s: &State) -> bool {
    lyrics_of(s).is_some_and(|l| l.synced && l.lines.iter().any(|x| !x.words.is_empty()))
}

/// Up Next in the lyrics' place: the songs after the playing one (the queue button).
fn queue_panel(s: &State, narrow: bool, cx: &Cx<State>) -> El {
    let c = col().key("np-queue");
    let c = if narrow { c.w_full().grow(1.0).min_h(0.0) } else { c.grow(1.0).max_w(520.0).h_full().pt(24.0) };
    c.child(super::up_next::list(s, super::up_next::Look::NowPlaying, cx))
}

/// Word by word: two lines of text broken into words, the first word lit (filled).
static WORDS_ICON: rune::Icon = rune::Icon {
    name: "np-words",
    view: 24.0,
    body: r#"<path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" d="M13 8.5h3.5M19.5 8.5h1.5M3.5 15.5h6M12.5 15.5h8"/><rect x="2.5" y="6" width="7.5" height="5" rx="1.5" fill="currentColor"/>"#,
};

static LYRICS_ICON: rune::Icon = rune::Icon {
    name: "np-lyrics",
    view: 24.0,
    body: r#"<path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2zM8 8h8M8 12h5"/>"#,
};

pub fn overlay(s: &State, cx: &mut Cx<State>) -> Option<El> {
    let song = s.playback.song.as_ref()?;
    let win = cx.layout_size("app-root").unwrap_or([1360.0, 860.0]);
    let tint = cover_tint(s, song);
    // A narrow window stacks the lyrics under a small cover header.
    let narrow = crate::theme::is_narrow(win[0]);

    let load = lyrics_load(s, song);
    let has = matches!(load, Some(Load::Ready(Some(l))) if !l.lines.is_empty());
    let pending = matches!(load, Some(Load::Loading) | Some(Load::Idle) | None) && lyrics_key(song).is_some();
    let show_queue = s.settings.now_playing_queue;
    let show_lyrics = lyrics_on(s) && (has || pending);
    let side = show_queue || show_lyrics;

    let enter = Spring::new(0.55, 0.0);
    let exit = Spring::new(0.42, 0.0);
    let bg = super::backdrop::np_layer(s, cx, win);
    // The root's own paint never changes: an opaque base, with the tint or a picture as a child
    // on it. A picture can land mid-slide (it renders off the UI thread); it then fades in over
    // the tint. Switching the root itself from the tint gradient to a fill made Rune spring the
    // fill up from clear, so the page underneath flashed through the sliding overlay.
    let mut root = el().key("np").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(PAL.np_base)
        .transition(Transition::move_edge(Edge::Bottom).springs(SpringPair::new(enter, exit)))
        .on_press(|_s: &mut State| {});
    root = match bg {
        Some(bg) => root.child(bg),
        None => root.child(
            el().key("np-tint").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .gradient(mix_base(tint, 0.34), PAL.np_base)
                .transition(Transition::opacity()),
        ),
    };

    let close_btn = icon_button("np-close", "Close Now Playing", &icons::CHEVRON_DOWN, 22.0, PAL.text2)
        .size(44.0, 44.0).radius(22.0).pin(None, Some(16.0), Some(16.0), None).on_press(close);

    let side_panel = |s: &State, cx: &Cx<State>, narrow: bool| {
        if show_queue { queue_panel(s, narrow, cx) } else { lines::panel(s, song, cx, narrow) }
    };
    let body = if side && narrow {
        let head = row().key("np-head").w_full().gap(14.0).center_y()
            .child(cover(s, song, 64.0))
            .child(el().grow(1.0).min_w(0.0).child(meta(s, song)));
        let lower = col().key("np-low").w_full().gap(10.0).child(seek_row(s)).child(transport(s)).child(bottom_rows(s, true));
        col().w_full().h_full().px(20.0).pt(72.0).pb(20.0).gap(12.0)
            .child(head)
            .child(side_panel(s, cx, true))
            .child(lower)
    } else {
        let size = if side {
            (win[1] - 72.0 - 300.0).clamp(180.0, 380.0).min(win[0] * 0.4)
        } else {
            (win[1] - 330.0).clamp(200.0, 560.0).min(win[0] * 0.7)
        };
        let mut left = col().key("np-left").w(size).gap(14.0)
            .child(cover(s, song, size))
            .child(meta(s, song).mt(6.0))
            .child(seek_row(s))
            .child(transport(s))
            .child(bottom_rows(s, narrow));
        if !side && lyrics_on(s) && !pending {
            left = left.child(
                row().w_full().justify_center().child(
                    text(if matches!(load, Some(Load::Failed(_))) { "Couldn\u{2019}t load lyrics" } else { "No lyrics for this song" })
                        .font(SMALL13).color(PAL.dim).key("np-nolyrics"),
                ),
            );
        }
        let mut r = row().w_full().h_full().px(48.0).py(36.0).gap(56.0).center().child(left);
        if side {
            r = r.child(side_panel(s, cx, false));
        }
        r
    };
    root = root.child(body).child(close_btn);
    Some(root)
}

/// Runs before every frame (`AppSpec::poll`). Returns true only when visible state changed;
/// frames while lyrics animate come from `wake_after` and live values, and nothing is scheduled
/// otherwise, so an idle or paused window draws nothing.
pub fn poll(s: &mut State, landed: bool, tx: &mut Tx) -> bool {
    let motion = s.poll_motion();
    // A clip held because the window can't draw: a timer frame each second finds out when it
    // can again (`state::motion::frame_outcome`).
    if s.motion.covered && s.motion.playback.is_some() {
        tx.wake_after(std::time::Duration::from_secs(1));
    }
    if let Some(d) = s.motion.closing_left() {
        tx.wake_after(d); // drops the parked clip once the slide is over
    }
    if s.motion.playback.as_ref().is_some_and(|p| p.warming()) {
        tx.wake_after(std::time::Duration::from_millis(250));
    }
    // A lookup that failed is asked again later: a frame then, even if nothing else draws.
    if let (true, Some(d)) = (s.ui.now_playing, s.motion.next_retry()) {
        tx.wake_after(d);
    }
    lyrics_poll(s, landed, tx) || motion
}

fn lyrics_poll(s: &mut State, landed: bool, tx: &mut Tx) -> bool {
    let visible = synced_visible(s);
    let playing = visible && matches!(s.playback.state, crate::bridge::PlayState::Playing);
    s.set_fine_time(playing);
    if !visible {
        follow::idle();
        return false;
    }
    let Some(lyr) = lyrics_of(s) else { return false };
    let song_id = s.playback.song.as_ref().map(|x| x.id.as_str()).unwrap_or("");
    let t = s.playback.position_now();
    let changed = follow::step(song_id, lyr, t, playing, landed, tx);
    if playing {
        tx.wake_after(std::time::Duration::ZERO);
    }
    changed
}

fn seek_to_line(s: &mut State, start: f64) {
    let pb = &s.playback;
    let dur = if pb.duration > 0.0 { pb.duration } else { pb.song.as_ref().map_or(0.0, |x| x.duration) };
    if dur > 0.0 {
        s.seek_to(start / dur);
    }
}

// ---- scenes (synthetic data only) -----------------------------------------------------------

/// Original synthetic lyrics: 14 lines, a 12 s intro and an 8 s gap before line 7. With `words`,
/// every word has its own timing.
pub fn synthetic_lyrics(words: bool) -> Lyrics {
    use crate::lyrics::{Line, Word};
    const LINES: [&str; 14] = [
        "The last tram leaves without a sound",
        "and the depot lights go low",
        "I fold the map along its seams",
        "somewhere past the overpass",
        "(we never needed the map)",
        "Every window holds a different sky",
        "copper, then a quiet blue",
        "and the driver hums a tune I almost know",
        "a tune you almost wrote",
        "(hold the door, hold the door)",
        "Terminus is only a word",
        "for the place we stop pretending",
        "so we walk the rest of the way",
        "singing it back to the dark",
    ];
    let mut t = 12.0;
    let mut out = Vec::new();
    for (i, text) in LINES.iter().enumerate() {
        if i == 7 {
            t += 8.0;
        }
        let parts: Vec<&str> = text.split(' ').collect();
        let start = t;
        let ws: Vec<Word> = if words {
            parts.iter().enumerate().map(|(j, p)| Word {
                start: start + 0.45 * j as f64,
                end: start + 0.45 * (j as f64 + 1.0),
                text: if j + 1 < parts.len() { format!("{p} ") } else { (*p).to_string() },
            }).collect()
        } else {
            Vec::new()
        };
        let end = start + 0.45 * parts.len() as f64;
        out.push(Line { start, end, text: (*text).into(), second_voice: i % 5 == 4, words: ws });
        t += 5.0;
    }
    Lyrics { synced: true, lines: out }
}

fn scene_base(s: &mut State, lyr: Lyrics, position: f64) {
    let song = Song {
        id: "np-scene".into(),
        catalog_id: Some("cat-np".into()),
        title: "Last Service".into(),
        artist: "Juniper Vale".into(),
        album: "Night Bus Hymns".into(),
        album_is_fallback: false,
        duration: 241.0,
        track_number: Some(1),
        disc_number: Some(1),
        explicit: false,
        artwork: Some(crate::models::Artwork { template: "https://example.invalid/{w}x{h}.{f}".into(), bg: Some("a79be6".into()) }),
        playable: true,
        genre: None,
        year: None,
        album_id: None,
        artists: Vec::new(),
    };
    s.playback.song = Some(song);
    s.playback.duration = 241.0;
    s.playback.position = position;
    s.lyrics.insert("cat-np".into(), Load::Ready(Some(lyr)));
    s.ui.now_playing = true;
}

/// Word-timed lyrics, mid-line (word fill visible).
pub fn scene_np_words(s: &mut State) {
    scene_base(s, synthetic_lyrics(true), 12.0 + 5.0 * 2.0 + 0.9);
}

/// Inside the 8 s gap before line 7: the breathing dots.
pub fn scene_np_interlude(s: &mut State) {
    scene_base(s, synthetic_lyrics(true), 12.0 + 5.0 * 6.0 + 4.6 + 3.0);
}

/// Before the first line (intro dots).
pub fn scene_np_intro(s: &mut State) {
    scene_base(s, synthetic_lyrics(true), 5.0);
}

/// Plain (unsynced) lyrics.
pub fn scene_np_unsynced(s: &mut State) {
    let mut l = synthetic_lyrics(false);
    l.synced = false;
    for x in &mut l.lines {
        x.start = 0.0;
        x.end = 0.0;
    }
    scene_base(s, l, 30.0);
}

/// Line-timed lyrics (no word timing) with Words on: no switch.
pub fn scene_np_lines_only(s: &mut State) {
    scene_base(s, synthetic_lyrics(false), 12.0 + 5.0 * 2.0 + 0.9);
    s.settings.lyrics_words = true;
}

/// Line-timed lyrics with the lyrics column switched off.
pub fn scene_np_cover_only(s: &mut State) {
    scene_base(s, synthetic_lyrics(false), 30.0);
    s.settings.now_playing_lyrics = false;
}

/// Animated artwork from the local fixture clip (`app/fixtures/motion-clip.mp4`, 256 px, 2 s,
/// made with ffmpeg's `gradients` source): playing, so the cover crossfades to the clip after
/// its first frame. Use `--frames`/`--at` to see it move. macOS draws it with Rune's player
/// (decoded per frame headless, so deterministic). Elsewhere, and with `UZUMAKI_MOTION=ffmpeg`,
/// ffmpeg (which must be installed) decodes in real time beside the fast headless clock: the
/// clip shows from about `--at 10`.
pub fn scene_np_motion(s: &mut State, lyrics: bool) {
    scene_base(s, synthetic_lyrics(false), 30.0);
    s.settings.now_playing_lyrics = lyrics;
    s.playback.state = crate::bridge::PlayState::Playing;
    let album = "1440857781";
    if let Some(song) = s.playback.song.as_mut() {
        song.album_id = Some(album.into());
    }
    scene_clip(s, album);
}

/// Gives catalog `album` the local fixture clip (on disk, nothing fetched), for scenes.
/// For measuring with a real clip: `UZUMAKI_SCENE_CLIP=<file>,<side>,<fps>` (a square one).
pub fn scene_clip(s: &mut State, album: &str) {
    use crate::motion::{Clip, MotionArt};
    let master = "https://example.invalid/motion/P1_default.m3u8";
    s.motion.albums.insert(album.into(), Load::Ready(Some(MotionArt { video: master.into(), preview: None })));
    let path = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/motion-clip.mp4"));
    let mut clip = Clip { path, width: 256, height: 256, fps: 24.0 };
    if let Ok(v) = std::env::var("UZUMAKI_SCENE_CLIP") {
        let p: Vec<&str> = v.split(',').collect();
        if let [file, side, fps] = p.as_slice() {
            let side = side.parse().unwrap_or(768);
            clip = Clip { path: file.into(), width: side, height: side, fps: fps.parse().unwrap_or(crate::motion::clips::DEFAULT_FPS) };
        }
    }
    s.motion.clips.insert(master.into(), Load::Ready(clip));
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    use crate::appearance::NpBg;
    use crate::backdrop::Slot;
    use crate::state::State;

    const DT: f64 = 1.0 / 60.0;

    /// The ∞ after repeat: there where AutoPlay applies, none (and no jump of the other
    /// buttons) in local only mode or in a station, and the row stays symmetric either way.
    #[test]
    fn the_autoplay_button_sits_after_repeat_and_nothing_moves_without_it() {
        let rects = |setup: fn(&mut State), width: f32| {
            let mut s = State::offline();
            (crate::scenes::SCENES.iter().find(|(n, _)| *n == "now-playing").unwrap().1)(&mut s);
            setup(&mut s);
            let mut rt = Runtime::new(s, crate::ui::view, (width, 860.0), Env { system_dark: true, scale: 1.0 });
            rt.frame(DT);
            let key = |k: &str| rt.presented(k).map(|(r, _)| r);
            ["np-shuffle", "np-prev", "np-play-pause", "np-next", "np-repeat", "np-autoplay"].map(key)
        };
        for width in [1360.0, 420.0] {
            let on = rects(|_| {}, width);
            let off = rects(|s| s.settings.music_source = crate::settings::MusicSource::LocalOnly, width);
            let (repeat, auto) = (on[4].expect("repeat"), on[5].expect("offered: the ∞ is there"));
            assert!(auto[0] > repeat[0] + repeat[2], "{width}: right after repeat");
            assert_eq!((auto[1], auto[2], auto[3]), (repeat[1], repeat[2], repeat[3]), "{width}: same size and line as repeat");
            assert!(off[5].is_none(), "{width}: local only mode: no ∞");
            assert_eq!(on[..5], off[..5], "{width}: the other buttons stay where they were");
            // Play sits under the middle of the scrubber, with the ∞ there or not.
            let seek = |setup: fn(&mut State)| {
                let mut s = State::offline();
                (crate::scenes::SCENES.iter().find(|(n, _)| *n == "now-playing").unwrap().1)(&mut s);
                setup(&mut s);
                let mut rt = Runtime::new(s, crate::ui::view, (width, 860.0), Env { system_dark: true, scale: 1.0 });
                rt.frame(DT);
                rt.presented("np-seek").unwrap().0
            };
            let mid = |r: [f32; 4]| r[0] + r[2] / 2.0;
            let centre = mid(seek(|_| {}));
            assert!((mid(on[2].unwrap()) - centre).abs() < 1.0, "{width}: play is centred with the ∞");
            assert!((mid(off[2].unwrap()) - centre).abs() < 1.0, "{width}: and without it");
        }
    }

    /// Animated artwork arriving while Now Playing shows: the clip fades in over the still in
    /// the same box (nothing moves), and closing Now Playing takes it away. macOS's native
    /// player shows its poster at once; the ffmpeg path needs a handle for its first-frame report,
    /// which tests don't have (`state::motion` tests cover it).
    #[cfg(target_os = "macos")]
    #[test]
    fn animated_artwork_fades_in_over_the_still_without_moving_it() {
        if std::env::var("UZUMAKI_MOTION").as_deref() == Ok("ffmpeg") {
            return;
        }
        let mut s = State::offline();
        super::scene_np_motion(&mut s, false);
        s.settings.animated_artwork = crate::settings::AnimatedArtwork::Off;
        let mut rt = Runtime::new(s, crate::ui::view, (1360.0, 860.0), Env { system_dark: true, scale: 1.0 });
        let step = |rt: &mut Runtime<State>| {
            rt.update_if(|s, tx| crate::ui::poll(s, 0.0, tx));
            rt.frame(DT);
        };
        for _ in 0..20 {
            step(&mut rt);
        }
        assert!(rt.presented("np-motion").is_none(), "setting off: the still only");
        let cover = rt.presented("np-cover").unwrap().0;

        rt.update(|s, _| s.set_animated_artwork(crate::settings::AnimatedArtwork::Always));
        let mut seen = Vec::new();
        for _ in 0..60 {
            step(&mut rt);
            if let Some((rect, opacity)) = rt.presented("np-motion") {
                assert_eq!(rect, cover, "the clip fills the cover's box exactly");
                seen.push(opacity);
            }
            assert_eq!(rt.presented("np-cover").unwrap().0, cover, "the cover never moves");
        }
        assert!(seen.iter().any(|o| *o > 0.05 && *o < 0.95), "a crossfade, not a cut: {seen:?}");
        assert!((seen.last().unwrap() - 1.0).abs() < 1e-3, "settles fully shown: {seen:?}");

        rt.update(|s, _| super::close(s));
        for _ in 0..60 {
            step(&mut rt);
        }
        rt.update(|s, _| { s.motion.stop(); }); // the parked clip's timer (real time here)
        step(&mut rt);
        assert!(rt.presented("np-motion").is_none());
        let mut gone = false;
        rt.update(|s, _| gone = s.motion.playback.is_none());
        assert!(gone, "no player left once Now Playing closed");
    }

    /// Keys of every element on screen, leaving ones included.
    #[cfg(target_os = "macos")]
    fn keyed(rt: &Runtime<State>, key: &str) -> Vec<rune::ElementInfo> {
        rt.elements().into_iter().filter(|e| e.key.as_deref() == Some(key)).collect()
    }

    /// 2026-11-14, Cmd+Shift+F pressed quickly: closing dropped the clip's player at once while
    /// Rune kept drawing the leaving overlay as it was, video layer included, so mid-slide the
    /// video's area was cut out. Now the clip is parked on its frame: the leaving overlay's
    /// video moves with it, exactly over the still, until the slide is over; then it goes.
    /// Reopening mid-slide takes the same clip back (no restart, no second decoder).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_clip_slides_away_with_now_playing_and_goes_after() {
        if std::env::var("UZUMAKI_MOTION").as_deref() == Ok("ffmpeg") {
            return;
        }
        crate::clock::freeze();
        let mut s = State::offline();
        super::scene_np_motion(&mut s, false);
        let mut rt = Runtime::new(s, crate::ui::view, (1360.0, 860.0), Env { system_dark: true, scale: 1.0 });
        let step = |rt: &mut Runtime<State>| {
            rt.update_if(|s, tx| crate::ui::poll(s, 0.0, tx));
            rt.frame(DT);
        };
        for _ in 0..40 {
            step(&mut rt);
        }
        let mut g0 = 0;
        rt.update(|s, _| g0 = s.motion.playback.as_ref().unwrap().generation);
        for round in 0..3 {
            rt.update(|s, _| super::close(s));
            let mut moved = false;
            for _ in 0..8 {
                step(&mut rt);
                crate::clock::advance(std::time::Duration::from_millis(16));
                let clip = keyed(&rt, "np-motion");
                let cover = keyed(&rt, "np-cover");
                assert_eq!(clip.len(), 1, "round {round}: the clip leaves with the overlay");
                assert!(clip[0].exiting && cover[0].exiting);
                assert_eq!(clip[0].rect, cover[0].rect, "round {round}: exactly over the still");
                moved |= cover[0].rect[1] > 60.0;
            }
            assert!(moved, "round {round}: it slides");
            let mut parked = false;
            rt.update(|s, _| parked = s.motion.parked() && s.motion.playback.is_none());
            assert!(parked, "round {round}: held, not decoding");
            // Reopened mid-slide: the same clip.
            rt.update(|s, _| super::open(s));
            step(&mut rt);
            let mut g = 0;
            rt.update(|s, _| g = s.motion.playback.as_ref().map_or(0, |p| p.generation));
            assert_eq!(g, g0, "round {round}: the parked clip is taken back");
            for _ in 0..30 {
                step(&mut rt);
            }
        }
        rt.update(|s, _| super::close(s));
        step(&mut rt);
        crate::clock::advance(crate::state::motion::CLOSING);
        step(&mut rt);
        let mut gone = false;
        rt.update(|s, _| gone = !s.motion.parked() && s.motion.playback.is_none());
        assert!(gone, "dropped once the slide is over");
    }

    /// In a window the Mac's clip counts as shown only once a frame of it is drawn: until then
    /// its layer is drawn nearly transparent (so Rune pulls that frame) and the still shows;
    /// then the clip fades in from a picture, never from an empty box.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_clip_fades_in_only_after_its_first_frame_is_drawn() {
        if std::env::var("UZUMAKI_MOTION").as_deref() == Ok("ffmpeg") {
            return;
        }
        let mut s = State::offline();
        super::scene_np_motion(&mut s, false);
        let album = s.playback.song.as_ref().and_then(|song| s.motion_album(song)).unwrap().to_string();
        let clip = s.motion.clips.values().find_map(crate::state::Load::ready).unwrap().clone();
        let p = crate::motion::Playback::start(album, 7, &clip, Box::new(|_, _| {}), true);
        assert!(!p.shown() && p.warming());
        s.motion.playback = Some(p);
        let mut rt = Runtime::new(s, crate::ui::view, (1360.0, 860.0), Env { system_dark: true, scale: 1.0 });
        let step = |rt: &mut Runtime<State>| {
            rt.update_if(|s, tx| crate::ui::poll(s, 0.0, tx));
            rt.frame(DT);
        };
        for _ in 0..20 {
            step(&mut rt);
            assert!(rt.presented("np-motion").is_none(), "no fade before a frame is drawn");
            let (_, o) = rt.presented("np-motion-warm").expect("drawn, to pull its first frame");
            assert!(o > 0.001 && o < 0.01, "invisible over the still: {o}");
        }
        rt.update_if(|s, _| s.motion_event(7, crate::motion::Event::FirstFrame));
        let mut seen = Vec::new();
        for _ in 0..40 {
            step(&mut rt);
            assert!(rt.presented("np-motion-warm").is_none());
            if let Some((_, o)) = rt.presented("np-motion") {
                seen.push(o);
            }
        }
        assert!(seen.iter().any(|o| *o > 0.05 && *o < 0.95), "a crossfade: {seen:?}");
        assert!((seen.last().unwrap() - 1.0).abs() < 1e-3);
    }

    /// The screencast's setup (`np-slide`), Now Playing closed, settled.
    fn slide_scene() -> Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "np-slide").unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, crate::ui::view, (1360.0, 860.0), Env { system_dark: true, scale: 1.0 });
        for _ in 0..30 {
            rt.frame(DT);
        }
        rt
    }

    /// The overlay's own presented fill (its base colour, not the tint or picture on it).
    fn np_fill(rt: &Runtime<State>) -> Option<rune::Color> {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("np") && !e.exiting).map(|e| e.fill)
    }

    /// 2026-11-09 screencast: with "Same as app", Now Playing waited about 230 ms for its own
    /// copy of the content area's picture, which landed mid-slide (and was uploaded then). Now it
    /// shows that very picture from its first frame.
    #[test]
    fn same_as_app_shows_the_app_picture_from_the_first_frame() {
        let mut rt = slide_scene();
        rt.update(|s, _| super::open(s));
        rt.frame(DT);
        assert!(rt.presented("np-bg").is_some(), "the picture is on the first frame of the slide");
        let (mut app, mut np) = (None, None);
        rt.update(|s, _| (app, np) = (s.backdrops.shown_id(Slot::App), s.backdrops.shown_id(Slot::NowPlaying)));
        assert!(app.is_some());
        assert_eq!(np, app, "Now Playing shares the content area's picture instead of rendering it again");
    }

    /// The same screencast: when the picture landed, the overlay's paint switched from the tint
    /// gradient to a fill that sprang up from clear, so the page flashed through the sliding
    /// overlay. The base stays opaque from the first frame, through a picture arriving late.
    #[test]
    fn the_overlay_base_stays_opaque_when_a_picture_arrives() {
        let mut rt = slide_scene();
        rt.update(|s, _| {
            s.settings.appearance.np = NpBg::Tint;
            super::open(s);
        });
        for i in 0..40 {
            rt.frame(DT);
            assert_eq!(np_fill(&rt).map(|c| c.a), Some(1.0), "tint, frame {i}");
        }
        assert!(rt.presented("np-tint").is_some());
        // The picture becomes available while the overlay is up (as a render landing would).
        rt.update(|s, _| s.settings.appearance.np = NpBg::App);
        for i in 0..40 {
            rt.frame(DT);
            assert_eq!(np_fill(&rt).map(|c| c.a), Some(1.0), "picture arriving, frame {i}");
        }
        assert!(rt.presented("np-bg").is_some());
        assert!(rt.presented("np-tint").is_none(), "the tint is gone once the picture is in");
    }
}
