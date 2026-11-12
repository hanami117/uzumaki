//! The Search page: a big field, tabs (Apple Music | Library), and results. The Library tab
//! searches the loaded library as you type; the Apple Music tab asks the catalog on Enter.

use rune::{Cursor, El, GrowthSpec, ScrollSpec, TextStyle, Tx, col, el, icon, row, text, text_input};

use super::cards::artist_row;
use super::lib_search::{self, Found};
use super::tracks::song_row;
use super::widgets::{Grid, card, card_row, chip, heading, muted, small_button, status, subheading};
use super::{PAGE_PX, icons, lists};
use crate::models::Collection;
use crate::state::{Load, State};
use crate::theme::*;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Apple,
    Library,
    Local,
}

const TABS: [(Tab, &str, &str); 3] =
    [(Tab::Apple, "tab-apple", "Apple Music"), (Tab::Library, "tab-library", "Library"), (Tab::Local, "tab-local", "Local Files")];

/// Puts the cursor in the search field.
pub fn focus(s: &mut State) {
    s.ui.search.focused = true;
}

fn field(s: &State) -> El {
    let ph = match s.ui.search_tab {
        Tab::Library => "Search your library",
        Tab::Local => "Search your local files",
        Tab::Apple => "Search Apple Music",
    };
    let mut r = row().key("search-box").role("textbox").w_full().max_w(720.0).h(52.0).px(16.0).gap(12.0).center_y().radius(14.0)
        .fill(PAL.field).border(1.0, PAL.field_border).cursor(Cursor::Text)
        .child(icon(&icons::SEARCH, 20.0).color(PAL.dim))
        .child(super::caret::ink(
            text_input(&s.ui.search, TextStyle::new(18.0, 400.0))
                .key("search").grow(1.0).single_line().color(PAL.text).placeholder(ph, PAL.dim)
                .on_edit(|s: &mut State| &mut s.ui.search)
                // Tab out of the field: note where it lands (`keys::track_focus`).
                .on_input_key(|s: &mut State, ev: &rune::KeyEvent, tx: &mut Tx| super::keys::track_focus(s, ev, tx))
                .on_submit(|s: &mut State, tx: &mut Tx| {
                    if s.ui.search_tab == Tab::Apple {
                        let q = s.ui.search.text.clone();
                        s.search(&q);
                    }
                    tx.blur();
                }),
            s,
        ));
    if !s.ui.search.text.is_empty() {
        r = r.child(
            el().key("search-clear").role("button").label("Clear").focusable(true).size(26.0, 26.0).radius(13.0).center()
                .hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
                .on_press_tx(|s: &mut State, tx: &mut Tx| {
                    s.ui.search.clear();
                    tx.instant();
                })
                .child(icon(&icons::CLOSE, 13.0).color(PAL.muted)),
        );
    }
    r
}

fn tabs(s: &State) -> El {
    let mut r = row().gap(8.0);
    for (t, key, label) in TABS {
        r = r.child(chip(key.to_string(), label, t == s.ui.search_tab).on_press_tx(move |s: &mut State, tx: &mut Tx| {
            s.ui.search_tab = t;
            s.ui.sel.clear();
            tx.instant();
        }));
    }
    r
}

fn scroller(key: String, body: El) -> El {
    col().key(key).grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(body)
}

fn grid_section(s: &State, title: &str, items: &[Collection], grid: Grid) -> El {
    let mut sec = col().w_full().mt(20.0).child(subheading(title).mb(6.0));
    for r in 0..items.len().div_ceil(grid.n) {
        sec = sec.child(card_row(s, items, grid, r).mx(-8.0));
    }
    sec
}

