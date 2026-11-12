//! Library edits beyond adding to a playlist (`actions.rs`) and creating one (`playlists.rs`):
//! adding catalog songs, albums and playlists to the library, and removing songs from one of
//! the user's playlists.
//!
//! Adding is Apple's documented `POST /v1/me/library?ids[songs]=…`. Removing is not in the
//! public API; music.apple.com does it with `DELETE /v1/me/library/playlists/{id}/tracks
//! ?ids[library-songs]=…&mode=all` (every copy of each song leaves that playlist), and rewrites
//! the list with `PUT /v1/me/library/playlists/{id}/tracks` `{"data":[{id,type},…]}` (reorders;
//! the app uses it to remove one copy of a song listed twice; both checked live 2026-11-10).
//! `mode=first` also exists but removes the first copy, not the selected one: never used. For
//! the library itself, `DELETE /v1/me/library/{songs|albums|playlists}/{id}`, one item a request.

use super::*;

impl State {
    /// "Add to Library" on catalog songs (search results, catalog albums and playlists).
    pub fn add_songs_to_library(&mut self, songs: &[Song]) {
        let ids: Vec<String> = songs
            .iter()
            .filter(|s| s.playable && !s.is_local())
            .filter_map(|s| s.catalog_id.clone().or_else(|| (!s.id.starts_with("i.")).then(|| s.id.clone())))
            .collect();
        let what = match songs {
            [one] => format!("“{}”", one.title),
            _ => format!("{} songs", ids.len()),
        };
        self.add_to_library("songs", ids, what);
    }

    /// "Add to Library" on a catalog album or playlist card or page.
    pub fn add_collection_to_library(&mut self, c: &Collection) {
        if in_library(&c.id) {
            self.toast = Some(format!("“{}” is already in your library.", c.title));
            return;
        }
        let kind = match c.kind {
            CollectionKind::Album => "albums",
            CollectionKind::Playlist => "playlists",
        };
        self.add_to_library(kind, vec![c.id.clone()], format!("“{}”", c.title));
    }

