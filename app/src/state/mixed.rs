//! Mixed playlists in the app state (`crate::mixed` holds the data, the merge and the file):
//! local songs inside the user's Apple playlists (the overlay), and Uzumaki playlists.
//!
//! Apple's list in `collections` is never changed by the overlay: a playlist page, a Play and
//! `uzu` read the merged list through `State::merged`, cached by its inputs (Apple's list, the
//! overlay's revision, the music folders' index), so the view never merges per frame.
//!
//! An Uzumaki playlist opens as an ordinary `Collection` (id `u.…`, built from the store by
//! `uzu_collection`) in `collections`, rebuilt whenever it or the music folders change, so the
//! page, the menus, selection and playback treat it like any playlist. Nothing here talks to
//! Apple: Apple songs in an Uzumaki playlist are kept as they were when added.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use super::*;
use crate::mixed::{self as mx, Found, LocalIndex, LocalRef, Merged, Origin, Store, UzuEntry, UzuPlaylist};

/// What a merged list was built from: Apple's list (its buffer, length and ends), the store's
/// revision and the music folders' index generation.
type MergeKey = (usize, usize, String, String, u64, u64, bool);

/// The music folders' index, with the (generation, loaded) it was built for.
type IndexSlot = Option<((u64, bool), Rc<LocalIndex>)>;

/// The store and its caches.
#[derive(Default)]
pub struct Mixed {
    pub store: Store,
    /// Counts changes to `store` (merged lists and the sidebar rebuild).
    pub rev: u64,
    /// The file; None in headless runs and tests (never written).
    path: Option<PathBuf>,
    /// The file was written by a newer Uzumaki: shown, never written over.
    read_only: bool,
    /// Why the file couldn't be read, shown once at startup.
    pub warning: Option<String>,
    /// The Uzumaki playlists as sidebar entries (no songs), newest first.
    pub cards: Vec<Collection>,
    index: RefCell<IndexSlot>,
    merged: RefCell<HashMap<String, (MergeKey, Rc<Merged>)>>,
}

impl Mixed {
    /// The app's store (`persist`), or an empty one that is never saved (headless, tests).
    pub fn new(persist: bool) -> Self {
        if !persist {
            return Self::default();
        }
        let path = mx::path();
        let mut warning = None;
        let (store, read_only) = match Store::read(&path) {
            mx::Loaded::Ok(s) => (s, false),
            mx::Loaded::Newer(s) => {
                eprintln!("uzumaki: {} is from a newer Uzumaki; playlist changes won't be saved", path.display());
                (s, true)
            }
            mx::Loaded::Broken => {
                eprintln!("uzumaki: {} was unreadable; kept as playlists.json.bad", path.display());
                (Store::default(), false)
            }
            mx::Loaded::Unreadable(e) => {
                eprintln!("uzumaki: can't read {}: {e}; playlist changes won't be saved", path.display());
                warning = Some(format!("Couldn't read your Uzumaki playlists ({e}). They're kept as they are; changes won't be saved until the app can read playlists.json."));
                (Store::default(), true)
            }
        };
        let mut m = Self { store, path: Some(path), read_only, warning, ..Self::default() };
        m.cards = cards(&m.store);
        m
    }

    /// Whether the Apple playlist `id` has local songs in it.
    pub fn has_overlay(&self, id: &str) -> bool {
        !self.store.overlay(id).is_empty()
    }
}

/// The sidebar entries of the Uzumaki playlists.
fn cards(store: &Store) -> Vec<Collection> {
    store.playlists.iter().map(|p| card(p, Vec::new())).collect()
}

fn card(p: &UzuPlaylist, tracks: Vec<Song>) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: p.id.clone(),
        title: p.name.clone(),
        subtitle: "Uzumaki playlist \u{b7} on this computer".into(),
        artwork: None,
        year: None,
        genre: None,
        description: p.description.clone(),
        track_count: Some(p.entries.len() as u32),
        modified: None,
        editable: false,
        favorites: false,
        public: None,
        artists: Vec::new(),
        tracks,
    }
}

/// Song counts in a toast: "“Song”" or "3 songs".
fn what(songs: &[Song]) -> String {
    match songs {
        [one] => format!("“{}”", one.title),
        _ => format!("{} songs", songs.len()),
    }
}

impl State {
    // ---------------------------------------------------------------- reading