/// Apple Music results (a short list: built in full).
fn apple_body(s: &State, grid: Grid) -> El {
    let mut body = col().w_full().px(PAGE_PX).pt(8.0).pb(40.0);
    if s.search_query.is_empty() {
        body = body.child(muted("Type and press Enter to search Apple Music."));
    } else {
        body = body.child(heading(format!("Results for \u{201c}{}\u{201d}", s.search_query)).mb(8.0));
        match &s.search {
            Load::Ready(r) if r.songs.is_empty() && r.albums.is_empty() && r.playlists.is_empty() => {
                body = body.child(status(format!("No results for \u{201c}{}\u{201d}.", s.search_query), false));
            }
            Load::Ready(r) => {
                if !r.songs.is_empty() {
                    let mut list = col().w_full().mx(-12.0);
                    for (i, song) in r.songs.iter().enumerate() {
                        list = list.child(col().w_full().py(1.0).child(song_row(s, song, format!("sr-{i}"), "search", &lists::row_key(s, i))));
                    }
                    body = body.child(col().w_full().mt(12.0).child(subheading("Songs").mb(6.0)).child(list));
                }
                if !r.albums.is_empty() {
                    body = body.child(grid_section(s, "Albums", &r.albums, grid));
                }
                if !r.playlists.is_empty() {
                    body = body.child(grid_section(s, "Playlists", &r.playlists, grid));
                }
            }
            Load::Failed(e) => body = body.child(status(format!("Search failed: {e}"), true)),
            _ => body = body.child(status("Searching\u{2026}", false)),
        }
    }
    scroller(format!("search-{}", s.search_seq), body)
}

// ---- Library tab ----

#[derive(Clone, Copy, PartialEq)]
enum Seg {
    Songs,
    SongsMore,
    Albums,
    Artists,
    Playlists,
    More,
}

/// How much of each group the page shows: a preview of the best matches, or everything
/// after "Show all".
#[derive(Clone, Copy)]
struct Counts {
    songs: usize,
    albums: usize,
    artists: usize,
    playlists: usize,
    more_songs: bool,
    more_groups: bool,
}

const PREVIEW_SONGS: usize = 8;
const PREVIEW_ARTISTS: usize = 5;

fn counts(f: &Found, g: Grid, all: bool) -> Counts {
    let pick = |n: usize, preview: usize| if all { n } else { n.min(preview) };
    let c = Counts {
        songs: pick(f.songs.len(), PREVIEW_SONGS),
        albums: pick(f.albums.len(), g.n),
        artists: pick(f.artists.len(), PREVIEW_ARTISTS),
        playlists: pick(f.playlists.len(), g.n),
        more_songs: false,
        more_groups: false,
    };
    Counts {
        more_songs: f.more_songs || c.songs < f.songs.len(),
        more_groups: f.more_groups || c.albums < f.albums.len() || c.artists < f.artists.len() || c.playlists < f.playlists.len(),
        ..c
    }
}

/// Row counts per group (header included), for the groups that have matches.
fn plan(c: &Counts, g: Grid) -> Vec<(Seg, usize)> {
    let mut v = Vec::new();
    if c.songs > 0 {
        v.push((Seg::Songs, 1 + c.songs));
        if c.more_songs {
            v.push((Seg::SongsMore, 1));
        }
    }
    if c.albums > 0 {
        v.push((Seg::Albums, 1 + c.albums.div_ceil(g.n)));
    }
    if c.artists > 0 {
        v.push((Seg::Artists, 1 + c.artists));
    }
    if c.playlists > 0 {
        v.push((Seg::Playlists, 1 + c.playlists.div_ceil(g.n)));
    }
    if c.more_groups {
        v.push((Seg::More, 1));
    }
    v
}

fn locate(plan: &[(Seg, usize)], mut i: usize) -> Option<(Seg, usize)> {
    for &(seg, n) in plan {
        if i < n {
            return Some((seg, i));
        }
        i -= n;
    }
    None
}

fn group_head(title: &str, n: usize, more: bool) -> El {
    let count = if more { format!("{n}+") } else { n.to_string() };
    row().w_full().px(PAGE_PX).pt(18.0).pb(6.0).gap(8.0).center_y()
        .child(subheading(title.to_string()))
        .child(text(count).font(SMALL13).color(PAL.dim))
}

