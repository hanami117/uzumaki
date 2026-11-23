//! What `uzu`, the command line, reads from and does to the state (`crate::ctl` runs the socket
//! and calls these on the UI thread). Protocol types are `uzumaki_ctl`'s.

use uzumaki_ctl as ctl;

use super::*;
use crate::cache::Stored;

pub fn song_info(s: &Song) -> ctl::SongInfo {
    ctl::SongInfo {
        id: s.id.clone(),
        title: s.title.clone(),
        artist: s.artist.clone(),
        album: s.album.clone(),
        duration: s.duration,
        explicit: s.explicit,
        local: s.is_local(),
        playable: s.playable,
    }
}

pub fn collection_info(c: &Collection) -> ctl::CollectionInfo {
    ctl::CollectionInfo {
        kind: match c.kind {
            CollectionKind::Album => ctl::Kind::Album,
            CollectionKind::Playlist => ctl::Kind::Playlist,
        },
        id: c.id.clone(),
        title: c.title.clone(),
        subtitle: c.subtitle.clone(),
        count: c.track_count.or_else(|| (!c.tracks.is_empty()).then_some(c.tracks.len() as u32)),
        favorites: c.favorites,
        editable: c.editable,
        year: c.year.clone(),
        pinned: None,
    }
}

pub fn repeat_mode(r: Repeat) -> ctl::RepeatMode {
    match r {
        Repeat::Off => ctl::RepeatMode::Off,
        Repeat::All => ctl::RepeatMode::All,
        Repeat::One => ctl::RepeatMode::One,
    }
}

pub fn repeat_from(m: ctl::RepeatMode) -> Repeat {
    match m {
        ctl::RepeatMode::Off => Repeat::Off,
        ctl::RepeatMode::All => Repeat::All,
        ctl::RepeatMode::One => Repeat::One,
    }
}

fn kind_word(c: &Collection) -> &'static str {
    match c.kind {
        CollectionKind::Album => "album",
        CollectionKind::Playlist => "playlist",
    }
}

fn play_state(p: PlayState) -> ctl::PlayState {
    match p {
        PlayState::Playing => ctl::PlayState::Playing,
        PlayState::Paused => ctl::PlayState::Paused,
        PlayState::Loading | PlayState::Seeking | PlayState::Waiting | PlayState::Stalled => ctl::PlayState::Loading,
        PlayState::None | PlayState::Stopped | PlayState::Ended | PlayState::Completed => ctl::PlayState::Stopped,
    }
}

/// Whether a disk-cached album or playlist can be played as is: library playlists must match
/// the `modified` date in the library list (they change often), the rest are good for a day.
/// The same rules as the detail page (`State::load_collection`).
pub fn cached_is_current(stored: &Stored<Collection>, listed_modified: Option<&str>) -> bool {
    is_current(&stored.data.id, stored.saved_at, stored.data.modified.as_deref(), listed_modified)
}

/// [`cached_is_current`] from the parts: the cached copy's id, save time and `modified`.
pub fn is_current(id: &str, saved_at: u64, cached_modified: Option<&str>, listed_modified: Option<&str>) -> bool {
    if id.starts_with("p.") {
        cache::playlist_current(cached_modified, listed_modified)
    } else {
        cache::now_secs().saturating_sub(saved_at) < COLLECTION_TTL_SECS
    }
}

/// The wire form of `l` (what `Request::Lyrics` answers): plain or synced, every line with its
/// words when Apple timed them.
pub fn lyrics_reply(song_id: &str, l: &crate::lyrics::Lyrics) -> ctl::Reply {
    ctl::Reply::Lyrics {
        song_id: song_id.to_string(),
        status: if l.synced { ctl::LyricsStatus::Synced } else { ctl::LyricsStatus::Plain },
        lines: l
            .lines
            .iter()
            .map(|x| ctl::LyricLine {
                start: x.start,
                end: x.end,
                text: x.text.clone(),
                second_voice: x.second_voice,
                words: x.words.iter().map(|w| ctl::LyricWord { start: w.start, end: w.end, text: w.text.clone() }).collect(),
            })
            .collect(),
    }
}

/// Where song `id` is in `ids` now, for a request that numbered it `index` in a list the client
/// fetched earlier: `index` itself when it still holds that song, else the nearest place that
/// does (not one of `taken`). `None`: the song is gone.
fn locate(ids: &[String], index: usize, id: &str, taken: &[usize]) -> Option<usize> {
    ids.iter().enumerate().filter(|&(k, x)| x == id && !taken.contains(&k)).min_by_key(|&(k, _)| k.abs_diff(index)).map(|(k, _)| k)
}

