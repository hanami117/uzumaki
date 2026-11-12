use std::collections::{HashSet, VecDeque};

use serde_json::Value;

use crate::cache::{Cached, Stored};
use crate::models::{CollectionKind, Page, api};

use super::*;

impl State {
    /// Forgets every list and page of the account in memory (signing in or out), and starts a
    /// new `Session::account`: replies to what was asked before are dropped when they land.
    pub(super) fn clear_personal(&mut self) {
        self.session.account += 1;
        self.shelves = Default::default();
        self.shelf_stale = [false; 2];
        self.library_albums = Paged::default();
        self.library_playlists = Paged::default();
        self.library_songs = Paged::default();
        self.library_artists = Paged::default();
        self.recently_added = Paged::default();
        self.made_for_you = Load::Idle;
        self.radio[1] = Load::Idle; // the personal station
        self.collections.retain(|id, _| !personal_id(id));
        self.coll_cache.retain(|id, _| !personal_id(id));
        self.coll_refresh.retain(|id, _| !personal_id(id));
        self.artist_pages.retain(|id, _| !Artist::is_library_id(id));
        self.artist_albums.retain(|id, _| !Artist::is_library_id(id));
        self.pl_rewrites.clear();
        self.creating_playlist = false;
        self.pl_sync = PlaylistSync::default();
        self.forget_favourites();
    }

    /// Signed out: the account's data goes, in memory and on disk, and so do the pages and pins
    /// that showed it. Local files stay.
    pub(super) fn drop_account(&mut self) {
        self.clear_personal();
        self.syncing = false;
        self.forget_storefront();
        if let Some(c) = &self.cache {
            c.forget_personal();
        }
        // Apple songs heard this session (the History panel); local ones stay.
        self.history.tracks.retain(|(_, s)| s.is_local());
        self.back_stack.retain(|r| !r.personal());
        if self.route.personal() {
            self.route = Route::Home;
            self.ui.route_changed();
            self.remember_route();
        }
        let pins = self.settings.pins.len();
        self.settings.pins.retain(|p| !personal_id(&p.id) && !Artist::is_library_id(&p.id));
        if self.settings.pins.len() != pins {
            self.save_settings();
        }
    }

    // ---------------------------------------------------------------- first sync

    /// Downloads the whole library (songs, albums, playlists) for the intro's sync screen, as
    /// soon as the bridge is up and signed in. The lists save themselves to disk when complete.
    pub fn sync_library(&mut self) {
        if !self.session.bridge_up || !self.session.authorized {
            return;
        }
        self.syncing = true;
        self.ensure_library_albums();
        self.ensure_library_songs();
        self.load_route_data(); // playlists (always fetched when signed in) and the start page
    }

    /// The download so far, per list (songs, albums, playlists): items in, and the list's
    /// length once Apple said it.
    pub fn sync_counts(&self) -> [(usize, Option<usize>); 3] {
        fn of<T>(l: &Paged<T>) -> (usize, Option<usize>) {
            let have = if l.refreshing { l.buf.len() } else { l.items.len() };
            // A finished list is whole, whatever the first page promised.
            let total = if l.load == Load::Ready(()) && !l.refreshing { Some(have) } else { l.total.map(|t| t.max(have)) };
            (have, total)
        }
        [of(&self.library_songs), of(&self.library_albums), of(&self.library_playlists)]
    }

    /// 0..1 across all three lists, once every list's length is known.
    pub fn sync_fraction(&self) -> Option<f32> {
        let counts = self.sync_counts();
        let total: usize = counts.iter().map(|(_, t)| *t).sum::<Option<usize>>()?;
        let have: usize = counts.iter().map(|(h, _)| *h).sum();
        Some(if total == 0 { 1.0 } else { (have as f32 / total as f32).min(1.0) })
    }

    /// Songs, albums and playlists are all in (or failed): the sync screen can go.
    pub fn library_synced(&self) -> bool {
        let done = |l: &Load<()>| matches!(l, Load::Ready(_) | Load::Failed(_));
        done(&self.library_songs.load) && done(&self.library_albums.load) && done(&self.library_playlists.load)
    }