    /// The music folders' index for resolving local songs, built once per index change.
    fn local_index(&self) -> Rc<LocalIndex> {
        let key = (self.local_lib.generation, self.local_lib.loaded);
        if let Some((k, ix)) = self.mixed.index.borrow().as_ref()
            && *k == key
        {
            return ix.clone();
        }
        let ix = Rc::new(LocalIndex::new(&self.local_lib.songs));
        *self.mixed.index.borrow_mut() = Some((key, ix.clone()));
        ix
    }

    /// The song a stored local reference stands for (a greyed stand-in when it's missing).
    fn resolve_local(&self, r: &LocalRef, ix: &LocalIndex) -> Song {
        mx::resolve(r, &self.local_lib.songs, ix, self.local_lib.loaded)
    }

    /// The rows of playlist `c` as its page shows them: Apple's songs with the overlay's local
    /// songs in their places (an Uzumaki playlist's own songs as they are). Cached by its inputs.
    pub fn merged(&self, c: &Collection) -> Rc<Merged> {
        let entries = self.mixed.store.overlay(&c.id);
        let t = &c.tracks;
        let end = |s: Option<&Song>| s.map(|s| s.id.clone()).unwrap_or_default();
        let key: MergeKey = (t.as_ptr() as usize, t.len(), end(t.first()), end(t.last()), self.mixed.rev, self.local_lib.generation, self.local_lib.loaded);
        if let Some((k, m)) = self.mixed.merged.borrow().get(&c.id)
            && *k == key
        {
            return m.clone();
        }
        let m = if entries.is_empty() || mx::is_uzu_id(&c.id) {
            Merged::plain(t)
        } else {
            let ix = self.local_index();
            mx::merge(t, entries, |r| self.resolve_local(r, &ix))
        };
        let m = Rc::new(m);
        let mut cache = self.mixed.merged.borrow_mut();
        if cache.len() > 16 {
            cache.clear();
        }
        cache.insert(c.id.clone(), (key, m.clone()));
        m
    }

    /// `c`'s songs to play or list: merged with its overlay when it has one.
    pub fn playlist_songs(&self, c: &Collection) -> Vec<Song> {
        if self.mixed.has_overlay(&c.id) { self.merged(c).tracks.clone() } else { c.tracks.clone() }
    }

    /// The Uzumaki playlist `id` with its songs resolved.
    pub fn uzu_collection(&self, id: &str) -> Option<Collection> {
        let p = self.mixed.store.playlist(id)?;
        let ix = self.local_index();
        let tracks = p
            .entries
            .iter()
            .map(|e| match e {
                UzuEntry::Apple { song } => song.clone(),
                UzuEntry::Local { file } => self.resolve_local(file, &ix),
            })
            .collect();
        Some(card(p, tracks))
    }

    /// Gives the open Uzumaki playlists (in `collections`) their current songs.
    pub(super) fn refresh_uzu_collections(&mut self) {
        let ids: Vec<String> = self.collections.keys().filter(|id| mx::is_uzu_id(id)).cloned().collect();
        for id in ids {
            match self.uzu_collection(&id) {
                Some(c) => {
                    self.collections.insert(id, Load::Ready(c));
                }
                None => {
                    self.collections.remove(&id);
                }
            }
        }
        if let Route::Collection { id, .. } = &self.route
            && mx::is_uzu_id(id)
            && !self.collections.contains_key(id)
        {
            let load = match self.uzu_collection(id) {
                Some(c) => Load::Ready(c),
                None => Load::Failed("This playlist was deleted.".into()),
            };
            self.collections.insert(id.clone(), load);
        }
    }

    /// The user's playlists that "Add to Playlist" offers: their own Apple playlists, then the
    /// Uzumaki playlists.
    pub fn add_targets(&self) -> Vec<&Collection> {
        self.library_playlists.items.iter().filter(|c| c.editable).chain(self.mixed.cards.iter()).collect()
    }

    /// A playlist "Add to Playlist" can take songs into, by id.
    pub fn add_target(&self, id: &str) -> Option<Collection> {
        self.add_targets().into_iter().find(|c| c.id == id).cloned()
    }

    // ---------------------------------------------------------------- writing

    /// The store changed: caches rebuild, open pages follow, the file is saved.
    fn mixed_changed(&mut self) {
        self.mixed.rev += 1;
        self.mixed.cards = cards(&self.mixed.store);
        self.refresh_uzu_collections();
        if let (Some(path), false) = (&self.mixed.path, self.mixed.read_only) {
            mx::save_in_background(&self.mixed.store, path.clone());
        }
    }