/// What a request that names a song gone from the queue answers.
const QUEUE_CHANGED: &str = "the queue changed and that song is no longer in it (run uzu queue again)";

fn lyrics_state(song_id: &str, status: ctl::LyricsStatus) -> ctl::Reply {
    ctl::Reply::Lyrics { song_id: song_id.to_string(), status, lines: Vec::new() }
}

impl State {
    /// `uzu`'s lyrics screen: the playing song's lyrics from memory (`want_lyrics` fetches them on
    /// every song change). Only when there is no entry at all, for a signed-in catalog song, it
    /// asks Apple, once (the entry turns `Loading`, so the next poll doesn't ask again).
    pub fn ctl_lyrics(&mut self) -> ctl::Reply {
        use ctl::LyricsStatus as L;
        let Some(song) = self.playback.song.clone() else { return lyrics_state("", L::None) };
        // A local song keeps its lyrics in a `.lrc` beside the file (Apple knows nothing of it).
        if let Some(file) = song.local_path() {
            return match crate::lyrics::read_lrc_beside(file) {
                Some(l) => lyrics_reply(&song.id, &l),
                None => lyrics_state(&song.id, L::None),
            };
        }
        if self.lyrics_of(&song).is_none() {
            self.want_lyrics(&song);
        }
        match self.lyrics_of(&song) {
            Some(Load::Ready(Some(l))) => lyrics_reply(&song.id, l),
            Some(Load::Loading) => lyrics_state(&song.id, L::Loading),
            _ => lyrics_state(&song.id, L::None),
        }
    }

    pub fn ctl_status(&self) -> ctl::Status {
        let p = &self.playback;
        let song = p.song.as_ref();
        // `pos` points at the list song; while a Play Next / Add to Queue song plays it doesn't.
        // The number shown is its place in the list itself, also with shuffle on (like mpc).
        let index = match (song, self.queue.current()) {
            (Some(s), Some(c)) if s.same_track(c) => self.queue.original.iter().position(|o| o.same_track(c)),
            _ => None,
        };
        ctl::Status {
            state: if song.is_some() { play_state(p.state) } else { ctl::PlayState::Stopped },
            song: song.map(song_info),
            position: p.heard_now(),
            duration: p.duration,
            volume: p.volume,
            shuffle: p.shuffle,
            repeat: repeat_mode(p.repeat),
            source: self.queue.source.clone(),
            index,
            count: self.queue.context.len(),
            queued: self.queue.user.len(),
            connected: self.session.bridge_up,
            signed_in: self.session.authorized,
            error: self.session.error.clone(),
            cover: song
                .and_then(|s| s.artwork.as_ref())
                .and_then(crate::art::cached_file)
                .map(|p| p.to_string_lossy().into_owned()),
            autoplay: self.autoplay_offered().then_some(self.settings.autoplay),
            airplay: self.ctl_airplay_now(),
            // The whole answer to `Queue`, so every change of it shows (a client's polls cost a
            // fingerprint, not the list).
            queue_rev: Some(self.ctl_queue().rev()),
        }
    }

    fn ctl_airplay_now(&self) -> Option<ctl::AirPlayNow> {
        use uzumaki_airplay::Cast;
        match &self.airplay.cast {
            Cast::Off => None,
            Cast::Connecting { name, .. } => Some(ctl::AirPlayNow { name: name.clone(), connecting: true, also: self.ctl_airplay_also(name) }),
            Cast::On { name, .. } => Some(ctl::AirPlayNow { name: name.clone(), connecting: false, also: self.ctl_airplay_also(name) }),
        }
    }

    /// The group's other speakers (multi-room), besides `first`.
    fn ctl_airplay_also(&self, first: &str) -> Vec<String> {
        self.airplay.speakers.iter().filter(|s| s.name != first).map(|s| s.name.clone()).collect()
    }

    /// `uzu airplay`: the speakers found and the ones in use.
    pub fn ctl_airplay(&self) -> ctl::Reply {
        let devices = self.airplay.devices.iter().map(|o| ctl::AirPlayDevice { name: o.name.clone(), kind: o.kind.label().to_string(), pair: o.pair }).collect();
        ctl::Reply::AirPlay { current: self.ctl_airplay_now(), devices }
    }