    /// What the sync fetches after the lists: (playlists done, of) and (covers done, of).
    pub fn sync_rest(&self) -> [(usize, usize); 2] {
        [(self.pl_sync.done, self.pl_sync.total), self.art.warm_progress()]
    }

    /// The lists are in, and so is every playlist's songs and their covers: the sync is over.
    pub fn sync_complete(&self) -> bool {
        let [_, (covers, of)] = self.sync_rest();
        self.library_synced() && self.pl_sync.idle() && covers >= of
    }

    /// The playlists list is in: fetch each playlist's songs for the disk, and their covers.
    /// The first sync takes every playlist; later starts only those with nothing on disk
    /// (made elsewhere since), looked up off the UI thread. A changed playlist is still
    /// refreshed when opened (`load_collection`).
    pub(super) fn start_playlist_sync(&mut self) {
        let ids: Vec<String> = self.library_playlists.items.iter().map(|c| c.id.clone()).collect();
        if self.syncing {
            self.queue_playlist_sync(ids);
            return;
        }
        let (Some(cache), Some(handle)) = (self.cache.clone(), self.handle.clone()) else { return };
        std::thread::spawn(move || {
            let dir = cache.dir().to_path_buf();
            let missing: Vec<String> = ids.into_iter().filter(|id| !dir.join(Cache::collection_name(id)).is_file()).collect();
            if !missing.is_empty() {
                handle.update(move |s: &mut State, _tx| s.queue_playlist_sync(missing));
            }
        });
    }

    pub(super) fn queue_playlist_sync(&mut self, ids: Vec<String>) {
        let ps = &mut self.pl_sync;
        for id in ids {
            if ps.seen.insert(id.clone()) {
                ps.todo.push_back(id);
                ps.total += 1;
            }
        }
        self.pump_playlist_sync();
    }

    fn pump_playlist_sync(&mut self) {
        while self.pl_sync.active.len() < PLAYLIST_SYNC_PARALLEL {
            let Some(id) = self.pl_sync.todo.pop_front() else { break };
            // Opened this session: its page loads and saves it.
            if self.collections.contains_key(&id) || self.coll_refresh.contains_key(&id) {
                self.pl_sync.done += 1;
                continue;
            }
            self.pl_sync.active.insert(id.clone(), None);
            self.request(api::playlist(&id), first_page, move |r| Msg::SyncPlaylist { id, result: r.and_then(|x| x) });
        }
    }

    /// A playlist's first page for the sync. Returns whether the sync's progress moved where
    /// it shows.
    pub(super) fn sync_playlist_first(&mut self, id: String, result: Result<(Collection, Option<String>), String>) -> bool {
        if !self.pl_sync.active.contains_key(&id) {
            return false; // signed out since
        }
        match result {
            Ok((c, Some(next))) => {
                self.request_sync_tracks(&id, &next);
                self.pl_sync.active.insert(id, Some(c));
                return false;
            }
            Ok((c, None)) => self.finish_playlist_sync(id, Some(c)),
            Err(_) => self.finish_playlist_sync(id, None),
        }
        self.syncing
    }

    /// More of a playlist's tracks for the sync.
    pub(super) fn sync_playlist_tracks(&mut self, id: String, result: Result<Page<Song>, String>) -> bool {
        let Some(Some(mut c)) = self.pl_sync.active.remove(&id) else { return false };
        match result {
            Ok(page) => {
                c.tracks.extend(page.items);
                match page.next {
                    Some(next) if c.tracks.len() < TRACKS_CAP => {
                        self.request_sync_tracks(&id, &next);
                        self.pl_sync.active.insert(id, Some(c));
                        return false;
                    }
                    _ => self.finish_playlist_sync(id, Some(c)),
                }
            }
            // Not saved: a partial copy would read as current. Its page fetches it when opened.
            Err(_) => self.finish_playlist_sync(id, None),
        }
        self.syncing
    }

    fn request_sync_tracks(&self, id: &str, next: &str) {
        let id = id.to_string();
        self.request(api::next(next, 100), |v| Page::parse(v, Song::parse), move |result| Msg::SyncPlaylistTracks { id, result });
    }

