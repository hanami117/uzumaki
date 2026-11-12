//! Favourites (Apple's star) for songs, albums and playlists.
//!
//! These are music.apple.com's own calls (read from its bundle on 2026-11-09):
//! - favourite and undo: `POST` / `DELETE /v1/me/favorites?ids[{type}]={ids}`, where the type is
//!   `songs`, `library-songs`, `albums`, `library-albums`, `playlists` or `library-playlists`;
//! - state: `GET /v1/catalog/{sf}?ids[songs]=…&fields[songs]=personalRating`, or
//!   `/v1/me/library?ids[library-songs]=…&fields[library-songs]=personalRating`.
//!   A `personalRating` of 1 means favourite, -1 means "suggest less", and no value means neither.
//!
//! How the app decides, by id (a song's library and catalog ids both count):
//! 1. an explicit answer (a lookup, or the user's own click) wins;
//! 2. otherwise a song in the Favourite Songs playlist is a favourite;
//! 3. otherwise it isn't, or isn't known yet. Both show an empty star.
//!
//! The playlist is read once a session, so stars show everywhere, Library → Songs included.
//! The playing song and the open album or playlist page (the page itself and its songs) are
//! also looked up, once a session each. That catches stars changed on another device.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::*;

/// Ids per lookup request.
const CHUNK: usize = 100;
/// The most Favourite Songs entries read (100 a page).
const LIST_CAP: usize = 5000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ListLoad {
    #[default]
    Idle,
    Loading,
    Done,
}

#[derive(Default)]
pub struct Favourites {
    /// Explicit answers by id: from a lookup or the user's own star.
    known: HashMap<String, bool>,
    /// Library and catalog ids of the songs in the Favourite Songs playlist.
    listed: HashSet<String>,
    list: ListLoad,
    /// Ids looked up this session (answered or not), so none is asked twice.
    asked: HashSet<String>,
    /// Open collection pages already looked up, with how many tracks were looked at.
    pages: HashMap<String, usize>,
    /// Bumped whenever a star may have changed: lazily built lists put it in their version.
    rev: u64,
}

impl Favourites {
    /// What the app knows about a song: Some(true) favourite, Some(false) not, None unknown.
    fn song(&self, s: &Song) -> Option<bool> {
        let ids = || std::iter::once(s.id.as_str()).chain(s.catalog_id.as_deref());
        if let Some(b) = ids().find_map(|id| self.known.get(id).copied()) {
            return Some(b);
        }
        if ids().any(|id| self.listed.contains(id)) {
            return Some(true);
        }
        (self.list == ListLoad::Done).then_some(false)
    }

    fn set(&mut self, ids: &[String], on: bool) {
        self.rev += 1;
        for id in ids {
            self.known.insert(id.clone(), on);
            if on {
                self.listed.insert(id.clone());
            } else {
                self.listed.remove(id);
            }
        }
    }
}

/// Apple's resource type for a song id, or None for local files.
fn song_type(s: &Song) -> Option<&'static str> {
    if s.is_local() || s.id.is_empty() {
        None
    } else if s.id.starts_with("i.") {
        Some("library-songs")
    } else {
        Some("songs")
    }
}

/// Apple's resource type for an album or playlist id.
fn collection_type(c: &Collection) -> &'static str {
    match c.kind {
        CollectionKind::Album if c.id.starts_with("l.") => "library-albums",
        CollectionKind::Album => "albums",
        CollectionKind::Playlist if c.id.starts_with("p.") => "library-playlists",
        CollectionKind::Playlist => "playlists",
    }
}

/// `personalRating` answers: (id, favourite) for every resource in `data`.
fn parse_ratings(v: &Value) -> Vec<(String, bool)> {
    v["data"]
        .as_array()
        .map(|d| {
            d.iter()
                .filter_map(|r| Some((r["id"].as_str()?.to_string(), r["attributes"]["personalRating"].as_i64() == Some(1))))
                .collect()
        })
        .unwrap_or_default()
}

