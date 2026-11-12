//! Virtualised card grids (each virtual row is a row of cards) over a state list, plus the pages
//! that are just a grid: Recently Added and Artist.

use std::rc::Rc;

use rune::{El, GrowthSpec, col, row};

use super::browse::{not_ready, scroll_page};
use super::ctx_menu;
use super::widgets::{Grid, card, heading, muted, status};
use super::PAGE_PX;
use crate::models::{Artist, Collection};
use crate::state::{Load, State};

/// Which state list a grid shows.
#[derive(Clone)]
pub enum GridSrc {
    Albums,
    /// Library playlists, Favourite Songs first.
    Playlists,
    Recent,
    Artist(String),
    /// A shelf of a catalog artist page (See All).
    ArtistShelf(String, crate::artist::Shelf),
    /// The music folders' albums (local only mode).
    LocalAlbums,
}

impl GridSrc {
    pub fn items<'a>(&self, s: &'a State) -> &'a [Collection] {
        match self {
            GridSrc::Albums => &s.library_albums.items,
            GridSrc::Playlists => &s.library_playlists.items,
            GridSrc::Recent => &s.recently_added.items,
            GridSrc::Artist(id) => s.artist_albums.get(id).and_then(Load::ready).map_or(&[], |v| v.as_slice()),
            GridSrc::ArtistShelf(id, shelf) => match s.artist_pages.get(id) {
                Some(Load::Ready(Some(p))) => &p.section(*shelf).items,
                _ => &[],
            },
            GridSrc::LocalAlbums => &s.local_lib.view.albums,
        }
    }

    /// Item `k` in display order.
    fn nth<'a>(&self, s: &'a State, k: usize) -> Option<&'a Collection> {
        let items = self.items(s);
        if let GridSrc::Playlists = self {
            if let Some(f) = items.iter().position(|c| c.favorites) {
                return match k {
                    0 => items.get(f),
                    k if k <= f => items.get(k - 1),
                    k => items.get(k),
                };
            }
        }
        items.get(k)
    }
}

/// `shown` lists indices into the source list (a filtered view). `tag` goes into the key version: change
/// it when the shown items change without changing their number.
pub fn grid(s: &State, key: String, src: GridSrc, g: Grid, loading: bool, shown: Option<Rc<Vec<usize>>>, tag: String) -> El {
    let n_items = shown.as_ref().map_or_else(|| src.items(s).len(), |v| v.len());
    let count = n_items.div_ceil(g.n) + usize::from(loading && shown.is_none());
    let first = shown.as_ref().map_or(0, |v| v.first().copied().unwrap_or(0));
    scroll_page(
        &key,
        col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(
                count,
                g.row_height(),
                move |_: &State, i| format!("r{i}").into(),
                move |s: &State, i| {
                    let mut r = row().w_full().gap(12.0).py(6.0).items_start();
                    let mut any = false;
                    for k in i * g.n..(i + 1) * g.n {
                        let c = match &shown {
                            Some(v) => v.get(k).and_then(|&j| src.items(s).get(j)),
                            None => src.nth(s, k),
                        };
                        let Some(c) = c else { break };
                        any = true;
                        r = r.child(card(s, c, g.w));
                    }
                    if !any {
                        return row().w_full().px(PAGE_PX).child(status("Loading more\u{2026}", false));
                    }
                    row().w_full().px(PAGE_PX - 8.0).child(r)
                },
            )
            .virtual_rows_version((n_items, g.n, g.w as u32, loading, tag, first)),
    )
}

pub fn recently_added_page(s: &State, g: Grid) -> El {
    let head = col().w_full().px(PAGE_PX).pt(28.0).pb(16.0).gap(4.0)
        .child(heading("Recently Added"))
        .child(muted("Albums and playlists newest in your library"));
    let ra = &s.recently_added;
    let body = if ra.items.is_empty() {
        col().w_full().px(PAGE_PX).child(match &ra.load {
            Load::Ready(_) => status("Nothing added recently.", false),
            other => not_ready(s, other, "recently added items"),
        })
    } else {
        grid(s, "recent".into(), GridSrc::Recent, g, matches!(ra.load, Load::Loading), None, String::new())
    };
    col().key("recently-added").grow(1.0).min_h(0.0).w_full().child(head).child(body)
}

pub fn artist_page(s: &State, id: &str, name: &str, g: Grid) -> El {
    let a = Artist { id: id.to_string(), name: name.to_string() };
    let pin = ctx_menu::more_button("artist-more", 34.0, move |s| ctx_menu::artist_items(s, &a));
    let head = row().w_full().px(PAGE_PX).pt(28.0).pb(16.0).gap(14.0).center_y()
        .child(heading(name.to_string()).min_w(0.0).truncate())
        .child(pin);
    let load = s.artist_albums.get(id);
    let body = match load {
        Some(Load::Ready(v)) if !v.is_empty() => {
            grid(s, format!("artist-{id}"), GridSrc::Artist(id.to_string()), g, false, None, String::new())
        }
        Some(Load::Ready(_)) => col().w_full().px(PAGE_PX).child(status("No albums found for this artist.", false)),
        Some(other) => col().w_full().px(PAGE_PX).child(not_ready(s, other, "this artist")),
        None => col().w_full().px(PAGE_PX).child(status("Loading\u{2026}", false)),
    };
    col().key(format!("artist-page-{id}")).grow(1.0).min_h(0.0).w_full().child(head).child(body)
}