    /// Apple's list of playlist `id` as far as it's loaded, and its song count.
    fn apple_list(&self, id: &str) -> (Vec<Song>, usize) {
        let loaded = self.collections.get(id).and_then(Load::ready);
        let tracks = loaded.map(|c| c.tracks.clone()).unwrap_or_default();
        let listed = self.library_playlists.items.iter().find(|c| c.id == id).and_then(|c| c.track_count);
        let count = listed.or(loaded.and_then(|c| c.track_count)).map_or(tracks.len(), |n| n as usize);
        (tracks, count)
    }

    /// Adds local songs to the overlay of the user's Apple playlist `playlist` (at its end).
    pub fn add_local_to_playlist(&mut self, playlist: &Collection, songs: &[Song]) -> usize {
        let files: Vec<LocalRef> = songs.iter().filter_map(LocalRef::of).collect();
        if files.is_empty() {
            return 0;
        }
        let n = files.len();
        // The caller's copy when it has the songs (a playlist Apple just made isn't open yet).
        let (apple, count) = if playlist.tracks.is_empty() { self.apple_list(&playlist.id) } else { (playlist.tracks.clone(), playlist.tracks.len()) };
        let entries = self.mixed.store.overlays.entry(playlist.id.clone()).or_default();
        mx::append(entries, &apple, count, files);
        self.mixed_changed();
        n
    }