    /// One playlist of the sync is in (or failed): save it, queue its covers (the page's and
    /// each row's), and start the next.
    fn finish_playlist_sync(&mut self, id: String, c: Option<Collection>) {
        self.pl_sync.active.remove(&id);
        self.pl_sync.done += 1;
        if let Some(c) = c {
            // Its page may have loaded it meanwhile, and saved that copy.
            if !matches!(self.collections.get(&id), Some(Load::Ready(_))) {
                self.save_collection(&c);
            }
            let page = c.artwork.as_ref().map(|a| a.url(crate::art::HEADER_PX));
            let rows = c.tracks.iter().filter_map(|t| t.artwork.as_ref()).map(|a| a.url(crate::art::ROW_PX));
            self.art.warm(page.into_iter().chain(rows));
        }
        self.pump_playlist_sync();
    }

    /// Signed in just now with nothing on disk: show the sync screen while the library loads.
    /// Only in the real app with its window up (not tests, headless runs or `--hidden`).
    pub(super) fn begin_sync(&mut self) {
        // Under the onboarding the sync runs behind it (its sign-in step shows the counts); the
        // sync pill takes over once the app shows.
        if self.handle.is_some() && !self.background && self.ui.onboarding.is_none() {
            crate::ui::intro::begin_sync(self);
        }
        self.sync_library();
    }

    // ---------------------------------------------------------------- disk cache

    /// Shows what the disk cache holds (read at startup on a background thread). Personal data
    /// appears once the bridge says we're signed in, or at once if the last session was.
    pub fn apply_cached(&mut self, c: Cached) -> bool {
        let show = if self.session.bridge_up { self.session.authorized } else { c.authorized };
        if !show {
            return false;
        }
        self.session.authorized = true;
        self.library_songs.seed(c.songs);
        self.library_albums.seed(c.albums);
        self.library_playlists.seed(c.playlists);
        for (i, shelf) in c.shelves.into_iter().enumerate() {
            if let (Load::Idle, Some(shelf)) = (&self.shelves[i], shelf) {
                self.shelves[i] = Load::Ready(shelf);
                self.shelf_stale[i] = true;
            }
        }
        self.load_route_data();
        true
    }

    /// A detail page's cache file was read (or wasn't there).
    pub fn apply_cached_collection(&mut self, id: String, cached: Option<Stored<Collection>>) -> bool {
        match cached {
            Some(s) if !matches!(self.collections.get(&id), Some(Load::Ready(_))) => {
                self.coll_cache.insert(id.clone(), CollCache::Cached(s.saved_at));
                self.collections.insert(id, Load::Ready(s.data));
            }
            _ => {
                self.coll_cache.insert(id, CollCache::Missing);
            }
        }
        self.load_route_data();
        true
    }

    /// Starts reading the current detail page's cache file (no bridge needed).
    fn lookup_collection_cache(&mut self) {
        let Route::Collection { id, .. } = &self.route else { return };
        let (Some(cache), Some(handle)) = (&self.cache, &self.handle) else { return };
        if personal_id(id) && !self.session.authorized {
            return; // signed out: the account's pages are gone from disk too
        }
        if self.coll_cache.contains_key(id) || matches!(self.collections.get(id), Some(Load::Ready(_))) {
            return;
        }
        self.coll_cache.insert(id.clone(), CollCache::Reading);
        self.collections.entry(id.clone()).or_insert(Load::Loading);
        let (cache, handle, id) = (cache.clone(), handle.clone(), id.clone());
        std::thread::spawn(move || {
            let cached = cache.read_collection(&id);
            handle.update(move |s: &mut State, tx| {
                s.landing(tx);
                s.apply_cached_collection(id, cached);
            });
        });
    }