    pub fn ctl_queue(&self) -> ctl::Queue {
        ctl::Queue {
            history: self.history_indices().iter().map(|&i| song_info(&self.history.tracks[i].1)).collect(),
            current: self.playback.song.as_ref().map(song_info),
            up_next: self.queue.up_next().map(song_info).collect(),
            queued: self.queue.user.len(),
            source: self.queue.source.clone(),
            shuffle: self.playback.shuffle,
            repeat: repeat_mode(self.playback.repeat),
            next_lap: self.queue.next_lap_songs().iter().map(song_info).collect(),
            station: self.queue.station.as_ref().map(|r| r.name.clone()),
            station_played: self.queue.station_played().iter().map(song_info).collect(),
            autoplay: self.queue.autoplay.iter().map(song_info).collect(),
            autoplay_on: self.autoplay_offered().then_some(self.settings.autoplay),
            run: Some(self.queue.run),
        }
    }

    /// The ids of the entries `Jump` numbers, in its order: Up Next, the next lap, AutoPlay's
    /// suggestions, then the songs a station played (as `ctl_queue` lists them).
    fn ctl_jump_ids(&self) -> Vec<String> {
        let lap = self.queue.next_lap_songs();
        self.queue
            .up_next()
            .chain(lap.iter())
            .chain(self.queue.autoplay.iter())
            .chain(self.queue.station_played().iter())
            .map(|s| s.id.clone())
            .collect()
    }

    fn ctl_up_next_ids(&self) -> Vec<String> {
        self.queue.up_next().map(|s| s.id.clone()).collect()
    }

    /// The library's playlists: those pinned in the sidebar first, in pin order (marked with
    /// `pinned`), then Favourite Songs, then the rest as Apple lists them. Each comes with its
    /// `modified` date (the socket thread fills counts the app doesn't hold from the disk cache,
    /// `ctl::fill_counts`). Pinned albums, artists and stations don't count.
    pub fn ctl_playlist_entries(&self) -> Vec<(ctl::CollectionInfo, Option<String>)> {
        let items = &self.library_playlists.items;
        let favs = items.iter().filter(|c| c.favorites);
        let pin_ids = self.settings.pins.iter().filter(|p| p.kind == PinKind::Playlist).map(|p| p.id.as_str());
        let ranks: HashMap<&str, u32> = pin_ids.zip(0..).collect();
        let mut out: Vec<(ctl::CollectionInfo, Option<String>)> = favs
            .chain(items.iter().filter(|c| !c.favorites))
            .map(|c| {
                let mut info = self.info_with_count(c);
                info.pinned = ranks.get(c.id.as_str()).copied();
                (info, c.modified.clone())
            })
            .collect();
        // Uzumaki playlists (kept on this computer) after Apple's.
        out.extend(self.mixed.cards.iter().map(|c| {
            let mut info = collection_info(c);
            info.editable = true;
            info.pinned = ranks.get(c.id.as_str()).copied();
            (info, None)
        }));
        out.sort_by_key(|(c, _)| c.pinned.unwrap_or(u32::MAX)); // stable
        out
    }

    /// `uzu playlist add-local`: adds the music folders' file at `path` to playlist `id` (one of
    /// the user's Apple playlists, on this computer only, or an Uzumaki playlist). The playlist
    /// with its songs afterwards.
    pub fn ctl_add_local(&mut self, id: &str, path: &str) -> Result<(ctl::CollectionInfo, Vec<ctl::SongInfo>), String> {
        let Some(c) = self.add_target(id) else {
            return Err("only your own playlists and Uzumaki playlists can take songs".into());
        };
        let want = Song::local_id(std::path::Path::new(path));
        let Some(song) = self.local_lib.songs.iter().find(|s| s.id == want).cloned() else {
            return Err(format!("{path} isn't in your music folders (Settings \u{2192} Local Files)"));
        };
        self.add_to_playlist(&c, std::slice::from_ref(&song));
        let full = if crate::mixed::is_uzu_id(id) {
            self.uzu_collection(id).unwrap_or(c)
        } else {
            self.collections.get(id).and_then(Load::ready).cloned().unwrap_or(c)
        };
        let songs = self.playlist_songs(&full);
        Ok((collection_info(&full), songs.iter().map(song_info).collect()))
    }

    pub fn ctl_albums(&self) -> Vec<ctl::CollectionInfo> {
        if self.local_only() {
            // The music folders' albums (their counts are in the view).
            return self.local_lib.view.albums.iter().map(collection_info).collect();
        }
        self.library_albums.items.iter().map(|c| self.info_with_count(c)).collect()
    }

    fn info_with_count(&self, c: &Collection) -> ctl::CollectionInfo {
        let mut info = collection_info(c);
        if info.count.is_none() {
            if let Some(Load::Ready(full)) = self.collections.get(&c.id) {
                info.count = (!full.tracks.is_empty()).then_some(full.tracks.len() as u32);
            }
        }
        info
    }

