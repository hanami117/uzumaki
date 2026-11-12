//! Search inside the already-loaded library, as you type. The library is folded (lower-case,
//! accents removed) once into a search index; each keystroke is then one `contains` per entry.
//! Both the index and the last result are cached in `UiState`, so time ticks cost nothing.

use std::cell::RefCell;
use std::rc::Rc;

use super::songs_table::fold;
use crate::models::Song;
use crate::state::State;

/// Songs matched before stopping, unless the user asks for all.
pub const SONG_CAP: usize = 200;
/// Albums, artists and playlists shown per group.
pub const GROUP_CAP: usize = 60;

#[derive(PartialEq, Eq, Clone)]
struct IndexKey {
    lens: [usize; 4],
    ends: [String; 2],
}

struct Index {
    key: IndexKey,
    songs: Vec<String>,
    albums: Vec<String>,
    artists: Vec<String>,
    playlists: Vec<String>,
}

pub struct Found {
    /// Matching songs in library order (at most `SONG_CAP` unless "show all").
    pub songs: Rc<Vec<Song>>,
    /// The scan stopped at the cap: there may be more songs.
    pub more_songs: bool,
    pub albums: Vec<usize>,
    pub artists: Vec<usize>,
    pub playlists: Vec<usize>,
    /// Some group stopped at its cap.
    pub more_groups: bool,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty() && self.albums.is_empty() && self.artists.is_empty() && self.playlists.is_empty()
    }
}

/// Folded local songs, with the (length, first id, last id) they were built from.
type LocalIndex = ((usize, String, String), Vec<String>);

/// Local-files search result: the matching songs and whether the cap cut the scan short.
pub struct LocalFound {
    pub songs: Rc<Vec<Song>>,
    pub more: bool,
}

#[derive(Default)]
pub struct Cache {
    local_index: RefCell<Option<LocalIndex>>,
    local_last: RefCell<Option<((String, (usize, String, String), bool), Rc<LocalFound>)>>,
    index: RefCell<Option<Index>>,
    last: RefCell<Option<((String, IndexKey, bool), Rc<Found>)>>,
}

fn key_of(s: &State) -> IndexKey {
    let id = |v: Option<&str>| v.unwrap_or("").to_string();
    IndexKey {
        lens: [s.library_songs.items.len(), s.library_albums.items.len(), s.library_artists.items.len(), s.library_playlists.items.len()],
        ends: [
            id(s.library_songs.items.first().map(|x| x.id.as_str())),
            id(s.library_songs.items.last().map(|x| x.id.as_str())),
        ],
    }
}

fn build(s: &State, key: IndexKey) -> Index {
    Index {
        key,
        songs: s.library_songs.items.iter().map(|x| fold(&format!("{}\u{1}{}\u{1}{}", x.title, x.artist, x.album))).collect(),
        albums: s.library_albums.items.iter().map(|c| fold(&format!("{}\u{1}{}", c.title, c.subtitle))).collect(),
        artists: s.library_artists.items.iter().map(|a| fold(&a.name)).collect(),
        playlists: s.library_playlists.items.iter().map(|c| fold(&c.title)).collect(),
    }
}

/// Every whitespace-separated word must appear somewhere in the entry.
fn hit(entry: &str, words: &[&str]) -> bool {
    words.iter().all(|w| entry.contains(w))
}

/// Indices of matching entries; stops once `cap` are found (and says so).
fn scan(entries: &[String], words: &[&str], cap: usize) -> (Vec<usize>, bool) {
    let mut out = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if hit(e, words) {
            if out.len() == cap {
                return (out, true);
            }
            out.push(i);
        }
    }
    (out, false)
}

/// Matches for the search field's text, or None while it is empty. `show_all` lifts the caps.
pub fn find(s: &State) -> Option<Rc<Found>> {
    let q = fold(&s.ui.search.text);
    if q.is_empty() {
        return None;
    }
    let all = s.ui.lib_show_all;
    let key = key_of(s);
    let cache = &s.ui.lib;
    if let Some(((cq, ck, ca), f)) = cache.last.borrow().as_ref() {
        if *cq == q && *ck == key && *ca == all {
            return Some(f.clone());
        }
    }
    if cache.index.borrow().as_ref().is_none_or(|i| i.key != key) {
        *cache.index.borrow_mut() = Some(build(s, key.clone()));
    }
    let idx = cache.index.borrow();
    let idx = idx.as_ref()?;
    let words: Vec<&str> = q.split_whitespace().collect();
    let (song_cap, group_cap) = if all { (usize::MAX, usize::MAX) } else { (SONG_CAP, GROUP_CAP) };
    let (song_ix, more_songs) = scan(&idx.songs, &words, song_cap);
    let (albums, m1) = scan(&idx.albums, &words, group_cap);
    let (artists, m2) = scan(&idx.artists, &words, group_cap);
    let (playlists, m3) = scan(&idx.playlists, &words, group_cap);
    let songs: Vec<Song> = song_ix.iter().map(|&i| s.library_songs.items[i].clone()).collect();
    let found = Rc::new(Found { songs: Rc::new(songs), more_songs, albums, artists, playlists, more_groups: m1 || m2 || m3 });
    *cache.last.borrow_mut() = Some(((q, key, all), found.clone()));
    Some(found)
}

/// Matches among the local songs for the search field's text, or None while it is empty.
pub fn find_local(s: &State) -> Option<Rc<LocalFound>> {
    let q = fold(&s.ui.search.text);
    if q.is_empty() {
        return None;
    }
    let all = s.ui.lib_show_all;
    let songs = &s.local_lib.songs;
    let key = (songs.len(), songs.first().map(|x| x.id.clone()).unwrap_or_default(), songs.last().map(|x| x.id.clone()).unwrap_or_default());
    let cache = &s.ui.lib;
    if let Some(((cq, ck, ca), f)) = cache.local_last.borrow().as_ref() {
        if *cq == q && *ck == key && *ca == all {
            return Some(f.clone());
        }
    }
    if cache.local_index.borrow().as_ref().is_none_or(|(k, _)| *k != key) {
        let idx = songs.iter().map(|x| fold(&format!("{}\u{1}{}\u{1}{}", x.title, x.artist, x.album))).collect();
        *cache.local_index.borrow_mut() = Some((key.clone(), idx));
    }
    let idx = cache.local_index.borrow();
    let (_, idx) = idx.as_ref()?;
    let words: Vec<&str> = q.split_whitespace().collect();
    let (ix, more) = scan(idx, &words, if all { usize::MAX } else { SONG_CAP });
    let found = Rc::new(LocalFound { songs: Rc::new(ix.iter().map(|&i| songs[i].clone()).collect()), more });
    *cache.local_last.borrow_mut() = Some(((q, key, all), found.clone()));
    Some(found)
}