    /// Decides what the current detail page needs from Apple, given the cache.
    fn load_collection(&mut self, kind: CollectionKind, id: &str) {
        match self.coll_cache.get(id).copied() {
            Some(CollCache::Reading) => {}
            Some(CollCache::Cached(saved_at)) => {
                let Some(Load::Ready(c)) = self.collections.get(id) else { return };
                let current = if id.starts_with("p.") {
                    // Library playlists change often: compare against the list we just re-fetched,
                    // so wait for it.
                    if !self.library_playlists.is_fresh() {
                        return;
                    }
                    let listed = self.library_playlists.items.iter().find(|p| p.id == id);
                    let current = cache::playlist_current(c.modified.as_deref(), listed.and_then(|p| p.modified.as_deref()));
                    // A picture someone uploaded is a signed link that dies after a day: the page's
                    // saved one is dead by now, the list's was just issued.
                    let fresh = listed.and_then(|p| p.artwork.clone());
                    if let (Some(fresh), Some(Load::Ready(c))) = (fresh, self.collections.get_mut(id)) {
                        c.artwork = Some(fresh);
                    }
                    current
                } else {
                    // An album saved before artist ids were kept (or without their names) is
                    // asked for again, once a session, so its artist name can link.
                    cache::now_secs().saturating_sub(saved_at) < COLLECTION_TTL_SECS && !c.lacks_artists()
                };
                self.coll_cache.insert(id.to_string(), CollCache::Verified);
                if !current {
                    self.coll_refresh.insert(id.to_string(), None);
                    self.fetch_collection(kind, id);
                }
            }
            Some(CollCache::Verified) => {
                if matches!(self.collections.get(id), Some(Load::Failed(_))) {
                    self.fetch_collection(kind, id);
                }
            }
            None | Some(CollCache::Missing) => {
                let seeded = matches!(self.collections.get(id), Some(Load::Ready(_)));
                self.coll_cache.insert(id.to_string(), CollCache::Verified);
                if !seeded {
                    self.fetch_collection(kind, id);
                }
            }
        }
    }

    fn fetch_collection(&mut self, kind: CollectionKind, id: &str) {
        if !self.coll_refresh.contains_key(id) {
            self.collections.insert(id.to_string(), Load::Loading);
        }
        let req = match kind {
            CollectionKind::Album => api::album(id),
            CollectionKind::Playlist => api::playlist(id),
        };
        let reply_id = id.to_string();
        self.request(req, first_page, move |r| Msg::Collection { id: reply_id, result: r.and_then(|x| x) });
    }

    /// The fresh copy of a detail page is complete: swap it in and save it.
    pub(super) fn finish_refresh(&mut self, id: String, c: Collection) {
        self.coll_refresh.remove(&id);
        self.save_collection(&c);
        self.collections.insert(id, Load::Ready(c));
    }