    fn add_to_library(&mut self, kind: &str, mut ids: Vec<String>, what: String) {
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            self.toast = Some("Already in your library.".into());
            return;
        }
        if !self.session.authorized {
            self.toast = Some("Sign in to Apple Music to add to your library.".into());
            return;
        }
        let path = format!("/v1/me/library?ids[{kind}]={}", ids.join(","));
        self.write("POST", path, None, move |result| Msg::AddedToLibrary { what, result });
    }

    /// Apple answered "Add to Library": the library lists are out of date now.
    pub(super) fn added_to_library(&mut self, what: String, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.toast = Some(format!("Added {what} to your library"));
                // The next visit checks them against Apple (cheap "newest" probe, then a reload).
                self.library_songs.stale = true;
                self.library_albums.stale = true;
                self.library_playlists.stale = true;
                self.recently_added = Paged::default();
                self.load_route_data();
            }
            Err(e) => self.toast = Some(format!("Couldn't add {what} to your library: {e}")),
        }
    }

    /// Removes the selected rows `rows` (each with its position in the playlist's own order,
    /// whatever the page's sort or filter) from the user's playlist `playlist`. The page
    /// updates at once; if Apple refuses, it reloads from Apple. A row that wasn't selected is
    /// never removed.
    ///
    /// When the rows hold every copy of each of their songs, one `DELETE … mode=all` does it.
    /// Otherwise (one copy of a song listed twice) the app reads Apple's whole list, checks it
    /// is the one on screen, and writes it back without those positions (`PUT`); if Apple's
    /// list differs, nothing is written and the page reloads.
    pub fn remove_from_playlist(&mut self, playlist: &Collection, rows: &[(usize, Song)]) {
        if !playlist.editable || !playlist.id.starts_with("p.") {
            self.toast = Some("Only your own playlists can be edited.".into());
            return;
        }
        let rows: Vec<&(usize, Song)> = rows.iter().filter(|(_, s)| s.id.starts_with("i.")).collect();
        if rows.is_empty() {
            self.toast = Some("These songs can't be removed from here.".into());
            return;
        }
        let (id, name) = (playlist.id.clone(), playlist.title.clone());
        if self.pl_rewrites.contains_key(&id) {
            self.toast = Some(format!("Still saving the last change to “{name}”."));
            return;
        }
        // The page's list (Apple's order); each row must still be where it was.
        let tracks = self.collections.get(&id).and_then(|l| l.ready()).map_or(&playlist.tracks, |c| &c.tracks);
        let mut drop: Vec<usize> = rows.iter().map(|(i, _)| *i).collect();
        drop.sort_unstable();
        drop.dedup();
        if rows.iter().any(|(i, s)| tracks.get(*i).is_none_or(|t| t.id != s.id)) {
            self.toast = Some(format!("“{name}” changed. Try again."));
            return;
        }
        let shown: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
        let mut ids: Vec<String> = drop.iter().map(|&i| shown[i].clone()).collect();
        ids.sort();
        ids.dedup();
        let every_copy = ids.iter().all(|x| drop.iter().filter(|&&i| shown[i] == *x).count() == shown.iter().filter(|t| *t == x).count());
        let count = drop.len();
        if let Some(Load::Ready(c)) = self.collections.get_mut(&id) {
            for &i in drop.iter().rev() {
                c.tracks.remove(i);
            }
            if let Some(n) = c.track_count.as_mut() {
                *n = n.saturating_sub(count as u32);
            }
            let c = c.clone();
            self.save_collection(&c);
        }
        self.ui.sel.clear();
        if every_copy {
            let path = format!("/v1/me/library/playlists/{id}/tracks?ids[library-songs]={}&mode=all", ids.join(","));
            self.write("DELETE", path, None, move |result| Msg::RemovedFromPlaylist { id, name, count, result });
        } else {
            self.pl_rewrites.insert(id.clone(), Rewrite { name, shown, drop, entries: Vec::new() });
            self.request_entries(&id, &format!("/v1/me/library/playlists/{id}/tracks"));
        }
    }

    /// One page of a playlist's entries, as music.apple.com reads them before a reorder.
    fn request_entries(&self, id: &str, path: &str) {
        let id = id.to_string();
        let parse = |v: &serde_json::Value| Page::parse(v, |e| Some((e["id"].as_str()?.to_string(), e["type"].as_str()?.to_string())));
        self.request((path.to_string(), serde_json::json!({"limit": 100, "omit[resource]": "autos"})), parse, move |result| {
            Msg::PlaylistEntries { id, result }
        });
    }

    /// A page of the entries `remove_from_playlist` asked for. After the last page: if Apple's
    /// list is the one that was on screen, write it back without the removed positions, each
    /// entry with the type Apple gave it; else reload the page and write nothing.
    pub(super) fn playlist_entries(&mut self, id: String, result: Result<Page<(String, String)>, String>) {
        let Some(mut rw) = self.pl_rewrites.remove(&id) else { return };
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                self.toast = Some(format!("Couldn't remove from “{}”: {e}", rw.name));
                self.reload_collection(&id);
                return;
            }
        };
        rw.entries.extend(page.items);
        if let Some(next) = page.next.filter(|_| rw.entries.len() <= rw.shown.len()) {
            self.request_entries(&id, &next);
            self.pl_rewrites.insert(id, rw);
            return;
        }
        let same = rw.entries.len() == rw.shown.len() && rw.entries.iter().zip(&rw.shown).all(|((a, _), b)| a == b);
        if !same {
            self.toast = Some(format!("“{}” changed on Apple Music, so nothing was removed. It has been reloaded.", rw.name));
            self.reload_collection(&id);
            return;
        }
        let data: Vec<serde_json::Value> = rw
            .entries
            .iter()
            .enumerate()
            .filter(|(i, _)| rw.drop.binary_search(i).is_err())
            .map(|(_, (eid, kind))| serde_json::json!({"id": eid, "type": kind}))
            .collect();
        let (name, count) = (rw.name, rw.drop.len());
        let path = format!("/v1/me/library/playlists/{id}/tracks");
        self.write("PUT", path, Some(serde_json::json!({ "data": data })), move |result| Msg::RemovedFromPlaylist { id, name, count, result });
    }

    pub(super) fn removed_from_playlist(&mut self, id: String, name: String, count: usize, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.toast = Some(match count {
                    1 => format!("Removed from “{name}”"),
                    n => format!("Removed {n} songs from “{name}”"),
                });
            }
            Err(e) => {
                self.toast = Some(format!("Couldn't remove from “{name}”: {e}"));
                self.reload_collection(&id);
            }
        }
    }

    /// Forgets the copy of playlist `id` (memory and disk) and loads what Apple really has.
    fn reload_collection(&mut self, id: &str) {
        self.collections.remove(id);
        self.coll_cache.remove(id);
        if let Some(c) = &self.cache {
            c.forget_collection(id);
        }
        self.load_route_data();
    }

    /// "Remove from Library" on library songs: gone from the library on every device.
    pub fn remove_songs_from_library(&mut self, songs: &[Song]) {
        let ids: Vec<(String, String)> =
            songs.iter().filter(|s| s.id.starts_with("i.")).map(|s| (s.id.clone(), s.title.clone())).collect();
        if ids.is_empty() {
            self.toast = Some("Only songs in your library can be removed from it.".into());
            return;
        }
        for (id, title) in ids {
            self.remove_library_item("songs", id, title);
        }
    }

    /// "Remove from Library" on a library album, or "Delete Playlist" on one of the user's.
    pub fn remove_collection_from_library(&mut self, c: &Collection) {
        let kind = match c.kind {
            CollectionKind::Album if c.id.starts_with("l.") => "albums",
            CollectionKind::Playlist if c.id.starts_with("p.") => "playlists",
            _ => {
                self.toast = Some(format!("“{}” isn't in your library.", c.title));
                return;
            }
        };
        self.remove_library_item(kind, c.id.clone(), c.title.clone());
    }

    fn remove_library_item(&mut self, kind: &'static str, id: String, title: String) {
        if !self.session.authorized {
            self.toast = Some("Sign in to Apple Music to change your library.".into());
            return;
        }
        let path = format!("/v1/me/library/{kind}/{id}");
        self.write("DELETE", path, None, move |result| Msg::RemovedFromLibrary { kind, id, title, result });
    }

    /// Apple removed (or refused to remove) a library item: drop it from the lists on screen.
    pub(super) fn removed_from_library(&mut self, kind: &str, id: String, title: String, result: Result<(), String>) {
        if let Err(e) = result {
            self.toast = Some(format!("Couldn't remove “{title}”: {e}"));
            return;
        }
        match kind {
            "songs" => {
                self.library_songs.items.retain(|s| s.id != id);
                self.save(cache::SONGS, self.library_songs.items.clone());
                for load in self.collections.values_mut() {
                    if let Load::Ready(c) = load {
                        c.tracks.retain(|t| t.id != id);
                    }
                }
            }
            "albums" => {
                self.library_albums.items.retain(|c| c.id != id);
                self.save(cache::ALBUMS, self.library_albums.items.clone());
            }
            _ => {
                self.library_playlists.items.retain(|c| c.id != id);
                self.save(cache::PLAYLISTS, self.library_playlists.items.clone());
                self.settings.pins.retain(|p| p.id != id);
                self.remove_playlist_picture(&id);
                self.save_settings();
            }
        }
        self.recently_added.items.retain(|c| c.id != id);
        self.collections.remove(&id);
        if let Some(c) = &self.cache {
            c.forget_collection(&id);
        }
        if matches!(&self.route, Route::Collection { id: open, .. } if *open == id) {
            self.back();
        }
        self.toast = Some(format!("Removed “{title}” from your library"));
    }

    /// A write through the bridge (recorded in tests).
    pub(super) fn write(&self, method: &str, path: String, body: Option<serde_json::Value>, done: impl FnOnce(Result<(), String>) -> Msg + Send + 'static) {
        #[cfg(test)]
        self.sent.borrow_mut().push(match &body {
            Some(b) => format!("{method} {path} {b}"),
            None => format!("{method} {path}"),
        });
        if let Some(b) = &self.bridge {
            b.api_send(method, &path, body, move |r| Some(done(r.map(|_| ()))));
        }
    }
}

/// A pending single-copy removal (`State::remove_from_playlist`): the list that was on screen,
/// the positions to drop (sorted) and Apple's entries read so far.
pub(super) struct Rewrite {
    name: String,
    shown: Vec<String>,
    drop: Vec<usize>,
    entries: Vec<(String, String)>,
}

/// Library ids: albums "l.…", playlists "p.…", songs "i.…" (catalog ids are digits / "pl.…").
pub fn in_library(id: &str) -> bool {
    id.starts_with("l.") || id.starts_with("p.") || id.starts_with("i.")
}
