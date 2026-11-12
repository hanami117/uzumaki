//! Browse pages made of shelves: New, Made for You, and Radio.

use rune::{Cursor, El, ScrollSpec, col, row, text};

use super::cards::{hero_card, station_card};
use super::carousel::carousel;
use super::widgets::{Size, art, card, fmt_time, heading, muted, status};
use super::PAGE_PX;
use crate::models::{Shelf, Song, Station};
use crate::state::{Load, State};
use crate::theme::*;

const CARD_W: f32 = 176.0;
const SONG_COL_W: f32 = 300.0;
const SONG_ROWS: usize = 4;
/// Cards built per shelf (the rest stay off the page: shelves are editorial, rarely longer).
const MAX_CARDS: usize = 30;

#[derive(Clone, Copy)]
pub enum Source {
    New,
    MadeForYou,
}

fn shelves(s: &State, src: Source) -> &Load<Vec<Shelf>> {
    match src {
        Source::New => &s.browse_new,
        Source::MadeForYou => &s.made_for_you,
    }
}

pub fn scroll_page(key: &str, body: El) -> El {
    col().key(key.to_string()).grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(body)
}

pub fn body() -> El {
    col().w_full().px(PAGE_PX).pt(28.0).pb(40.0)
}

/// Loading / Failed / signed-out / empty text for a `Load` that isn't ready.
pub fn not_ready<T>(s: &State, load: &Load<T>, what: &str) -> El {
    match load {
        Load::Loading => status("Loading\u{2026}", false),
        Load::Failed(e) => status(format!("Could not load {what}: {e}"), true),
        _ if !s.session.authorized => status(format!("Sign in to see {what}."), false),
        _ => status(format!("No {what} to show."), false),
    }
}

/// One song row of a song shelf: 44pt cover, title, artist; click plays the shelf from it.
fn song_cell(s: &State, src: Source, shelf: usize, i: usize, song: &Song, title: &str) -> El {
    let title = title.to_string();
    row().key(format!("sg-{shelf}-{i}")).role("button").label(song.title.clone()).focusable(true).w_full().h(56.0).px(6.0)
        .gap(12.0).center_y().radius(8.0).hover(PAL.row_hover).press(crate::theme::ov(0.04)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| {
            let songs = match shelves(s, src).ready().and_then(|v| v.get(shelf)) {
                Some(sh) => sh.songs.clone(),
                None => return,
            };
            s.play_list(&songs, i, &title);
        })
        .child(art(s, song.artwork.as_ref(), &song.album, Size::Small(44.0), 6.0, 0.0))
        .child(
            col().grow(1.0).min_w(0.0).gap(1.0)
                .child(text(song.title.clone()).font(TRACK_TITLE).color(PAL.text2).truncate())
                .child(text(song.artist.clone()).font(SMALL13).color(PAL.muted).truncate()),
        )
        .child(text(fmt_time(song.duration)).font(SMALL12).color(PAL.dim))
}

/// Column `c` of a song shelf: up to `SONG_ROWS` songs.
fn song_column(s: &State, src: Source, shelf: usize, sh: &Shelf, c: usize) -> El {
    let chunk = sh.songs.chunks(SONG_ROWS).nth(c).unwrap_or_default();
    col().w(SONG_COL_W).children(
        chunk.iter().enumerate().map(|(k, song)| song_cell(s, src, shelf, c * SONG_ROWS + k, song, &sh.title)),
    )
}

fn shelf_view(s: &State, page: &str, src: Source, idx: usize, sh: &Shelf) -> El {
    let mut sec = col().w_full();
    let n_cards = (sh.items.len() + sh.stations.len()).min(MAX_CARDS);
    let mut titled = false;
    if n_cards > 0 {
        let card_at = |i: usize| match sh.items.get(i) {
            Some(c) => card(s, c, CARD_W),
            None => station_card(s, &sh.stations[i - sh.items.len()], CARD_W),
        };
        sec = sec.child(carousel(s, &format!("{page}-{idx}"), &sh.title, n_cards, card_at));
        titled = true;
    }
    if !sh.songs.is_empty() {
        let t = if titled { "" } else { sh.title.as_str() };
        let n_cols = sh.songs.len().div_ceil(SONG_ROWS).min(MAX_CARDS);
        sec = sec.child(carousel(s, &format!("{page}-{idx}-songs"), t, n_cols, |c| song_column(s, src, idx, sh, c)));
    }
    sec
}

fn shelves_page(s: &State, key: &'static str, title: &str, sub: &str, src: Source) -> El {
    let mut b = body().child(heading(title.to_string()).mb(4.0)).child(muted(sub.to_string()).mb(20.0));
    match shelves(s, src) {
        Load::Ready(list) if list.is_empty() => b = b.child(status("Nothing here yet.", false)),
        Load::Ready(list) => {
            for (i, sh) in list.iter().enumerate() {
                b = b.child(shelf_view(s, key, src, i, sh));
            }
        }
        other => b = b.child(not_ready(s, other, title)),
    }
    scroll_page(key, b)
}

pub fn new_page(s: &State) -> El {
    shelves_page(s, "new", "New", "Fresh from Apple Music", Source::New)
}

pub fn made_for_you_page(s: &State) -> El {
    shelves_page(s, "made-for-you", "Made for You", "Mixes and stations picked for you", Source::MadeForYou)
}

fn stations(l: &Load<Vec<Shelf>>) -> Vec<&Station> {
    l.ready().map(|v| v.iter().flat_map(|sh| sh.stations.iter()).collect()).unwrap_or_default()
}

pub fn radio_page(s: &State, avail: f32) -> El {
    let mut b = body().child(heading("Radio").mb(4.0)).child(muted("Live stations and your own").mb(20.0));
    let mut live = stations(&s.radio[0]);
    // Apple Music 1 leads (stable, so the rest keep the API's order).
    live.sort_by_key(|st| !st.name.contains("Apple Music 1"));
    match &s.radio[0] {
        Load::Ready(_) if !live.is_empty() => {
            let n = (((avail + 14.0) / 294.0).floor() as usize).clamp(1, 4);
            let w = ((avail - 14.0 * (n as f32 - 1.0)) / n as f32).floor();
            let mut grid = col().w_full().gap(14.0).mb(26.0);
            for chunk in live.chunks(n) {
                grid = grid.child(row().w_full().gap(14.0).children(chunk.iter().map(|st| hero_card(s, st, w))));
            }
            b = b.child(grid);
        }
        other => b = b.child(not_ready(s, other, "live stations")),
    }
    let mine = stations(&s.radio[1]);
    if !mine.is_empty() {
        b = b.child(super::widgets::subheading("Your station").mb(8.0)).child(
            row().w_full().gap(12.0).flex_wrap().mx(-8.0).children(mine.iter().map(|st| station_card(s, st, CARD_W))),
        );
    }
    scroll_page("radio", b)
}