    pub(super) fn save<T: serde::Serialize + Send + 'static>(&self, name: &str, data: T) {
        if let Some(c) = &self.cache {
            c.save(name, data);
        }
    }

    pub(super) fn save_collection(&self, c: &Collection) {
        self.save(&Cache::collection_name(&c.id), c.clone());
    }

    pub(super) fn save_shelves(&self) {
        let shelves: [Option<Shelf>; 2] = std::array::from_fn(|i| self.shelves[i].ready().cloned());
        self.save(cache::SHELVES, shelves);
    }

    // ---------------------------------------------------------------- requests

    /// API request whose reply is parsed on the reader thread into a `Msg`.
    pub(super) fn request<T: Send + 'static>(
        &self,
        (path, params): (String, Value),
        parse: impl FnOnce(&Value) -> T + Send + 'static,
        wrap: impl FnOnce(Result<T, String>) -> Msg + Send + 'static,
    ) {
        #[cfg(test)]
        self.sent.borrow_mut().push(format!("{path} {params}"));
        let account = api::is_personal(&path, &params).then_some(self.session.account);
        if let Some(b) = &self.bridge {
            b.api(&path, params, move |r| Some(stamp(account, wrap(r.map(|v| parse(&v))))));
        }
    }

    pub(super) fn request_tracks(&self, id: &str, next: &str) {
        let id = id.to_string();
        self.request(api::next(next, 100), |v| Page::parse(v, Song::parse), move |result| Msg::CollectionTracks {
            id,
            result,
        });
    }

    /// Starts whatever the current route (and the sidebar) need and haven't loaded yet. Cached
    /// lists are checked against Apple once per session (see `cache.rs` for the rules).
    /// Loads library albums once; a list shown from the disk cache is checked for changes once.
    fn ensure_library_albums(&mut self) {
        let l = &mut self.library_albums;
        if l.load == Load::Idle {
            l.load = Load::Loading;
            self.request(api::library_albums(), |v| Page::parse(v, Collection::parse), Msg::LibraryAlbums);
        } else if std::mem::take(&mut l.stale) {
            self.request(api::newest("albums"), newest, |result| Msg::LibraryNewest { songs: false, result });
        }
    }

    fn ensure_library_songs(&mut self) {
        let l = &mut self.library_songs;
        if l.load == Load::Idle {
            l.load = Load::Loading;
            self.request(api::library_songs(), |v| Page::parse(v, Song::parse), Msg::LibrarySongs);
        } else if std::mem::take(&mut l.stale) {
            self.request(api::newest("songs"), newest, |result| Msg::LibraryNewest { songs: true, result });
        }
    }

    fn ensure_library_artists(&mut self) {
        if self.library_artists.load == Load::Idle {
            self.library_artists.load = Load::Loading;
            self.request(api::library_artists(), |v| Page::parse(v, Artist::parse), Msg::LibraryArtists);
        }
    }

    pub fn load_route_data(&mut self) {
        // Local albums, artists and folders come from the music folders, never from Apple.
        self.fill_local_page();
        if self.route.local_library() {
            return;
        }
        self.lookup_collection_cache();
        self.favourite_lookups();
        // Headless runs have no bridge; tests have none either but still record requests.
        if (self.bridge.is_none() && !cfg!(test)) || !self.session.bridge_up {
            return;
        }
        let me = self.session.authorized;
        // The sidebar lists library playlists on every page.
        if me {
            let l = &mut self.library_playlists;
            // Always re-fetched (one page); a cached list stays shown until the new one lands.
            let fetch = l.load == Load::Idle || l.stale;
            if l.load == Load::Idle {
                l.load = Load::Loading;
            } else if l.stale {
                l.refreshing = true;
            }
            l.stale = false;
            if fetch {
                self.request(api::library_playlists(), |v| Page::parse(v, Collection::parse), Msg::LibraryPlaylists);
            }
        }
        match self.route.clone() {
            Route::Home if me => {
                let requests = [api::recently_played(), api::recently_added()];
                for (index, req) in requests.into_iter().enumerate() {
                    let idle = self.shelves[index] == Load::Idle;
                    if idle || self.shelf_stale[index] {
                        if idle {
                            self.shelves[index] = Load::Loading;
                        }
                        self.shelf_stale[index] = false;
                        self.request(req, |v| Page::parse(v, Collection::parse).items, move |result| Msg::Shelf {
                            index,
                            result,
                        });
                    }
                }
            }
            Route::Library(LibraryTab::Albums) if me => self.ensure_library_albums(),
            Route::Library(LibraryTab::Songs) if me => self.ensure_library_songs(),
            // Library search runs in-app over the loaded lists, so make sure they all load.
            Route::Search if me => {
                self.ensure_library_albums();
                self.ensure_library_songs();
                self.ensure_library_artists();
            }
            Route::Collection { kind, id } if me || !personal_id(&id) => self.load_collection(kind, &id),
            Route::New if self.browse_new == Load::Idle => {
                self.browse_new = Load::Loading;
                self.request(api::editorial(), crate::models::parse_editorial, |result| Msg::Browse {
                    page: BrowsePage::New,
                    result,
                });
            }
            Route::MadeForYou if me && self.made_for_you == Load::Idle => {
                self.made_for_you = Load::Loading;
                self.request(api::recommendations(), crate::models::parse_recommendations, |result| Msg::Browse {
                    page: BrowsePage::MadeForYou,
                    result,
                });
            }
            Route::Radio => {
                let reqs = [
                    (api::live_radio(), BrowsePage::LiveRadio, "Live radio"),
                    (api::personal_station(), BrowsePage::PersonalStation, "Your station"),
                ];
                for (i, (req, page, title)) in reqs.into_iter().enumerate() {
                    if self.radio[i] == Load::Idle && (me || page == BrowsePage::LiveRadio) {
                        self.radio[i] = Load::Loading;
                        self.request(
                            req,
                            move |v| Shelf::from_resources(title, v["data"].as_array().map(Vec::as_slice).unwrap_or(&[])).into_iter().collect(),
                            move |result| Msg::Browse { page, result },
                        );
                    }
                }
            }
            Route::RecentlyAdded if me && self.recently_added.load == Load::Idle => {
                self.recently_added.load = Load::Loading;
                self.request(api::recently_added(), |v| Page::parse(v, Collection::parse), Msg::RecentlyAdded);
            }
            Route::Artists if me => self.ensure_library_artists(),
            Route::Artist { id, .. } => self.ensure_artist(&id, me),
            Route::ArtistShelf { id, shelf, .. } => {
                self.ensure_artist(&id, me);
                self.artist_see_all(&id, shelf);
            }
            _ => {}
        }
    }
}