    /// Creates an Uzumaki playlist holding `songs` and opens it. Returns its id.
    pub fn create_uzu_playlist(&mut self, name: &str, description: &str, songs: &[Song]) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let id = mx::new_id(&self.mixed.store.playlists);
        let now = mx::now_secs();
        let description = Some(description.trim().to_string()).filter(|d| !d.is_empty());
        let entries = songs.iter().filter_map(entry_of).collect();
        self.mixed.store.playlists.insert(0, UzuPlaylist { id: id.clone(), name: name.to_string(), description, created: now, modified: now, entries });
        self.mixed_changed();
        self.toast = Some(format!("Created “{name}” on this computer"));
        if let Some(c) = self.uzu_collection(&id) {
            self.collections.insert(id.clone(), Load::Ready(c.clone()));
            self.open_collection(&c);
        }
        Some(id)
    }

    /// Renames an Uzumaki playlist.
    pub fn rename_uzu_playlist(&mut self, id: &str, name: &str, description: &str) {
        let name = name.trim();
        let Some(p) = self.mixed.store.playlist_mut(id) else { return };
        if name.is_empty() {
            return;
        }
        let description = Some(description.trim().to_string()).filter(|d| !d.is_empty());
        if p.name == name && p.description == description {
            return;
        }
        p.name = name.to_string();
        p.description = description;
        p.modified = mx::now_secs();
        self.mixed_changed();
    }

    /// Deletes an Uzumaki playlist (and its pin); its page goes back.
    pub fn delete_uzu_playlist(&mut self, id: &str) {
        let Some(at) = self.mixed.store.playlists.iter().position(|p| p.id == id) else { return };
        let p = self.mixed.store.playlists.remove(at);
        if self.settings.pins.iter().any(|x| x.id == id) {
            self.settings.pins.retain(|x| x.id != id);
            self.save_settings();
        }
        self.collections.remove(id);
        self.remove_playlist_picture(id);
        self.back_stack.retain(|r| !matches!(r, Route::Collection { id: x, .. } if x == id));
        let here = matches!(&self.route, Route::Collection { id: x, .. } if x == id);
        if here {
            let home = if self.local_only() { Route::LocalFiles } else { Route::Home };
            self.route = self.back_stack.pop().unwrap_or(home);
            self.ui.route_changed();
            self.remember_route();
            self.load_route_data();
        }
        self.mixed_changed();
        self.collections.remove(id);
        self.toast = Some(format!("Deleted “{}”", p.name));
    }

    /// Adds `songs` (Apple songs and local files alike) to the end of an Uzumaki playlist.
    pub fn add_to_uzu(&mut self, id: &str, songs: &[Song]) {
        let entries: Vec<UzuEntry> = songs.iter().filter_map(entry_of).collect();
        let Some(p) = self.mixed.store.playlist_mut(id) else { return };
        if entries.is_empty() {
            return;
        }
        p.entries.extend(entries);
        p.modified = mx::now_secs();
        let name = p.name.clone();
        self.mixed_changed();
        self.toast = Some(format!("Added {} to “{name}”", what(songs)));
    }

    /// "Remove from Playlist" on rows of playlist `c` (`rows`: each row's position in the merged
    /// list, as `lists::visible_rows_at` gives it, and its song). Local rows leave the overlay
    /// (Apple's list isn't touched); Apple rows go to `remove_from_playlist` as before.
    pub fn remove_playlist_rows(&mut self, c: &Collection, rows: &[(usize, Song)]) {
        if mx::is_uzu_id(&c.id) {
            return self.remove_uzu_rows(&c.id, rows);
        }
        if !self.mixed.has_overlay(&c.id) {
            return self.remove_from_playlist(c, rows);
        }
        let full = self.collections.get(&c.id).and_then(Load::ready).cloned().unwrap_or_else(|| c.clone());
        let merged = self.merged(&full);
        let mut locals: Vec<usize> = Vec::new();
        let mut apple: Vec<(usize, Song)> = Vec::new();
        for (m, song) in rows {
            match merged.origin.get(*m) {
                Some(Origin::Local(k)) if merged.tracks[*m].id == song.id => locals.push(*k),
                Some(Origin::Apple(i)) if merged.tracks[*m].id == song.id => apple.push((*i, song.clone())),
                _ => {
                    self.toast = Some(format!("“{}” changed. Try again.", c.title));
                    return;
                }
            }
        }
        if !locals.is_empty() {
            locals.sort_unstable();
            locals.dedup();
            if let Some(entries) = self.mixed.store.overlays.get_mut(&c.id) {
                for &k in locals.iter().rev() {
                    if k < entries.len() {
                        entries.remove(k);
                    }
                }
                if entries.is_empty() {
                    self.mixed.store.overlays.remove(&c.id);
                }
            }
            self.mixed_changed();
            self.ui.sel.clear();
            let n = locals.len();
            self.toast = Some(format!("Removed {n} local song{} from “{}”", if n == 1 { "" } else { "s" }, c.title));
        }
        if !apple.is_empty() {
            self.remove_from_playlist(&full, &apple);
        }
    }

    fn remove_uzu_rows(&mut self, id: &str, rows: &[(usize, Song)]) {
        let Some(p) = self.mixed.store.playlist_mut(id) else { return };
        let mut drop: Vec<usize> = rows.iter().map(|(i, _)| *i).filter(|&i| i < p.entries.len()).collect();
        drop.sort_unstable();
        drop.dedup();
        for &i in drop.iter().rev() {
            p.entries.remove(i);
        }
        p.modified = mx::now_secs();
        let name = p.name.clone();
        self.mixed_changed();
        self.ui.sel.clear();
        let n = drop.len();
        self.toast = Some(format!("Removed {n} song{} from “{name}”", if n == 1 { "" } else { "s" }));
    }

    /// Whether the row at merged position `m` of playlist `c` can move by `delta` (-1 up, +1
    /// down): any row of an Uzumaki playlist, a local row of an Apple playlist's overlay.
    pub fn can_move_row(&self, c: &Collection, m: usize, delta: isize) -> bool {
        let Some(to) = m.checked_add_signed(delta) else { return false };
        if mx::is_uzu_id(&c.id) {
            return self.mixed.store.playlist(&c.id).is_some_and(|p| m < p.entries.len() && to < p.entries.len());
        }
        let merged = self.merged(c);
        matches!(merged.origin.get(m), Some(Origin::Local(_))) && to < merged.origin.len()
    }

    /// Moves the row at merged position `m` of playlist `c` one row up or down (see `can_move_row`).
    pub fn move_playlist_row(&mut self, c: &Collection, m: usize, delta: isize) {
        if !self.can_move_row(c, m, delta) {
            return;
        }
        let to = m.wrapping_add_signed(delta);
        if mx::is_uzu_id(&c.id) {
            if let Some(p) = self.mixed.store.playlist_mut(&c.id) {
                p.entries.swap(m, to);
                p.modified = mx::now_secs();
            }
        } else {
            let apple = c.tracks.clone();
            let Some(entries) = self.mixed.store.overlays.get_mut(&c.id) else { return };
            if !mx::move_row(entries, &apple, m, delta) {
                return;
            }
        }
        self.mixed_changed();
    }

    /// After a scan: local songs that moved (found by their tags at a new path) keep that path.
    pub(super) fn relink_moved(&mut self) {
        if !self.local_lib.loaded {
            return;
        }
        let ix = self.local_index();
        let songs = &self.local_lib.songs;
        let mut changed = false;
        let mut fix = |r: &mut LocalRef| {
            if let Found::Moved(i) = ix.find(songs, r)
                && let Some(new) = LocalRef::of(&songs[i])
            {
                *r = new;
                changed = true;
            }
        };
        for entries in self.mixed.store.overlays.values_mut() {
            entries.iter_mut().for_each(|e| fix(&mut e.file));
        }
        for p in &mut self.mixed.store.playlists {
            for e in &mut p.entries {
                if let UzuEntry::Local { file } = e {
                    fix(file);
                }
            }
        }
        if changed {
            self.mixed_changed();
        }
    }
}