    /// An album or playlist with its songs, when they're in memory; else the library list's
    /// entry for it (its `modified` date tells whether the disk copy is current).
    pub fn ctl_lookup(&self, id: &str) -> (Option<Collection>, Option<Collection>) {
        if crate::mixed::is_uzu_id(id) {
            return (self.uzu_collection(id), None);
        }
        if crate::local::view::is_album_id(id) {
            return (self.local_album(id), None);
        }
        let loaded = match self.collections.get(id) {
            Some(Load::Ready(c)) if !c.tracks.is_empty() => Some(c.clone()),
            _ => None,
        };
        let listed = || {
            let in_list = |l: &Paged<Collection>| l.items.iter().find(|c| c.id == id).cloned();
            in_list(&self.library_playlists).or_else(|| in_list(&self.library_albums))
        };
        match loaded {
            Some(c) => (Some(c), None),
            None => (None, listed()),
        }
    }

    /// Keeps an album or playlist `uzu` loaded, so its page opens at once. `saved_at`: when it
    /// came from the disk cache (the page checks it against Apple as usual); None when it was
    /// just fetched (then it's saved).
    pub fn ctl_adopt(&mut self, c: Collection, saved_at: Option<u64>) {
        if matches!(self.collections.get(&c.id), Some(Load::Ready(x)) if !x.tracks.is_empty()) {
            return;
        }
        let state = match saved_at {
            Some(t) => CollCache::Cached(t),
            None => {
                self.save_collection(&c);
                CollCache::Verified
            }
        };
        self.coll_cache.insert(c.id.clone(), state);
        self.collections.insert(c.id.clone(), Load::Ready(c));
    }

    /// Plays `c` from song `index`; without one, from a random song when shuffle is on, else
    /// from the first playable one. Without its songs (Apple sent none), `play_collection` asks
    /// Apple for them once more.
    ///
    /// `order` (song indices into `c.tracks`) plays them in that order instead, and `index` is a
    /// position within it; an index past the songs is an error (the list changed since the
    /// client loaded it). An empty `order` is the same as none.
    ///
    /// `song_id`: the song the client showed at `index`. When the list changed since, that song
    /// plays from where it is now; gone (or a sorted list that changed) is an error.
    pub fn ctl_play_collection(&mut self, c: &Collection, index: Option<usize>, order: Option<&[usize]>, song_id: Option<&str>) -> Result<(), String> {
        // With the local songs of its overlay: `Tracks` listed them so (`mixed.rs`).
        let all = self.playlist_songs(c);
        let reordered: Vec<Song>;
        let tracks: &[Song] = match order.filter(|o| !o.is_empty()) {
            Some(o) => {
                if o.iter().any(|&i| i >= all.len()) {
                    return Err("this list changed since it was loaded; press R to reload".into());
                }
                reordered = o.iter().map(|&i| all[i].clone()).collect();
                &reordered
            }
            None => &all,
        };
        if tracks.is_empty() {
            if self.bridge.is_none() {
                return Err("the playback bridge isn't running".into());
            }
            match index {
                None if self.playback.shuffle => self.shuffle_collection(c),
                _ => self.play_collection(c, index.unwrap_or(0)),
            }
            return Ok(());
        }
        let index = match (index, song_id) {
            (Some(i), Some(want)) if tracks.get(i).is_none_or(|s| s.id != want) => {
                if order.is_some_and(|o| !o.is_empty()) {
                    // The order was made from the old list: no telling what it means now.
                    return Err(format!("“{}” changed since uzu loaded it: reload it (R, or uzu {} again)", c.title, kind_word(c)));
                }
                let ids: Vec<String> = tracks.iter().map(|s| s.id.clone()).collect();
                match locate(&ids, i, want, &[]) {
                    Some(k) => Some(k),
                    None => return Err(format!("“{}” changed and that song is no longer in it: reload it (R, or uzu {} again)", c.title, kind_word(c))),
                }
            }
            (index, _) => index,
        };
        let playable: Vec<usize> = (0..tracks.len()).filter(|&i| tracks[i].playable).collect();
        let index = match index {
            Some(i) => i,
            None if playable.is_empty() => return Err(format!("nothing in “{}” is available on Apple Music", c.title)),
            None if self.playback.shuffle => playable[super::queue::random_below(playable.len())],
            None => playable[0],
        };
        let Some(song) = tracks.get(index) else {
            return Err(format!("“{}” has {} songs", c.title, tracks.len()));
        };
        if !song.playable {
            return Err(format!("“{}” isn't available on Apple Music", song.title));
        }
        self.play_list(tracks, index, &c.title);
        Ok(())
    }