fn show_all(msg: &str) -> El {
    row().w_full().px(PAGE_PX).py(10.0).gap(12.0).center_y()
        .child(text(msg.to_string()).font(SMALL13).color(PAL.dim))
        .child(small_button("lib-show-all", "Show all").on_press_tx(|s: &mut State, tx: &mut Tx| {
            s.ui.lib_show_all = true;
            tx.instant();
        }))
}

fn card_line(s: &State, items: &[usize], list: &[Collection], g: Grid, line: usize) -> El {
    let mut r = row().w_full().gap(12.0).py(6.0).items_start();
    for &k in items.iter().skip(line * g.n).take(g.n) {
        if let Some(c) = list.get(k) {
            r = r.child(card(s, c, g.w));
        }
    }
    row().w_full().px(PAGE_PX - 8.0).child(r)
}

fn lib_row(s: &State, f: &Found, c: &Counts, plan: &[(Seg, usize)], g: Grid, i: usize) -> El {
    let Some((seg, k)) = locate(plan, i) else { return col().h(1.0) };
    match (seg, k) {
        (Seg::Songs, 0) => group_head("Songs", c.songs, c.more_songs),
        (Seg::Songs, k) => match f.songs.get(k - 1).filter(|_| k <= c.songs) {
            Some(song) => row().w_full().px(PAGE_PX - 12.0).py(1.0).child(song_row(s, song, format!("ls-{k}-{}", song.id), "search", &lists::row_key(s, k - 1))),
            None => col().h(1.0),
        },
        (Seg::SongsMore, _) => show_all("More songs match."),
        (Seg::Albums, 0) => group_head("Albums", c.albums, f.albums.len() > c.albums),
        (Seg::Albums, k) => card_line(s, &f.albums[..c.albums], &s.library_albums.items, g, k - 1),
        (Seg::Artists, 0) => group_head("Artists", c.artists, f.artists.len() > c.artists),
        (Seg::Artists, k) => match f.artists.get(k - 1).filter(|_| k <= c.artists).and_then(|&a| s.library_artists.items.get(a)) {
            Some(a) => row().w_full().px(PAGE_PX - 12.0).py(2.0).child(artist_row(format!("la-{}", a.id), a)),
            None => col().h(1.0),
        },
        (Seg::Playlists, 0) => group_head("Playlists", c.playlists, f.playlists.len() > c.playlists),
        (Seg::Playlists, k) => card_line(s, &f.playlists[..c.playlists], &s.library_playlists.items, g, k - 1),
        (Seg::More, _) => show_all("More albums, artists or playlists match."),
    }
}

fn library_body(s: &State, g: Grid) -> El {
    let Some(found) = lib_search::find(s) else {
        let l = |n: usize, what: &str| format!("{n} {what}");
        let counts = [
            l(s.library_songs.items.len(), "songs"),
            l(s.library_albums.items.len(), "albums"),
            l(s.library_artists.items.len(), "artists"),
            l(s.library_playlists.items.len(), "playlists"),
        ];
        return scroller(
            "search-lib-hint".into(),
            col().w_full().px(PAGE_PX).pt(8.0).gap(4.0)
                .child(muted("Type to search your songs, albums, artists and playlists."))
                .child(text(format!("In your library: {}", counts.join(" \u{b7} "))).font(SMALL13).color(PAL.dim)),
        );
    };
    if found.is_empty() {
        let loading = [&s.library_songs.load, &s.library_albums.load].iter().any(|l| matches!(l, Load::Loading));
        let msg = if loading { "Your library is still loading\u{2026}".to_string() } else { format!("No matches in your library for \u{201c}{}\u{201d}.", s.ui.search.text.trim()) };
        return scroller("search-lib-none".into(), col().w_full().px(PAGE_PX).child(status(msg, false)));
    }
    let cn = counts(&found, g, s.ui.lib_show_all);
    let pl = plan(&cn, g);
    let count: usize = pl.iter().map(|p| p.1).sum();
    let cur = s.playback.song.as_ref().map(|x| x.id.clone());
    let stars = s.favourites_rev();
    let rows = col().key("rows").growth(GrowthSpec::default())
        .virtual_rows(
            count,
            50.0,
            {
                let (pl, f) = (pl.clone(), found.clone());
                move |_: &State, i| match locate(&pl, i) {
                    Some((Seg::Songs, k)) if k > 0 => format!("s{k}-{}", f.songs.get(k - 1).map_or("", |x| x.id.as_str())).into(),
                    Some((seg, k)) => format!("{}-{k}", seg as u8).into(),
                    None => format!("x{i}").into(),
                }
            },
            move |s: &State, i| lib_row(s, &found, &cn, &pl, g, i),
        )
        .virtual_rows_version((count, s.ui.search.text.clone(), s.ui.sel.rev, cur, g.n, s.ui.lib_show_all, stars));
    scroller("search-lib".into(), rows)
}