/// How a song is kept in an Uzumaki playlist.
fn entry_of(song: &Song) -> Option<UzuEntry> {
    if song.is_local() {
        return LocalRef::of(song).map(|file| UzuEntry::Local { file });
    }
    song.playable.then(|| UzuEntry::Apple { song: song.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str) -> Song {
        let mut s = crate::state::tests::song(id);
        s.title = title.into();
        s
    }

    fn local(name: &str) -> Song {
        let mut s = song(&format!("file:///music/{name}.flac"), name);
        s.album = "Album".into();
        s
    }

    fn playlist(id: &str, tracks: Vec<Song>) -> Collection {
        let mut c = card(&UzuPlaylist { id: id.into(), name: "Mine".into(), description: None, created: 0, modified: 0, entries: Vec::new() }, tracks);
        c.subtitle = String::new();
        c.editable = true;
        c.track_count = Some(c.tracks.len() as u32);
        c
    }

    fn signed_in_with(c: Collection, files: Vec<Song>) -> State {
        let mut s = State::offline();
        s.session.authorized = true;
        s.library_playlists.items.push(Collection { tracks: Vec::new(), ..c.clone() });
        s.collections.insert(c.id.clone(), Load::Ready(c));
        s.local_lib.songs = files;
        s.local_lib.loaded = true;
        s.local_lib.generation = 1;
        s
    }

    fn titles(v: &[Song]) -> Vec<&str> {
        v.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn local_files_join_an_apple_playlist_here_and_nothing_goes_to_apple() {
        let c = playlist("p.1", vec![song("i.1", "One"), song("i.2", "Two")]);
        let mut s = signed_in_with(c.clone(), vec![local("x")]);
        s.add_to_playlist(&c, &[local("x")]);
        assert!(s.sent.borrow().is_empty(), "no request: {:?}", s.sent.borrow());
        let full = s.collections["p.1"].ready().unwrap().clone();
        assert_eq!(full.tracks.len(), 2, "Apple's list is untouched");
        assert_eq!(titles(&s.merged(&full).tracks), ["One", "Two", "x"]);
        assert_eq!(titles(&s.playlist_songs(&full)), ["One", "Two", "x"]);
        assert!(s.toast.as_deref().unwrap().contains("this computer"), "{:?}", s.toast);
    }

    #[test]
    fn a_mixed_add_sends_only_the_apple_songs() {
        let c = playlist("p.1", vec![song("i.1", "One")]);
        let mut s = signed_in_with(c.clone(), vec![local("x")]);
        s.add_to_playlist(&c, &[local("x"), song("123", "Cat")]);
        let sent = s.sent.borrow().clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].starts_with("POST /v1/me/library/playlists/p.1/tracks") && !sent[0].contains("file://"), "{sent:?}");
        assert!(s.mixed.has_overlay("p.1"));
    }

    #[test]
    fn removing_a_local_row_only_edits_the_overlay() {
        let c = playlist("p.1", vec![song("i.1", "One"), song("i.2", "Two")]);
        let mut s = signed_in_with(c.clone(), vec![local("x"), local("y")]);
        s.add_to_playlist(&c, &[local("x"), local("y")]);
        let rows = vec![(2, local("x"))];
        s.remove_playlist_rows(&c, &rows);
        assert!(s.sent.borrow().is_empty(), "{:?}", s.sent.borrow());
        assert_eq!(titles(&s.playlist_songs(&c)), ["One", "Two", "y"]);
        // An Apple row and a local row: the local one leaves here, the Apple one goes to Apple
        // at its own position in Apple's list (1), not the page's.
        s.remove_playlist_rows(&c, &[(1, song("i.2", "Two")), (2, local("y"))]);
        let sent = s.sent.borrow().clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].starts_with("DELETE /v1/me/library/playlists/p.1/tracks?ids[library-songs]=i.2"), "{sent:?}");
        assert!(!s.mixed.has_overlay("p.1"));
    }

    #[test]
    fn local_rows_move_and_apple_rows_stay() {
        let c = playlist("p.1", vec![song("i.1", "One"), song("i.2", "Two")]);
        let mut s = signed_in_with(c.clone(), vec![local("x")]);
        s.add_to_playlist(&c, &[local("x")]);
        assert!(!s.can_move_row(&c, 0, 1), "Apple's rows are Apple's to order");
        assert!(!s.can_move_row(&c, 2, 1), "last");
        s.move_playlist_row(&c, 2, -1);
        s.move_playlist_row(&c, 1, -1);
        assert_eq!(titles(&s.playlist_songs(&c)), ["x", "One", "Two"]);
        assert!(s.sent.borrow().is_empty());
    }

    #[test]
    fn a_missing_file_shows_greyed_and_a_moved_one_is_relinked() {
        let c = playlist("p.1", vec![song("i.1", "One")]);
        let mut s = signed_in_with(c.clone(), vec![local("x"), local("y")]);
        s.add_to_playlist(&c, &[local("x"), local("y")]);
        // y moved to another folder, x is gone.
        let mut moved = local("y");
        moved.id = "file:///new/place/y.flac".into();
        s.set_local_index(vec![moved], Default::default());
        let rows = s.playlist_songs(&c);
        assert!(crate::mixed::is_missing(&rows[1]), "x: {:?}", rows[1]);
        assert_eq!(rows[2].id, "file:///new/place/y.flac");
        assert_eq!(s.mixed.store.overlay("p.1")[1].file.path, PathBuf::from("/new/place/y.flac"), "the new path is kept");
        // Playing the list skips the missing file.
        s.play_collection(&c, 0);
        assert_eq!(s.queue.original.iter().filter(|x| x.playable).count(), 2);
    }

    #[test]
    fn uzu_add_local_needs_a_file_of_the_music_folders_and_lists_it() {
        let c = playlist("p.1", vec![song("i.1", "One")]);
        let mut s = signed_in_with(c, vec![local("x")]);
        let (info, songs) = s.ctl_add_local("p.1", "/music/x.flac").unwrap();
        assert_eq!(info.id, "p.1");
        assert_eq!(songs.iter().map(|x| (x.title.as_str(), x.local)).collect::<Vec<_>>(), [("One", false), ("x", true)]);
        assert!(s.ctl_add_local("p.1", "/elsewhere/y.flac").unwrap_err().contains("music folders"));
        assert!(s.ctl_add_local("pl.catalog", "/music/x.flac").is_err(), "not the user's playlist");
        assert!(s.sent.borrow().is_empty());
    }

    #[test]
    fn uzumaki_playlists_hold_both_kinds_and_play_as_one_queue() {
        let mut s = State::offline();
        s.local_lib.songs = vec![local("x")];
        s.local_lib.loaded = true;
        let id = s.create_uzu_playlist("Road trip", "", &[song("123", "Cat"), local("x")]).unwrap();
        assert!(id.starts_with("u."));
        assert_eq!(s.route, Route::Collection { kind: CollectionKind::Playlist, id: id.clone() });
        assert_eq!(s.mixed.cards[0].title, "Road trip");
        let c = s.collections[&id].ready().unwrap().clone();
        assert_eq!(titles(&c.tracks), ["Cat", "x"]);
        s.add_to_uzu(&id, &[local("x")]);
        assert_eq!(s.collections[&id].ready().unwrap().tracks.len(), 3, "the open page follows");
        let c = s.collections[&id].ready().unwrap().clone();
        s.move_playlist_row(&c, 0, 1);
        s.remove_playlist_rows(&c, &[(2, local("x"))]);
        assert_eq!(titles(&s.collections[&id].ready().unwrap().tracks), ["x", "Cat"]);
        s.rename_uzu_playlist(&id, "Night drive", "");
        assert_eq!(s.collections[&id].ready().unwrap().title, "Night drive");
        let c = s.collections[&id].ready().unwrap().clone();
        s.play_collection(&c, 0);
        assert_eq!(titles(&s.queue.original), ["x", "Cat"]);
        assert!(s.sent.borrow().is_empty(), "nothing asked of Apple: {:?}", s.sent.borrow());
        s.delete_uzu_playlist(&id);
        assert!(s.mixed.cards.is_empty() && !s.collections.contains_key(&id));
        assert_ne!(s.route, Route::Collection { kind: CollectionKind::Playlist, id });
    }
}
