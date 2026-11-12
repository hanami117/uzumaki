//! Creating library playlists (Ctrl+N, the sidebar's +, "Add to Playlist → New Playlist…").
//! One POST to `/v1/me/library/playlists` through the bridge; the songs, when given, go in the
//! same request. Local files can't be in Apple playlists, so they are left out.

use serde_json::{Value, json};

use super::*;

/// Apple's references for `songs` (playable Apple songs only): the catalog id when known,
/// else the library id.
pub(super) fn song_refs(songs: &[Song]) -> Vec<Value> {
    songs
        .iter()
        .filter(|s| s.playable && !s.is_local())
        .map(|s| match &s.catalog_id {
            Some(cat) => json!({"id": cat, "type": "songs"}),
            None if s.id.starts_with("i.") => json!({"id": s.id, "type": "library-songs"}),
            None => json!({"id": s.id, "type": "songs"}),
        })
        .collect()
}

/// The request body for a new playlist named `name`.
pub(super) fn new_playlist_body(name: &str, description: &str, songs: &[Song]) -> Value {
    let mut attributes = json!({ "name": name });
    if !description.trim().is_empty() {
        attributes["description"] = json!(description.trim());
    }
    let mut body = json!({ "attributes": attributes });
    let refs = song_refs(songs);
    if !refs.is_empty() {
        body["relationships"] = json!({ "tracks": { "data": refs } });
    }
    body
}

impl State {
    /// Creates a library playlist named `name` (with `songs` in it, if any). The answer arrives
    /// as `Msg::PlaylistCreated`; meanwhile `creating_playlist` is true.
    pub fn create_playlist(&mut self, name: &str, description: &str, songs: &[Song]) {
        let name = name.trim().to_string();
        if name.is_empty() || self.creating_playlist {
            return;
        }
        if !self.session.authorized {
            self.toast = Some("Sign in to Apple Music to create playlists.".into());
            return;
        }
        let body = new_playlist_body(&name, description, songs);
        let apple: Vec<Song> = songs.iter().filter(|s| s.playable && !s.is_local()).cloned().collect();
        if apple.len() < songs.len() {
            self.toast = Some("Local files can't be in Apple Music playlists; they were left out.".into());
        }
        #[cfg(test)]
        self.sent.borrow_mut().push(format!("POST /v1/me/library/playlists {body}"));
        let Some(b) = &self.bridge else { return };
        self.creating_playlist = true;
        b.api_post("/v1/me/library/playlists", body, move |r| {
            let result = r.and_then(|v| v["data"].get(0).and_then(Collection::parse).ok_or_else(|| "Apple sent no playlist back".to_string()));
            Some(Msg::PlaylistCreated { name, songs: apple, result })
        });
    }

    /// Apple created the playlist: list it first in the sidebar, open it, and say so.
    pub(super) fn playlist_created(&mut self, name: String, songs: Vec<Song>, result: Result<Collection, String>) {
        self.creating_playlist = false;
        match result {
            Ok(mut c) => {
                if c.title.is_empty() {
                    c.title = name.clone();
                }
                c.editable = true;
                if c.tracks.is_empty() {
                    c.tracks = songs;
                }
                self.library_playlists.items.retain(|p| p.id != c.id);
                self.library_playlists.items.insert(0, Collection { tracks: Vec::new(), ..c.clone() });
                self.save(cache::PLAYLISTS, self.library_playlists.items.clone());
                self.toast = Some(format!("Created “{name}”"));
                if let Some(src) = self.new_playlist_picture.take() {
                    self.set_playlist_picture(&c.id, src);
                }
                self.open_collection(&c);
            }
            Err(e) => {
                self.new_playlist_picture = None;
                self.toast = Some(format!("Couldn't create “{name}”: {e}"));
            }
        }
    }
}
