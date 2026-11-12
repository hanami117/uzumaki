//! Library: chips + albums / playlists grids or the songs list, all virtualised.

use rune::{El, GrowthSpec, ScrollSpec, col, row};

use super::{PAGE_PX, filter, lists};
use super::songs_table::{self as table, Which};
use super::grids::{self, GridSrc};
use super::widgets::{Grid, heading, status};
use crate::state::{LibraryTab, Load, State};

fn scroller(key: String, rows: El) -> El {
    col().key(key).grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(rows)
}

/// What to show instead of the list, if anything.
pub(super) fn placeholder<T>(s: &State, items: &[T], load: &Load<()>, what: &str) -> Option<El> {
    if !items.is_empty() {
        return None;
    }
    Some(col().w_full().px(PAGE_PX).child(match load {
        Load::Loading => status("Loading\u{2026}", false),
        Load::Failed(e) => status(format!("Could not load your {what}: {e}"), true),
        _ if !s.session.authorized => status(format!("Sign in to see your {what}."), false),
        _ => status(format!("No {what} in your library."), false),
    }))
}

fn grid(s: &State, tab: LibraryTab, g: Grid, loading: bool) -> El {
    let shown = (tab == LibraryTab::Albums).then(|| lists::album_matches(s)).flatten();
    let src = if tab == LibraryTab::Playlists { GridSrc::Playlists } else { GridSrc::Albums };
    grids::grid(s, format!("lib-{tab:?}"), src, g, loading, shown, lists::query(s))
}

/// Library > Songs: the sortable table (fixed header, virtual rows).
fn songs(s: &State, avail: f32) -> El {
    let loading = matches!(s.library_songs.load, Load::Loading);
    let items = &s.library_songs.items;
    let order = lists::order(s);
    let count = order.len() + usize::from(loading);
    let sort = s.ui.table.lib_sort;
    let cols = s.ui.table.cols(avail);
    let first = items.first().map(|x| x.id.clone()).unwrap_or_default();
    let query = lists::query(s);
    let cur_id = s.playback.song.as_ref().map(|x| x.id.clone());
    let rows = scroller(
        "lib-songs".into(),
        col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(
                count,
                50.0,
                |_: &State, i| format!("r{i}").into(),
                move |s: &State, i| {
                    let items = &s.library_songs.items;
                    let order = lists::order(s);
                    match order.get(i).and_then(|&k| items.get(k).map(|x| (k, x))) {
                        Some((k, song)) => row().w_full().px(PAGE_PX - 12.0).py(1.0).child(table::song_row(
                            s,
                            song,
                            i + 1,
                            cols,
                            format!("song-{i}-{}", song.id),
                            "lib-songs",
                            &lists::row_key(s, k),
                        )),
                        None if order.is_empty() && !s.library_songs.items.is_empty() => {
                            row().w_full().px(PAGE_PX).child(status("No songs match your filter.", false))
                        }
                        None => row().w_full().px(PAGE_PX).child(status("Loading more\u{2026}", false)),
                    }
                },
            )
            .virtual_rows_version((count, sort, cols, first, query, s.ui.sel.rev, cur_id, s.favourites_rev())),
    );
    col().w_full().grow(1.0).min_h(0.0).child(table::head(Which::Library, sort, cols)).child(rows)
}

pub fn page(s: &State, tab: LibraryTab, g: Grid, avail: f32) -> El {
    let title = match tab {
        LibraryTab::Albums => "Albums",
        LibraryTab::Playlists => "All Playlists",
        LibraryTab::Songs => "Songs",
    };
    let head = col().w_full().px(PAGE_PX).pt(28.0).pb(16.0).child(heading(title));

    let body = match tab {
        LibraryTab::Albums => match placeholder(s, &s.library_albums.items, &s.library_albums.load, "albums") {
            Some(p) => p,
            None => grid(s, tab, g, matches!(s.library_albums.load, Load::Loading)),
        },
        LibraryTab::Playlists => match placeholder(s, &s.library_playlists.items, &s.library_playlists.load, "playlists") {
            Some(p) => p,
            None => grid(s, tab, g, matches!(s.library_playlists.load, Load::Loading)),
        },
        LibraryTab::Songs => match placeholder(s, &s.library_songs.items, &s.library_songs.load, "songs") {
            Some(p) => p,
            None => songs(s, avail),
        },
    };
    let mut page = col().key("library").grow(1.0).min_h(0.0).w_full().child(head);
    let bar = match tab {
        LibraryTab::Songs => filter::bar(s, lists::order(s).len(), s.library_songs.items.len()),
        LibraryTab::Albums => {
            let n = lists::album_matches(s).map_or(s.library_albums.items.len(), |m| m.len());
            filter::bar(s, n, s.library_albums.items.len())
        }
        LibraryTab::Playlists => None,
    };
    page = page.child(bar.unwrap_or_else(col));
    page.child(body)
}