/// Local Files tab: matching local songs, one virtual list (group header, rows, "Show all").
fn local_body(s: &State) -> El {
    let Some(found) = lib_search::find_local(s) else {
        let n = s.local_lib.songs.len();
        let msg = if s.settings.music_folders.is_empty() {
            "Add a music folder in Settings to search your own files.".to_string()
        } else {
            "Type to search your local files.".to_string()
        };
        return scroller(
            "search-local-hint".into(),
            col().w_full().px(PAGE_PX).pt(8.0).gap(4.0).child(muted(msg))
                .child(text(format!("In your local files: {n} songs")).font(SMALL13).color(PAL.dim)),
        );
    };
    if found.songs.is_empty() {
        let msg = if s.local_lib.scanning && s.local_lib.songs.is_empty() {
            "Your music folders are still being scanned\u{2026}".to_string()
        } else {
            format!("No matches in your local files for \u{201c}{}\u{201d}.", s.ui.search.text.trim())
        };
        return scroller("search-local-none".into(), col().w_full().px(PAGE_PX).child(status(msg, false)));
    }
    let n = found.songs.len();
    let more = found.more && !s.ui.lib_show_all;
    let count = 1 + n + usize::from(more);
    let cur = s.playback.song.as_ref().map(|x| x.id.clone());
    let rows = col().key("rows").growth(GrowthSpec::default())
        .virtual_rows(
            count,
            50.0,
            {
                let f = found.clone();
                move |_: &State, i| match i {
                    0 => "head".into(),
                    i if i <= f.songs.len() => format!("s{i}-{}", f.songs[i - 1].id).into(),
                    _ => "more".into(),
                }
            },
            {
                let f = found.clone();
                move |s: &State, i| match i {
                    0 => group_head("Songs", f.songs.len(), f.more && !s.ui.lib_show_all),
                    i if i <= f.songs.len() => {
                        let song = &f.songs[i - 1];
                        row().w_full().px(PAGE_PX - 12.0).py(1.0).child(song_row(s, song, format!("lo-{i}-{}", song.id), "search", &lists::row_key(s, i - 1)))
                    }
                    _ => show_all("More songs match."),
                }
            },
        )
        .virtual_rows_version((count, s.ui.search.text.clone(), s.ui.sel.rev, cur, s.ui.lib_show_all));
    scroller("search-local".into(), rows)
}

pub fn page(s: &State, grid: Grid) -> El {
    let mut head = col().w_full().px(PAGE_PX).pt(28.0).pb(14.0).gap(14.0).child(field(s));
    // Local only mode searches the music folders only (`search_tab` is Local): no tabs.
    if !s.local_only() {
        head = head.child(tabs(s));
    }
    let body = match s.ui.search_tab {
        Tab::Apple => apple_body(s, grid),
        Tab::Library => library_body(s, grid),
        Tab::Local => local_body(s),
    };
    col().key("search-page").grow(1.0).min_h(0.0).w_full().child(head).child(body)
}
