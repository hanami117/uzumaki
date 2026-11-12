//! The song list of the current page: its identity, the songs in display order (sorted, then
//! filtered) and the filter field's state. Selection, menus and play-from-row all go through
//! here, so they agree with what the rows show.

use std::cell::RefCell;
use std::rc::Rc;

use rune::TextEdit;

use super::search::Tab as SearchTab;
use super::songs_table::{Sort, Which, fold};
use crate::models::{CollectionKind, Song};
use crate::state::{LibraryTab, Load, Route, State};

/// The Ctrl+F filter field. Only applies on the list it was opened for (`list`).
#[derive(Default)]
pub struct Filter {
    pub open: bool,
    pub list: String,
    pub edit: TextEdit,
}

type OrderKey = (String, String, usize, String, Option<Sort>);

/// Last computed display order (one list is on screen at a time).
#[derive(Default)]
pub struct OrderCache(RefCell<Option<(OrderKey, Rc<Vec<usize>>)>>);

type RowKeysKey = (String, usize, String, String);

/// Last computed row keys of the page's list, keyed by (list, length, first and last song id).
#[derive(Default)]
pub struct RowKeysCache(RefCell<Option<(RowKeysKey, Rc<Vec<String>>)>>);

/// Last filtered Library > Albums, keyed by (filter, list length, first album id).
#[derive(Default)]
pub struct AlbumCache(RefCell<Option<((String, usize, String), Rc<Vec<usize>>)>>);

/// The albums grid of this page: the library's, or the music folders' (local only mode).
pub fn album_items(s: &State) -> &[crate::models::Collection] {
    if s.route == Route::LocalAlbums { &s.local_lib.view.albums } else { &s.library_albums.items }
}

/// A local artist's or folder's page: its songs are `LocalLibrary::page`.
fn local_group(route: &Route) -> bool {
    match route {
        Route::Artist { id, .. } => crate::local::view::is_artist_id(id),
        Route::Folders { path } => path.is_some(),
        _ => false,
    }
}

/// Indices of the grid's albums that match the Ctrl+F filter; None when no filter applies.
pub fn album_matches(s: &State) -> Option<Rc<Vec<usize>>> {
    let q = query(s);
    if q.is_empty() {
        return None;
    }
    let items = album_items(s);
    let key = (q.clone(), items.len(), items.first().map(|c| c.id.clone()).unwrap_or_default());
    if let Some((k, v)) = s.ui.albums.0.borrow().as_ref() {
        if *k == key {
            return Some(v.clone());
        }
    }
    let v: Rc<Vec<usize>> = Rc::new((0..items.len()).filter(|&i| matches(&q, [&items[i].title, &items[i].subtitle, ""])).collect());
    *s.ui.albums.0.borrow_mut() = Some((key, v.clone()));
    Some(v)
}

/// Identity of the page's song list (selection and play-from-row), if the page has one.
pub fn list_id(s: &State) -> Option<String> {
    match &s.route {
        Route::Collection { id, .. } => Some(format!("coll:{id}")),
        Route::Library(LibraryTab::Songs) => Some("lib-songs".into()),
        Route::Search => Some("search".into()),
        Route::LocalFiles => Some("local-files".into()),
        Route::Artist { id, .. } if local_group(&s.route) => Some(format!("artist:{id}")),
        // A catalog artist page: its top songs.
        Route::Artist { id, .. } => super::artist::top_songs(s, id).map(|_| super::artist::list_id(id)),
        Route::Folders { path: Some(p) } => Some(format!("folder:{}", p.display())),
        _ => None,
    }
}

/// Key of the page's scroll view, for Home/End in its song list.
pub fn scroll_key(s: &State) -> Option<String> {
    match &s.route {
        Route::Collection { id, .. } => Some(format!("coll-{id}")),
        Route::Library(LibraryTab::Songs) => Some("lib-songs".into()),
        Route::LocalFiles => Some("local-songs".into()),
        Route::Artist { id, .. } if local_group(&s.route) => Some(format!("local-artist-{id}-scroll")),
        Route::Folders { path: Some(p) } => Some(format!("folder-{}-scroll", p.display())),
        Route::Search => Some(format!("search-{}", s.search_seq)),
        _ => None,
    }
}

/// Identity of a page that Ctrl+F can filter (song lists and the albums grid).
pub fn filter_list_id(s: &State) -> Option<String> {
    match &s.route {
        Route::Library(LibraryTab::Albums) => Some("lib-albums".into()),
        Route::LocalAlbums => Some("local-albums".into()),
        Route::Search => None,
        // Ten or twenty top songs: nothing to filter.
        Route::Artist { .. } if !local_group(&s.route) => None,
        _ => list_id(s),
    }
}

/// The filter field is showing on this page.
pub fn filter_active(s: &State) -> bool {
    let f = &s.ui.filter;
    f.open && filter_list_id(s).is_some_and(|id| id == f.list)
}

/// The folded filter text, or "" when no filter applies on this page.
pub fn query(s: &State) -> String {
    if filter_active(s) { fold(&s.ui.filter.edit.text) } else { String::new() }
}

pub fn matches(query: &str, fields: [&str; 3]) -> bool {
    query.is_empty() || fields.iter().any(|f| fold(f).contains(query))
}

/// The page's songs: borrowed from state, or the cached library-search matches.
pub enum Songs<'a> {
    Slice(&'a [Song]),
    Shared(Rc<Vec<Song>>),
}

impl std::ops::Deref for Songs<'_> {
    type Target = [Song];
    fn deref(&self) -> &[Song] {
        match self {
            Songs::Slice(v) => v,
            Songs::Shared(v) => v,
        }
    }
}