    /// `Jump`: entry `index` in `ctl_jump_ids`' order; with `id`, that song wherever it is now.
    pub fn ctl_jump(&mut self, index: usize, id: Option<&str>) -> Result<(), String> {
        let ids = self.ctl_jump_ids();
        let index = match id {
            Some(id) => locate(&ids, index, id, &[]).ok_or(QUEUE_CHANGED)?,
            None => index,
        };
        let n = ids.len();
        if index >= n {
            return Err(match n {
                0 => "Up Next is empty".into(),
                n => format!("there is no song {} in Up Next (it has {n})", index + 1),
            });
        }
        self.play_up_next(index);
        Ok(())
    }

    /// `History`: entry `index` of the session history (oldest first, as `ctl_queue` lists it);
    /// with `id`, that song wherever it is now.
    pub fn ctl_history(&mut self, index: usize, id: Option<&str>) -> Result<(), String> {
        let entries = self.history_indices();
        let index = match id {
            Some(id) => {
                let ids: Vec<String> = entries.iter().map(|&i| self.history.tracks[i].1.id.clone()).collect();
                locate(&ids, index, id, &[]).ok_or("the history changed and that song is no longer in it (run uzu queue again)")?
            }
            None => index,
        };
        let Some(&i) = entries.get(index) else { return Err("History entry no longer exists".into()) };
        self.play_history(i);
        Ok(())
    }

    /// `uzu move`: Up Next entry `from` to index `to` (0-based). With `id` (the song at `from`)
    /// found elsewhere, `to` moves by as much.
    pub fn ctl_move(&mut self, from: usize, to: usize, id: Option<&str>) -> Result<(), String> {
        let ids = self.ctl_up_next_ids();
        let n = ids.len();
        let (from, to) = match id {
            Some(id) => {
                let now = locate(&ids, from, id, &[]).ok_or(QUEUE_CHANGED)?;
                (now, (to + now).saturating_sub(from).min(n.saturating_sub(1)))
            }
            None => (from, to),
        };
        self.check_up_next(n, &[from, to])?;
        self.move_up_next(from, to);
        Ok(())
    }

    /// `uzu del`: removes Up Next entries (0-based). `ids` (when given, one per index): the songs
    /// meant, wherever they are now; one gone removes nothing.
    pub fn ctl_remove(&mut self, indices: &[usize], ids: &[String]) -> Result<(), String> {
        let now = self.ctl_up_next_ids();
        let n = now.len();
        let mut found = Vec::with_capacity(indices.len());
        if !ids.is_empty() && ids.len() == indices.len() {
            for (&i, id) in indices.iter().zip(ids) {
                let k = locate(&now, i, id, &found).ok_or(QUEUE_CHANGED)?;
                found.push(k);
            }
        } else {
            found.extend_from_slice(indices);
        }
        self.check_up_next(n, &found)?;
        self.remove_up_next_many(&found);
        Ok(())
    }

    fn check_up_next(&self, n: usize, indices: &[usize]) -> Result<(), String> {
        match indices.iter().find(|&&i| i >= n) {
            None => Ok(()),
            Some(_) if n == 0 => Err("Up Next is empty".into()),
            Some(i) => Err(format!("there is no song {} in Up Next (it has {n})", i + 1)),
        }
    }

    /// Search results picked on the command line (unplayable ones are skipped).
    pub fn ctl_pick(&mut self, songs: Vec<Song>, action: ctl::PickAction, source: &str) -> Result<(), String> {
        let songs: Vec<Song> = songs.into_iter().filter(|s| s.playable).collect();
        if songs.is_empty() {
            return Err("that isn't available on Apple Music".into());
        }
        match action {
            ctl::PickAction::Play => self.play_list(&songs, 0, source),
            ctl::PickAction::Next => self.play_next(&songs),
            ctl::PickAction::Add => self.add_to_queue(&songs),
        }
        Ok(())
    }

    pub fn ctl_seek(&mut self, to: ctl::SeekTo) {
        let target = match to {
            ctl::SeekTo::Absolute(t) => t,
            ctl::SeekTo::Relative(d) => self.playback.heard_now() + d,
            ctl::SeekTo::Percent(p) => self.playback.duration * p / 100.0,
        };
        self.seek_seconds(target.max(0.0));
    }

    pub fn ctl_volume(&mut self, to: ctl::VolumeTo) {
        match to {
            ctl::VolumeTo::Set(v) => self.set_volume(v),
            ctl::VolumeTo::Change(d) => self.set_volume(self.playback.volume + d),
        }
    }
}
