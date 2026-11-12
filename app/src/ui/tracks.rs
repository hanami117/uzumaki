//! Song rows used by Search and Library Songs (36pt art square, title, artist · album, time).

use rune::{Cursor, El, col, el, icon, row, text};

use super::{icons, select};
use super::widgets::{Size, art, fmt_time};
use crate::models::Song;
use crate::state::State;
use crate::theme::*;

pub fn same_song(a: &Song, b: &Song) -> bool {
    a.id == b.id || a.play_id() == b.play_id()
}

pub fn is_current(s: &State, song: &Song) -> bool {
    s.playback.song.as_ref().is_some_and(|p| same_song(p, song))
}

/// Width of the star slot in song rows.
pub const STAR_W: f32 = 28.0;

/// The favourite star of a song row, as Music on the Mac shows it: filled and always visible when
/// starred, outline and only while the row is hovered when it can be, else an empty slot of the
/// same width so the columns line up. Pressing it toggles the star and nothing else (a child's own
/// `on_press` wins over the row's select and play handlers).
pub fn star_slot(s: &State, song: &Song, key: &str) -> El {
    let slot = el().w(STAR_W).h(STAR_W).center();
    let on = s.is_favourite(song);
    if !on && !s.can_favourite(song) {
        return slot;
    }
    let (glyph, color) = if on { (&icons::STAR_FILLED, PAL.accent) } else { (&icons::STAR, PAL.muted) };
    let song = song.clone();
    let btn = el().key(format!("{key}-star")).role("button").label(if on { "Undo Favourite" } else { "Favourite" })
        .size(STAR_W, STAR_W).radius(STAR_W / 2.0).center().cursor(Cursor::Pointer).hover(crate::theme::ov(0.08))
        .on_press(move |s: &mut State| s.toggle_favourite_songs(std::slice::from_ref(&song)))
        .child(icon(glyph, 14.0).color(color));
    slot.child(if on { btn } else { btn.group_opacity(0.0, 1.0) })
}

/// Row with a 36pt art square; selectable, double-click plays (see `select::wire`). `rk` is the
/// row's key in its list (`lists::row_key`).
pub fn song_row(s: &State, song: &Song, key: String, list: &str, rk: &str) -> El {
    let cur = is_current(s, song);
    let sub = match (song.artist.is_empty(), song.album.is_empty()) {
        (_, true) => song.artist.clone(),
        (true, false) => song.album.clone(),
        _ => format!("{} \u{b7} {}", song.artist, song.album),
    };
    let middle = [title_with_mark(song, cur), text(sub).font(BODY14).color(PAL.muted).truncate().grow(1.0).min_w(0.0)];
    row_with(s, song, key, list, rk, middle, 48.0)
}

/// `song_row` for an artist's top songs: the title with the album under it (the artist is the
/// page's), so narrow columns keep the title readable.
pub fn song_row_compact(s: &State, song: &Song, key: String, list: &str, rk: &str) -> El {
    let cur = is_current(s, song);
    let title = col().grow(1.0).min_w(0.0).gap(1.0)
        .child(title_with_mark(song, cur).grow(0.0).w_full())
        .child(text(song.album.clone()).font(SMALL13).color(PAL.muted).truncate());
    row_with(s, song, key, list, rk, [title], 54.0)
}

/// A song row: thumbnail (hover plays), `middle`, star, time.
fn row_with<const N: usize>(s: &State, song: &Song, key: String, list: &str, rk: &str, middle: [El; N], min_h: f32) -> El {
    let play_key = format!("{key}-play");
    let thumb = art(s, song.artwork.as_ref(), &song.album, Size::Fixed(36.0), 6.0, 0.0)
        .child(
            el().size(36.0, 36.0).radius(6.0).center().fill(rune::Color::BLACK.alpha(0.5)).group_opacity(0.0, 1.0)
                .child(icon(&icons::PLAY, 14.0).color(PAL.text)),
        );
    let thumb = select::play_button(el().key(play_key).child(thumb), list, rk);
    let star = star_slot(s, song, &key);
    let r = row().key(key).role("button").label(song.title.clone()).focusable(true).w_full().min_h(min_h).px(12.0).gap(12.0)
        .center_y().radius(8.0).hover(PAL.row_hover).hover_group().cursor(Cursor::Pointer)
        .child(el().w(44.0).child(thumb))
        .children(middle)
        .child(star)
        .child(row().w(56.0).justify_end().child(text(fmt_time(song.duration)).font(TIME14).color(PAL.muted)));
    select::wire(s, r, rk, list)
}

/// The title, followed by a small "local file" marker for songs from the music folders.
pub fn title_with_mark(song: &Song, cur: bool) -> El {
    let title = text(song.title.clone()).font(TRACK_TITLE).color(if cur { PAL.accent } else { PAL.text2 }).truncate().min_w(0.0).shrink();
    let mut r = row().grow(1.0).min_w(0.0).gap(6.0).center_y().child(title);
    if song.is_local() {
        r = r.child(icon(&icons::LOCAL_MARK, 14.0).color(PAL.dim));
    }
    r
}