/// A library album ("l.…") or playlist ("p.…") id: the account's own.
pub(super) fn personal_id(id: &str) -> bool {
    id.starts_with("l.") || id.starts_with("p.")
}

/// A reply about the account, stamped with the `Session::account` it was asked under (None:
/// not about the account, passed as it is).
pub(super) fn stamp(account: Option<u64>, msg: Msg) -> Msg {
    match account {
        Some(generation) => Msg::Account { generation, msg: Box::new(msg) },
        None => msg,
    }
}

/// Reads the probe reply: `meta.total` and the ids of the (up to 10) newest items.
fn newest(v: &Value) -> (Option<u64>, Vec<String>) {
    let ids = v["data"].as_array().map(|d| d.iter().filter_map(|x| x["id"].as_str().map(String::from)).collect());
    (v["meta"]["total"].as_u64(), ids.unwrap_or_default())
}

/// Appends a page; returns the next page's path when more should be fetched.
pub(super) fn append_page<T>(list: &mut Paged<T>, result: Result<Page<T>, String>) -> Option<String> {
    match result {
        Ok(page) => {
            if let Some(total) = page.total {
                list.total = Some((total as usize).min(LIBRARY_CAP));
            }
            // A refresh collects its pages aside and swaps them in at the end.
            let refreshing = list.refreshing;
            let dest = if refreshing { &mut list.buf } else { &mut list.items };
            dest.extend(page.items);
            match page.next {
                Some(next) if dest.len() < LIBRARY_CAP => {
                    if !refreshing {
                        list.load = Load::Loading;
                    }
                    Some(next)
                }
                _ => {
                    if refreshing {
                        list.items = std::mem::take(&mut list.buf);
                        list.refreshing = false;
                    }
                    list.load = Load::Ready(());
                    None
                }
            }
        }
        Err(e) if list.refreshing => {
            // Keep the cached items, and look again at the next opportunity.
            let _ = e;
            list.buf.clear();
            list.refreshing = false;
            list.stale = true;
            None
        }
        Err(e) => {
            list.load = Load::Failed(e);
            None
        }
    }
}

/// A detail page's first reply: the collection, and the path of its next page of tracks.
fn first_page(v: &Value) -> Result<(Collection, Option<String>), String> {
    let d = &v["data"][0];
    let next = d["relationships"]["tracks"]["next"].as_str().map(String::from);
    Collection::parse(d).map(|c| (c, next)).ok_or_else(|| "not found".to_string())
}

/// Playlists the sync fetches at once.
const PLAYLIST_SYNC_PARALLEL: usize = 2;

/// The library sync's second half (`State::start_playlist_sync`): each library playlist's
/// songs, saved to disk so it opens at once, then its covers (`Art::warm`). Nothing is kept
/// in memory once saved.
#[derive(Default)]
pub(super) struct PlaylistSync {
    /// Still to fetch.
    todo: VecDeque<String>,
    /// Being fetched, with its pages so far (None until the first arrives).
    active: HashMap<String, Option<Collection>>,
    /// Every playlist queued this session, so each is fetched once.
    seen: HashSet<String>,
    /// Finished (saved, skipped or failed), of all queued: the sync's progress.
    done: usize,
    total: usize,
}

impl PlaylistSync {
    fn idle(&self) -> bool {
        self.todo.is_empty() && self.active.is_empty()
    }
}