/// One item to look up: the id sent, its resource type, and another id it is also known by
/// (a library song's catalog id).
type Ask = (String, &'static str, Option<String>);

impl State {
    /// True when the song carries Apple's star.
    pub fn is_favourite(&self, song: &Song) -> bool {
        self.favs.song(song) == Some(true)
    }

    /// The star can be set on this song: an Apple song, with the user signed in.
    pub fn can_favourite(&self, song: &Song) -> bool {
        self.session.authorized && song_type(song).is_some()
    }

    /// Changes whenever any star (or signing in) may have: virtual lists rebuild their rows.
    pub fn favourites_rev(&self) -> (u64, bool) {
        (self.favs.rev, self.session.authorized)
    }

    pub fn is_favourite_collection(&self, c: &Collection) -> bool {
        self.favs.known.get(&c.id).copied().unwrap_or(false)
    }

    /// The song menu's item and the star buttons: favourite them all, or, when every one of
    /// them already is, undo it.
    pub fn toggle_favourite_songs(&mut self, songs: &[Song]) {
        let apple: Vec<&Song> = songs.iter().filter(|s| song_type(s).is_some()).collect();
        let on = !apple.iter().all(|s| self.is_favourite(s));
        self.set_favourite_songs(songs, on);
    }

    /// Stars (`on`) or un-stars `songs`. Only the songs whose state changes are sent, so a
    /// failed request can turn them all back.
    pub fn set_favourite_songs(&mut self, songs: &[Song], on: bool) {
        if !self.session.authorized {
            self.toast = Some("Sign in to Apple Music to use favourites.".into());
            return;
        }
        let change: Vec<&Song> = songs.iter().filter(|s| song_type(s).is_some() && self.is_favourite(s) != on).collect();
        if change.is_empty() {
            return;
        }
        let what = match change.as_slice() {
            [one] => format!("“{}”", one.title),
            many => format!("{} songs", many.len()),
        };
        // Per resource kind: the ids sent, and those plus their catalog ids (what a refusal
        // turns back).
        let mut groups: Vec<(&'static str, Vec<String>, Vec<String>)> = Vec::new();
        for s in &change {
            let t = song_type(s).unwrap_or("songs");
            let i = groups.iter().position(|g| g.0 == t).unwrap_or_else(|| {
                groups.push((t, Vec::new(), Vec::new()));
                groups.len() - 1
            });
            let g = &mut groups[i];
            g.1.push(s.id.clone());
            g.2.push(s.id.clone());
            g.2.extend(s.catalog_id.clone());
        }
        let method = if on { "POST" } else { "DELETE" };
        for (t, mut sent, mut ids) in groups {
            sent.sort();
            sent.dedup();
            ids.sort();
            ids.dedup();
            self.favs.set(&ids, on);
            if !on {
                self.drop_from_favourites_page(&ids);
            }
            let what = what.clone();
            let path = format!("/v1/me/favorites?ids[{t}]={}", sent.join(","));
            self.write(method, path, None, move |result| Msg::FavouriteSet { ids, on, what, result });
        }
    }

    /// The star on an album or playlist page, or its card menu.
    pub fn toggle_favourite_collection(&mut self, c: &Collection) {
        if !self.session.authorized {
            self.toast = Some("Sign in to Apple Music to use favourites.".into());
            return;
        }
        let on = !self.is_favourite_collection(c);
        let ids = vec![c.id.clone()];
        self.favs.set(&ids, on);
        let method = if on { "POST" } else { "DELETE" };
        let path = format!("/v1/me/favorites?ids[{}]={}", collection_type(c), c.id);
        let what = format!("“{}”", c.title);
        self.write(method, path, None, move |result| Msg::FavouriteSet { ids, on, what, result });
    }

    /// Apple's answer to a star change.
    pub(super) fn favourite_set(&mut self, ids: Vec<String>, on: bool, what: String, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.toast = Some(if on { format!("Added {what} to Favourites") } else { format!("Removed {what} from Favourites") });
                if on {
                    self.favourites_page_changed();
                }
            }
            Err(e) => {
                self.favs.set(&ids, !on);
                self.toast = Some(if on {
                    format!("Couldn't add {what} to Favourites: {e}")
                } else {
                    format!("Couldn't remove {what} from Favourites: {e}")
                });
                // The Favourite Songs page may have lost rows that Apple kept.
                self.favourites_page_changed();
            }
        }
    }

    /// The Favourite Songs playlist's id, when the library playlists are in.
    fn favourites_playlist(&self) -> Option<String> {
        self.library_playlists.items.iter().find(|c| c.favorites).map(|c| c.id.clone())
    }

    /// Un-starred songs leave the Favourite Songs page at once.
    fn drop_from_favourites_page(&mut self, ids: &[String]) {
        let Some(fav) = self.favourites_playlist() else { return };
        let Some(Load::Ready(c)) = self.collections.get_mut(&fav) else { return };
        let before = c.tracks.len();
        c.tracks.retain(|t| !ids.contains(&t.id) && !t.catalog_id.as_ref().is_some_and(|id| ids.contains(id)));
        let gone = before - c.tracks.len();
        if gone > 0 {
            if let Some(n) = c.track_count.as_mut() {
                *n = n.saturating_sub(gone as u32);
            }
            let c = c.clone();
            self.save_collection(&c);
        }
    }

    /// Apple's Favourite Songs playlist changed: drop the copy we hold, so the next visit (or
    /// this one, if it is open) loads Apple's.
    fn favourites_page_changed(&mut self) {
        let Some(fav) = self.favourites_playlist() else { return };
        if !self.collections.contains_key(&fav) {
            return;
        }
        self.collections.remove(&fav);
        self.coll_cache.remove(&fav);
        self.coll_refresh.remove(&fav);
        if let Some(c) = &self.cache {
            c.forget_collection(&fav);
        }
        if matches!(&self.route, Route::Collection { id, .. } if *id == fav) {
            self.load_route_data();
        }
    }

    // ---------------------------------------------------------------- what Apple says

    /// Starts the lookups the screen needs and hasn't asked for yet. Runs after every message
    /// and navigation, so it must stay cheap when there's nothing to do.
    pub(super) fn favourite_lookups(&mut self) {
        if !self.session.authorized || !self.session.bridge_up {
            return;
        }
        // The Favourite Songs playlist, once a session.
        if self.favs.list == ListLoad::Idle {
            if let Some(id) = self.favourites_playlist() {
                self.favs.list = ListLoad::Loading;
                let req = (format!("/v1/me/library/playlists/{id}/tracks"), json!({"limit": 100}));
                self.request(req, |v| Page::parse(v, Song::parse), Msg::FavouriteList);
            }
        }
        let mut asks: Vec<Ask> = Vec::new();
        // The playing song.
        if let Some(song) = &self.playback.song {
            self.ask_song(song, &mut asks);
        }
        // The open album or playlist page: the page itself and its songs.
        if let Route::Collection { id, .. } = &self.route {
            if let Some(Load::Ready(c)) = self.collections.get(id) {
                let seen = self.favs.pages.get(id).copied();
                if seen != Some(c.tracks.len()) {
                    if seen.is_none() && !self.favs.asked.contains(&c.id) && !c.favorites {
                        asks.push((c.id.clone(), collection_type(c), None));
                    }
                    for t in &c.tracks[seen.unwrap_or(0).min(c.tracks.len())..] {
                        self.ask_song(t, &mut asks);
                    }
                    self.favs.pages.insert(id.clone(), c.tracks.len());
                }
            }
        }
        if !asks.is_empty() {
            self.look_up(asks);
        }
    }

    fn ask_song(&self, song: &Song, asks: &mut Vec<Ask>) {
        let Some(t) = song_type(song) else { return };
        if self.favs.known.contains_key(&song.id) || self.favs.asked.contains(&song.id) || asks.iter().any(|a| a.0 == song.id) {
            return;
        }
        asks.push((song.id.clone(), t, song.catalog_id.clone()));
    }

    /// One request per resource kind and `CHUNK` ids.
    fn look_up(&mut self, asks: Vec<Ask>) {
        let mut kinds: Vec<&'static str> = asks.iter().map(|a| a.1).collect();
        kinds.sort();
        kinds.dedup();
        for kind in kinds {
            let of_kind: Vec<&Ask> = asks.iter().filter(|a| a.1 == kind).collect();
            for chunk in of_kind.chunks(CHUNK) {
                let ids: Vec<&str> = chunk.iter().map(|a| a.0.as_str()).collect();
                let path = if kind.starts_with("library-") { "/v1/me/library".to_string() } else { "/v1/catalog/{{storefrontId}}".to_string() };
                let params = json!({format!("ids[{kind}]"): ids.join(","), format!("fields[{kind}]"): "personalRating"});
                let asked: Vec<(String, Option<String>)> = chunk.iter().map(|a| (a.0.clone(), a.2.clone())).collect();
                for (id, _) in &asked {
                    self.favs.asked.insert(id.clone());
                }
                self.request((path, params), parse_ratings, move |result| Msg::FavouriteLookup { asked, result });
            }
        }
    }

    /// Apple's `personalRating`s. An id it doesn't list isn't a favourite. A click made while
    /// the lookup was out wins over its answer.
    pub(super) fn favourite_lookup(&mut self, asked: Vec<(String, Option<String>)>, result: Result<Vec<(String, bool)>, String>) -> bool {
        let Ok(answers) = result else { return false }; // stays unknown; not asked again this session
        self.favs.rev += 1;
        for (id, alias) in asked {
            let on = answers.iter().any(|(a, b)| *a == id && *b);
            self.favs.known.entry(id).or_insert(on);
            if let Some(alias) = alias {
                self.favs.known.entry(alias).or_insert(on);
            }
        }
        true
    }

    /// A page of the Favourite Songs playlist.
    pub(super) fn favourite_list(&mut self, result: Result<Page<Song>, String>) -> bool {
        self.favs.rev += 1;
        let Ok(page) = result else {
            self.favs.list = ListLoad::Done;
            return true; // unknown songs now read as not starred
        };
        for s in &page.items {
            self.favs.listed.insert(s.id.clone());
            if let Some(c) = &s.catalog_id {
                self.favs.listed.insert(c.clone());
            }
        }
        match page.next {
            Some(next) if self.favs.listed.len() < 2 * LIST_CAP => self.request(api::next(&next, 100), |v| Page::parse(v, Song::parse), Msg::FavouriteList),
            _ => self.favs.list = ListLoad::Done,
        }
        true
    }

    /// Headless scenes: show these song, album or playlist ids as starred.
    pub fn star_for_scene(&mut self, ids: &[&str]) {
        for id in ids {
            self.favs.known.insert(id.to_string(), true);
        }
        self.favs.rev += 1;
    }

    /// Signing out (or in as someone else): forget every star.
    pub(super) fn forget_favourites(&mut self) {
        self.favs = Favourites { rev: self.favs.rev + 1, ..Favourites::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: format!("Song {id}"),
            artist: String::new(),
            album: String::new(),
            duration: 200.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: Vec::new(),
        }
    }

    fn signed_in() -> State {
        let mut s = State::offline();
        s.session.authorized = true;
        s.session.bridge_up = true;
        s
    }

    fn fav_playlist() -> Collection {
        Collection {
            kind: CollectionKind::Playlist,
            id: "p.fav".into(),
            title: "Favourite Songs".into(),
            subtitle: String::new(),
            artwork: None,
            year: None,
            genre: None,
            description: None,
            track_count: Some(2),
            modified: None,
            editable: false,
            favorites: true,
            public: None,
            tracks: Vec::new(),
            artists: Vec::new(),
        }
    }

    #[test]
    fn starring_sends_one_request_per_kind_and_undo_deletes() {
        let mut s = signed_in();
        let lib = Song { catalog_id: Some("111".into()), ..song("i.a") };
        s.toggle_favourite_songs(&[lib.clone(), song("222"), song("file:///x.flac")]);
        let sent = s.sent.borrow().clone();
        assert!(sent.contains(&"POST /v1/me/favorites?ids[library-songs]=i.a".to_string()), "{sent:?}");
        assert!(sent.contains(&"POST /v1/me/favorites?ids[songs]=222".to_string()), "{sent:?}");
        assert!(s.is_favourite(&lib) && s.is_favourite(&song("222")), "shown at once");
        assert!(s.is_favourite(&song("111")), "the catalog copy of a starred library song is starred too");
        // All starred now: the same click undoes it.
        s.toggle_favourite_songs(&[lib.clone(), song("222")]);
        assert_eq!(s.sent.borrow().last().unwrap(), "DELETE /v1/me/favorites?ids[songs]=222");
        assert!(!s.is_favourite(&lib));
    }

    #[test]
    fn a_refused_star_turns_back() {
        let mut s = signed_in();
        s.toggle_favourite_songs(&[song("222")]);
        assert!(s.is_favourite(&song("222")));
        s.apply_msg(Msg::FavouriteSet { ids: vec!["222".into()], on: true, what: "“Song 222”".into(), result: Err("HTTP 403".into()) });
        assert!(!s.is_favourite(&song("222")));
        assert_eq!(s.toast.as_deref(), Some("Couldn't add “Song 222” to Favourites: HTTP 403"));
    }

    #[test]
    fn signed_out_nothing_is_sent() {
        let mut s = State::offline();
        s.toggle_favourite_songs(&[song("222")]);
        assert!(s.sent.borrow().is_empty());
        assert!(s.toast.as_deref().unwrap().contains("Sign in"));
    }

    #[test]
    fn the_favourite_songs_playlist_stars_its_songs_everywhere() {
        let mut s = signed_in();
        s.library_playlists = Paged::ready(vec![fav_playlist()]);
        s.favourite_lookups();
        assert!(s.sent.borrow().iter().any(|r| r.starts_with("/v1/me/library/playlists/p.fav/tracks")));
        let page = Page { items: vec![Song { catalog_id: Some("333".into()), ..song("i.b") }], next: None, total: None };
        s.apply_msg(Msg::FavouriteList(Ok(page)));
        assert!(s.is_favourite(&song("i.b")));
        assert!(s.is_favourite(&song("333")), "found by catalog id in search results");
        assert!(!s.is_favourite(&song("444")));
        // Read once a session.
        let n = s.sent.borrow().len();
        s.favourite_lookups();
        assert_eq!(s.sent.borrow().len(), n);
    }

    #[test]
    fn the_playing_song_and_the_open_page_are_looked_up_once() {
        let mut s = signed_in();
        s.playback.song = Some(Song { catalog_id: Some("900".into()), ..song("i.now") });
        let album = Collection { kind: CollectionKind::Album, id: "l.alb".into(), favorites: false, tracks: vec![song("i.1"), song("i.2")], ..fav_playlist() };
        s.collections.insert("l.alb".into(), Load::Ready(album));
        s.route = Route::Collection { kind: CollectionKind::Album, id: "l.alb".into() };
        s.favourite_lookups();
        let sent = s.sent.borrow().clone();
        let songs = sent.iter().find(|r| r.contains("ids[library-songs]")).expect("songs looked up");
        assert!(songs.starts_with("/v1/me/library "), "{songs}");
        assert!(songs.contains("i.now,i.1,i.2") && songs.contains("personalRating"), "{songs}");
        assert!(sent.iter().any(|r| r.contains("\"ids[library-albums]\":\"l.alb\"")), "{sent:?}");
        // Answers: i.1 is a favourite, the rest aren't.
        s.apply_msg(Msg::FavouriteLookup {
            asked: vec![("i.now".into(), Some("900".into())), ("i.1".into(), None), ("i.2".into(), None)],
            result: Ok(vec![("i.1".into(), true), ("i.2".into(), false)]),
        });
        assert!(s.is_favourite(&song("i.1")) && !s.is_favourite(&song("i.2")) && !s.is_favourite(&song("900")));
        let n = s.sent.borrow().len();
        s.favourite_lookups();
        assert_eq!(s.sent.borrow().len(), n, "nothing asked twice");
    }

    #[test]
    fn a_click_during_a_lookup_wins_over_its_answer() {
        let mut s = signed_in();
        s.toggle_favourite_songs(&[song("5")]);
        s.apply_msg(Msg::FavouriteLookup { asked: vec![("5".into(), None)], result: Ok(vec![]) });
        assert!(s.is_favourite(&song("5")));
    }

    #[test]
    fn un_starring_drops_the_row_from_favourite_songs() {
        let mut s = signed_in();
        let mut fav = fav_playlist();
        fav.tracks = vec![song("i.a"), Song { catalog_id: Some("7".into()), ..song("i.b") }];
        s.library_playlists = Paged::ready(vec![fav.clone()]);
        s.collections.insert("p.fav".into(), Load::Ready(fav));
        s.favs.listed.extend(["i.a".to_string(), "i.b".to_string(), "7".to_string()]);
        s.toggle_favourite_songs(&[song("7")]);
        assert_eq!(s.sent.borrow().last().unwrap(), "DELETE /v1/me/favorites?ids[songs]=7");
        let c = s.collections["p.fav"].ready().unwrap();
        assert_eq!(c.tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["i.a"]);
        assert_eq!(c.track_count, Some(1));
    }

    #[test]
    fn albums_and_playlists_star_by_their_kind() {
        let mut s = signed_in();
        let alb = Collection { kind: CollectionKind::Album, id: "1440857781".into(), favorites: false, ..fav_playlist() };
        s.toggle_favourite_collection(&alb);
        assert_eq!(s.sent.borrow().last().unwrap(), "POST /v1/me/favorites?ids[albums]=1440857781");
        assert!(s.is_favourite_collection(&alb));
        s.toggle_favourite_collection(&alb);
        assert_eq!(s.sent.borrow().last().unwrap(), "DELETE /v1/me/favorites?ids[albums]=1440857781");
        let mine = Collection { id: "p.mine".into(), favorites: false, ..fav_playlist() };
        s.toggle_favourite_collection(&mine);
        assert_eq!(s.sent.borrow().last().unwrap(), "POST /v1/me/favorites?ids[library-playlists]=p.mine");
    }

    #[test]
    fn ratings_parse_from_apples_answer() {
        let v = json!({"data": [
            {"id": "1", "type": "songs", "attributes": {"personalRating": 1}},
            {"id": "2", "type": "songs", "attributes": {"personalRating": -1}},
            {"id": "3", "type": "songs", "attributes": {}}
        ]});
        assert_eq!(parse_ratings(&v), [("1".to_string(), true), ("2".to_string(), false), ("3".to_string(), false)]);
    }
}