/// The page's songs, in the list's own order.
pub fn songs(s: &State) -> Songs<'_> {
    Songs::Slice(match &s.route {
        Route::Collection { id, .. } => s.collections.get(id).and_then(Load::ready).map_or(&[], |c| &c.tracks),
        Route::Library(LibraryTab::Songs) => &s.library_songs.items,
        Route::LocalFiles => &s.local_lib.songs,
        r if local_group(r) => s.local_lib.page_songs(r),
        Route::Artist { id, .. } => super::artist::top_songs(s, id).unwrap_or(&[]),
        Route::Search if s.ui.search_tab == SearchTab::Library => {
            return Songs::Shared(super::lib_search::find(s).map_or_else(Default::default, |f| f.songs.clone()));
        }
        Route::Search if s.ui.search_tab == SearchTab::Local => {
            return Songs::Shared(super::lib_search::find_local(s).map_or_else(Default::default, |f| f.songs.clone()));
        }
        Route::Search => match &s.search {
            Load::Ready(r) => &r.songs,
            _ => &[],
        },
        _ => &[],
    })
}

/// What "Continue playing from" calls this list.
pub fn name(s: &State) -> String {
    match &s.route {
        Route::Collection { id, .. } => s.collections.get(id).and_then(Load::ready).map(|c| c.title.clone()).unwrap_or_default(),
        Route::Library(_) => "Library".into(),
        Route::LocalFiles => "Local Files".into(),
        Route::Artist { name, .. } => name.clone(),
        Route::Folders { path: Some(p) } => p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()),
        Route::Search if s.ui.search_tab == SearchTab::Local => "Local Files".into(),
        _ => String::new(),
    }
}

/// Which sorted table (if any) orders this page's songs.
fn sorted_by(s: &State) -> Option<Which> {
    match &s.route {
        Route::Library(LibraryTab::Songs) => Some(Which::Library),
        Route::LocalFiles => Some(Which::Local),
        r if local_group(r) => Some(Which::Group),
        Route::Collection { kind: CollectionKind::Playlist, .. } if s.ui.table.songs_view => Some(Which::Playlist),
        _ => None,
    }
}

/// Indices into `songs(s)` in display order: sorted, then filtered. Cached.
pub fn order(s: &State) -> Rc<Vec<usize>> {
    let list = songs(s);
    let q = query(s);
    let which = sorted_by(s);
    let first = list.first().map(|x| x.id.clone()).unwrap_or_default();
    let key: OrderKey = (list_id(s).unwrap_or_default(), q.clone(), list.len(), first.clone(), which.map(|w| s.ui.table.sort(w)));
    if let Some((k, o)) = s.ui.order.0.borrow().as_ref() {
        if *k == key {
            return o.clone();
        }
    }
    let base: Vec<usize> = match which {
        Some(w) => (*s.ui.table.order(w, if w == Which::Playlist { &key.0 } else { &first }, &list)).clone(),
        None => (0..list.len()).collect(),
    };
    let out: Vec<usize> = if q.is_empty() {
        base
    } else {
        base.into_iter().filter(|&i| matches(&q, [&list[i].title, &list[i].artist, &list[i].album])).collect()
    };
    let out = Rc::new(out);
    *s.ui.order.0.borrow_mut() = Some((key, out.clone()));
    out
}

/// The page's songs as displayed.
pub fn visible(s: &State) -> Vec<Song> {
    let list = songs(s);
    order(s).iter().filter_map(|&i| list.get(i)).cloned().collect()
}

/// The page's rows as displayed: each row's key (`row_keys`) and its song.
pub fn visible_rows(s: &State) -> Vec<(String, Song)> {
    visible_rows_at(s).into_iter().map(|(_, k, x)| (k, x)).collect()
}

/// `visible_rows` with each row's position in the list's own order (Apple's order for a
/// playlist), whatever the sort and filter: what Remove from Playlist needs to drop one copy.
pub fn visible_rows_at(s: &State) -> Vec<(usize, String, Song)> {
    let list = songs(s);
    let keys = row_keys(s);
    order(s).iter().filter_map(|&i| Some((i, keys.get(i)?.clone(), list.get(i)?.clone()))).collect()
}

/// What identifies a row of a song list: the song's id, plus a count for each repeat of it
/// (a playlist can hold a song twice). Selection, menus, drags and play-from-row all go by row
/// key, so an action on one copy never reaches the other. Keys follow the list's own order, so
/// sorting and filtering don't change them. The first copy's key is the bare id.
pub fn keys_of(list: &[Song]) -> Vec<String> {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    list.iter()
        .map(|x| {
            let n = seen.entry(x.id.as_str()).or_insert(0);
            *n += 1;
            if *n == 1 { x.id.clone() } else { format!("{}\u{1f}{}", x.id, *n) }
        })
        .collect()
}

/// Row keys of the page's list (`keys_of`), in its own order. Cached.
pub fn row_keys(s: &State) -> Rc<Vec<String>> {
    let list = songs(s);
    let id = |x: Option<&Song>| x.map(|x| x.id.clone()).unwrap_or_default();
    let key: RowKeysKey = (list_id(s).unwrap_or_default(), list.len(), id(list.first()), id(list.last()));
    if let Some((k, v)) = s.ui.row_keys.0.borrow().as_ref() {
        if *k == key {
            return v.clone();
        }
    }
    let v = Rc::new(keys_of(&list));
    *s.ui.row_keys.0.borrow_mut() = Some((key, v.clone()));
    v
}

/// Key of row `k` of the page's list (`k` counts in the list's own order).
pub fn row_key(s: &State, k: usize) -> String {
    row_keys(s).get(k).cloned().unwrap_or_else(|| songs(s).get(k).map(|x| x.id.clone()).unwrap_or_default())
}
