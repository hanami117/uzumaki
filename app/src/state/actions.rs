use super::*;

impl State {
    // ---------------------------------------------------------------- navigation

    pub fn navigate(&mut self, route: Route) {
        // Local only mode has no Apple pages.
        if self.local_only() && route.needs_apple() {
            return;
        }
        if route != self.route {
            let old = std::mem::replace(&mut self.route, route);
            self.back_stack.push(old);
            self.ui.route_changed();
            self.remember_route();
        }
        self.load_route_data();
    }

    pub fn back(&mut self) {
        if self.local_only() {
            self.back_stack.retain(|r| !r.needs_apple());
        }
        if let Some(route) = self.back_stack.pop() {
            self.route = route;
            self.ui.route_changed();
            self.remember_route();
            self.load_route_data();
        }
    }

    pub fn open_collection(&mut self, c: &Collection) {
        // Seed the detail page with what we already know, so the header shows at once.
        if !self.collections.contains_key(&c.id) && !c.tracks.is_empty() {
            self.collections.insert(c.id.clone(), Load::Ready(c.clone()));
        }
        self.navigate(Route::Collection { kind: c.kind, id: c.id.clone() });
    }

    /// Submits a search: switches to the Search page and asks the catalog.
    pub fn search(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        self.search_query = query.to_string();
        self.search_seq += 1;
        let seq = self.search_seq;
        self.search = if self.bridge.is_some() { Load::Loading } else { Load::Idle };
        self.request(api::search(query), SearchResults::parse, move |result| Msg::Search { seq, result });
        if self.route != Route::Search {
            self.navigate(Route::Search);
        }
    }

    // ---------------------------------------------------------------- playback

    /// Plays an album or playlist from track `index` to its end.
    pub fn play_collection(&mut self, c: &Collection, index: usize) {
        if c.tracks.is_empty() && crate::local::view::is_album_id(&c.id) {
            // A card of local only mode's Albums: its songs come from the music folders.
            if let Some(full) = self.local_album(&c.id) {
                self.play_list(&full.tracks, index, &full.title);
            }
        } else if !c.tracks.is_empty() {
            self.play_list(&c.tracks, index, &c.title);
        } else if self.bridge.is_some() {
            // Track list not loaded yet: let MusicKit resolve the album/playlist itself.
            self.stop_local();
            let Some(b) = &self.bridge else { return };
            match c.kind {
                CollectionKind::Album => b.play_album(&c.id, index),
                CollectionKind::Playlist => b.play_playlist(&c.id, index),
            }
        }
    }

    /// A Shuffle button: turns shuffle on and plays `songs` from a random playable song (not
    /// always the first, as in Apple Music), the rest shuffled behind it.
    pub fn shuffle_play(&mut self, songs: &[Song], source: &str) {
        self.set_shuffle(true);
        // None playable: start at 0 so `play_list` says why nothing plays.
        self.play_list(songs, super::queue::random_playable(songs).unwrap_or(0), source);
    }

    /// The Shuffle button of an album or playlist. Without its songs, MusicKit resolves it from
    /// a random position (playability unknown until then).
    pub fn shuffle_collection(&mut self, c: &Collection) {
        if c.tracks.is_empty() && crate::local::view::is_album_id(&c.id) {
            if let Some(full) = self.local_album(&c.id) {
                self.shuffle_play(&full.tracks, &full.title);
            }
        } else if c.tracks.is_empty() {
            self.set_shuffle(true);
            let n = c.track_count.unwrap_or(0) as usize;
            self.play_collection(c, if n > 1 { super::queue::random_below(n) } else { 0 });
        } else {
            self.shuffle_play(&c.tracks, &c.title);
        }
    }

    /// Plays `songs[index]` followed by the rest of the list.
    pub fn play_songs(&mut self, songs: &[Song], index: usize) {
        self.play_list(songs, index, "");
    }

    /// Plays `songs[index]` and makes the list the play order ("Continue playing from `source`").
    /// With shuffle on, the rest of the list follows in random order.
    pub fn play_list(&mut self, songs: &[Song], index: usize, source: &str) {
        let Some(first) = songs.get(index) else { return };
        if !first.playable {
            self.toast = Some(format!("“{}” isn't available on Apple Music.", first.title));
            return;
        }
        // Clicking the song that is already starting again would only restart its loading.
        let loading = matches!(self.playback.state, PlayState::Loading | PlayState::Waiting | PlayState::Stalled);
        if loading && self.playback.song.as_ref().is_some_and(|cur| cur.id == first.id) {
            return;
        }
        let key = format!("{}@{}/{}", first.id, index, songs.len());
        let now = crate::clock::now();
        if let Some((last, at)) = &self.last_play {
            if *last == key && now.duration_since(*at) < PLAY_DEBOUNCE {
                return;
            }
        }
        self.last_play = Some((key, now));
        let user = std::mem::take(&mut self.queue.user);
        self.queue = PlayQueue::start(songs, index, self.playback.shuffle, source, user);
        self.queue.plan_lap(self.playback.repeat); // the bridge's loop and Up Next need the next lap from the start
        self.begin_current(first.clone());
    }

    /// Shows `song` as loading at once and starts it: a local file in the in-process player,
    /// an Apple song in the bridge, followed by the queue up to the next local file.
    pub(super) fn begin_current(&mut self, song: Song) {
        self.begin_current_at(song, 0.0);
    }

    /// `begin_current`, starting `at` seconds in (a resumed song).
    pub(super) fn begin_current_at(&mut self, song: Song, at: f64) {
        // Local only mode: an Apple song (from the history, say) can't play.
        if self.local_only() && !song.is_local() {
            self.toast = Some("That song is on Apple Music, which is off in local only mode.".into());
            return;
        }
        self.resume = None;
        self.playback.song = Some(song.clone());
        self.playback.state = PlayState::Loading;
        self.starting = Some(crate::clock::now());
        let at = if at > 0.0 && (song.duration <= 0.0 || at < song.duration) { at } else { 0.0 };
        self.playback.position = at;
        self.playback.position_at = None;
        self.playback.slew = 0.0;
        // Reports from before the start position took are ignored (see `apply_time`).
        self.playback.seek_target = (at > 0.0).then(|| (at, crate::clock::now()));
        self.playback.duration = song.duration;
        // Whichever player starts now starts its own queue here (Previous before it is ours).
        self.queue.base = self.queue.pos;
        self.want_lyrics(&song);
        self.preload_art();
        // The song starts now, before the player says so.
        if self.hooks.observe(Some(&song), self.playback.state) {
            self.notifier.song_changed(&song, self.playback.state);
        }
        if song.is_local() {
            self.start_local(&song, at);
        } else {
            self.stop_local();
            self.start_in_bridge(song.play_id().to_string(), at);
        }
        self.resume_changed(true);
    }

    /// Starts `first` in MusicKit, followed by the queue up to the next local file.
    fn start_in_bridge(&self, first: String, at: f64) {
        if let Some(b) = &self.bridge {
            let mut ids = vec![first.clone()];
            ids.extend(self.queue.upcoming_ids());
            b.play_queue(&ids, self.bridge_cycle().as_deref(), at, move |error| Some(Msg::QueueFailed { first, error }));
        }
    }

    /// MusicKit refused to start `first`. MusicKit then carries on with whatever it played
    /// before, so the bar must not keep showing the new song as loading. A song Apple can't find
    /// is tried once more by its library id (an uploaded or matched copy can still play), then
    /// skipped like Apple Music skips greyed-out songs. Other failures show what really plays.
    pub(super) fn queue_failed(&mut self, first: &str, error: &str) {
        // The paused hand-over of a resumed song (`prime_resume`) failed: Play starts it afresh.
        if let Some(p) = &mut self.resume {
            p.primed = None;
            return;
        }
        let Some(song) = self.playback.song.clone() else { return };
        if self.playback.local || (song.play_id() != first && song.id != first) {
            return; // a newer song took over already
        }
        if !error.contains("NOT_FOUND") {
            self.starting = None;
            self.toast = Some(format!("Couldn't play “{}”: {error}", song.title));
            if let Some(b) = &self.bridge {
                b.announce();
            }
            return;
        }
        if first != song.id && !song.is_local() {
            let at = self.playback.seek_target.map_or(0.0, |(t, _)| t);
            return self.start_in_bridge(song.id.clone(), at);
        }
        self.starting = None;
        self.mark_unavailable(&song);
        self.skipped_in_row += 1;
        let next = if self.skipped_in_row < MAX_SKIPS { self.queue.advance(self.playback.repeat) } else { None };
        match next {
            Some(next) => {
                self.toast = Some(format!("“{}” isn't available on Apple Music, skipped", song.title));
                self.begin_current(next);
            }
            None => {
                self.toast = Some(format!("“{}” isn't available on Apple Music", song.title));
                self.playback.set_state(PlayState::Stopped);
                self.skipped_in_row = 0;
                if let Some(b) = &self.bridge {
                    b.announce(); // MusicKit may still be playing the song from before
                }
            }
        }
    }

    /// Greys out a song Apple couldn't find, wherever this session lists it, so it isn't queued
    /// again (until the list reloads from Apple).
    fn mark_unavailable(&mut self, song: &Song) {
        let dead = |s: &mut Song| {
            if s.same_track(song) {
                s.playable = false;
            }
        };
        self.queue.context.iter_mut().chain(self.queue.original.iter_mut()).chain(self.queue.next_lap.iter_mut().flatten()).for_each(dead);
        for c in self.collections.values_mut() {
            if let Load::Ready(c) = c {
                c.tracks.iter_mut().for_each(dead);
            }
        }
        self.library_songs.items.iter_mut().for_each(dead);
    }

    /// The whole list for the bridge to loop under repeat all, when it can (Apple songs only).
    pub(super) fn bridge_cycle(&self) -> Option<Vec<String>> {
        if self.playback.repeat == Repeat::All { self.queue.cycle_ids() } else { None }
    }

    /// Tells the bridge what follows the current song after the queue changed. While a local
    /// file plays there's nothing to tell: the next Apple song starts a fresh bridge queue.
    pub(super) fn sync_upcoming(&mut self) {
        self.queue.plan_lap(self.playback.repeat);
        self.resume_changed(false);
        if self.playback.local {
            return;
        }
        if let Some(b) = &self.bridge {
            b.set_upcoming(&self.queue.upcoming_ids(), self.bridge_cycle().as_deref());
        }
    }

    pub fn toggle_shuffle(&mut self) {
        self.set_shuffle(!self.playback.shuffle);
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.playback.shuffle {
            return;
        }
        self.playback.shuffle = on;
        self.queue.set_shuffle(on);
        self.sync_upcoming();
        self.settings.shuffle = self.playback.shuffle;
        self.save_settings();
    }

    /// Off → All → One → Off.
    pub fn cycle_repeat(&mut self) {
        self.set_repeat(self.playback.repeat.next());
    }

    pub fn set_repeat(&mut self, mode: Repeat) {
        self.playback.repeat = mode;
        if let Some(b) = &self.bridge {
            b.set_repeat(self.playback.repeat.bridge_name());
        }
        self.sync_upcoming();
        self.settings.repeat = self.playback.repeat;
        self.save_settings();
    }

    /// Plays `songs` right after the current song (before anything else queued).
    pub fn play_next(&mut self, songs: &[Song]) {
        let songs: Vec<Song> = songs.iter().filter(|s| s.playable).cloned().collect();
        if songs.is_empty() {
            return;
        }
        if self.playback.song.is_none() {
            return self.play_list(&songs, 0, "");
        }
        self.queue.play_next(&songs);
        self.sync_upcoming();
        self.toast = Some(match songs.len() {
            1 => format!("“{}” plays next", songs[0].title),
            n => format!("{n} songs play next"),
        });
    }

    /// Adds `songs` after what the user already queued (before the rest of the list).
    pub fn add_to_queue(&mut self, songs: &[Song]) {
        let songs: Vec<Song> = songs.iter().filter(|s| s.playable).cloned().collect();
        if songs.is_empty() {
            return;
        }
        if self.playback.song.is_none() {
            return self.play_list(&songs, 0, "");
        }
        self.queue.add_to_queue(&songs);
        self.sync_upcoming();
        self.toast = Some(match songs.len() {
            1 => format!("Added “{}” to the queue", songs[0].title),
            n => format!("Added {n} songs to the queue"),
        });
    }

    /// Plays the `i`-th entry of Up Next now.
    pub fn play_up_next(&mut self, i: usize) {
        if let Some(song) = self.queue.jump(i) {
            self.begin_current(song);
        }
    }

    /// Hide an optimistically starting track without recording it or deleting confirmed history.
    pub fn history_indices(&self) -> Vec<usize> {
        self.history.tracks.iter().enumerate().filter(|(_, (_, song))| {
            !self.playback.song.as_ref().is_some_and(|current| current.same_track(song))
        }).map(|(i, _)| i).collect()
    }

    /// History in the active context jumps there; otherwise replay one track, keeping Play Next.
    pub fn play_history(&mut self, i: usize) {
        let Some((_, song)) = self.history.tracks.get(i).cloned() else { return };
        if !song.playable { return; }
        if self.queue.station.is_some() {
            // Use the existing one-off/resume path, also for history from an earlier source.
            let run = self.queue.station.as_mut().unwrap();
            let k = run.played.iter().position(|s| s.same_track(&song)).unwrap_or_else(|| {
                run.played.push(song.clone()); run.played.len() - 1
            });
            self.play_up_next(self.queue.up_next_len() + self.queue.next_lap_songs().len() + k);
        } else if let Some(pos) = self.queue.context.iter().position(|s| s.same_track(&song)) {
            self.queue.pos = pos;
            self.queue.base = pos;
            self.queue.plan_lap(self.playback.repeat);
            self.begin_current(song);
        } else {
            self.play_list(&[song], 0, "History");
        }
    }

    pub fn remove_up_next(&mut self, i: usize) {
        self.queue.remove_up_next(i);
        self.sync_upcoming();
    }

    /// Removes several Up Next entries at once (0-based, any order).
    pub fn remove_up_next_many(&mut self, indices: &[usize]) {
        self.queue.remove_up_next_many(indices);
        self.sync_upcoming();
    }

    /// Moves Up Next entry `from` to index `to`. False when out of range.
    pub fn move_up_next(&mut self, from: usize, to: usize) -> bool {
        let moved = self.queue.move_up_next(from, to);
        if moved {
            self.sync_upcoming();
        }
        moved
    }

    pub fn toggle_play(&mut self) {
        if self.playback.state.is_active() {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Resumes (or, for a local file that ended, plays it again from the start).
    pub fn play(&mut self) {
        if self.resume.is_some() {
            return self.play_resumed();
        }
        if self.playback.local {
            if self.playback.state == PlayState::Paused {
                #[cfg(test)]
                self.local_sent.borrow_mut().push("resume".into());
                if let Some(p) = &self.local {
                    p.resume();
                }
            } else if let Some(song) = self.playback.song.clone().filter(|s| s.is_local()) {
                self.begin_current(song);
            }
        } else if let Some(b) = &self.bridge {
            b.play();
        }
    }

    pub fn pause(&mut self) {
        if self.resume.is_some() {
            return; // a resumed song is paused until Play
        }
        if self.playback.local {
            #[cfg(test)]
            self.local_sent.borrow_mut().push("pause".into());
            if let Some(p) = &self.local {
                p.pause();
            }
        } else if let Some(b) = &self.bridge {
            b.pause();
        }
    }

    /// Skips to the next song. MusicKit does it when it has that song; otherwise (a local file
    /// plays, or comes next) the app starts it.
    pub fn next(&mut self) {
        let repeat = self.playback.repeat;
        // Next on the last song played on its own on a station: the station picks up again
        // (MusicKit has nothing after it, so its skip would only stop).
        if self.last_station_one_off() {
            return self.resume_station();
        }
        // The resumed song was never handed over: the app starts the next one itself.
        if self.resume.is_some() {
            if let Some(song) = self.queue.advance(repeat) {
                self.begin_current(song);
            }
            return;
        }
        let app_decides = self.playback.local || (self.queue.peek_next(repeat).is_some() && !self.queue.bridge_has_next(repeat));
        if app_decides {
            if let Some(song) = self.queue.advance(repeat) {
                self.begin_current(song);
            }
        } else if let Some(b) = &self.bridge {
            b.next();
        }
    }

    /// Restarts the song after 3 s; otherwise the previous song of the list, also one before
    /// the song playback started from (MusicKit doesn't have those, so the queue restarts there).
    pub fn previous(&mut self) {
        if self.resume.is_some() && self.playback.position <= 3.0 {
            match self.queue.step_back() {
                Some(song) => self.begin_current(song),
                None => self.seek_resumed(0.0),
            }
            return;
        }
        if self.playback.position > 3.0 {
            return self.seek_to(0.0);
        }
        if self.queue.pos > self.queue.base && !self.playback.local {
            if let Some(b) = &self.bridge {
                b.previous();
            }
        } else if let Some(song) = self.queue.step_back() {
            self.begin_current(song);
        } else {
            self.seek_to(0.0);
        }
    }

    /// Synced lyrics in full-screen Now Playing: ask for time 4×/s while they show.
    pub fn set_fine_time(&mut self, on: bool) {
        self.fine_time_np = on;
        self.sync_time_rate();
    }

    /// Synced lyrics in the right sidebar: same as `set_fine_time`, for the other view. The
    /// bridge sends time 4×/s while either asks.
    pub fn set_panel_fine_time(&mut self, on: bool) {
        self.fine_time_panel = on;
        self.sync_time_rate();
    }

    fn sync_time_rate(&mut self) {
        let on = self.fine_time_np || self.fine_time_panel;
        if self.fine_time != on {
            self.fine_time = on;
            if let Some(b) = &self.bridge {
                b.time_rate(if on { 4 } else { 1 });
            }
        }
    }

    // ---------------------------------------------------------------- right sidebar

    /// Ctrl+Shift+B: shows or hides the right sidebar (on the tab it had).
    pub fn toggle_right_panel(&mut self) {
        self.settings.right_panel = !self.settings.right_panel;
        self.save_settings();
    }

    /// The player bar's Lyrics / Up Next buttons: shows the sidebar on `tab`, or hides it when
    /// it already shows that tab.
    pub fn show_right_tab(&mut self, tab: crate::settings::RightTab) {
        if self.settings.right_panel && self.settings.right_tab == tab {
            self.settings.right_panel = false;
        } else {
            self.settings.right_panel = true;
            self.settings.right_tab = tab;
        }
        self.save_settings();
    }

    /// Full-screen Now Playing's column shows `tab`: the lyrics (switched on), or Up Next.
    pub fn set_now_playing_column(&mut self, tab: crate::settings::RightTab) {
        let (queue, lyrics) = match tab {
            crate::settings::RightTab::Queue => (true, self.settings.now_playing_lyrics),
            crate::settings::RightTab::Lyrics => (false, true),
        };
        if (queue, lyrics) != (self.settings.now_playing_queue, self.settings.now_playing_lyrics) {
            self.settings.now_playing_queue = queue;
            self.settings.now_playing_lyrics = lyrics;
            self.save_settings();
        }
    }

    // ---------------------------------------------------------------- keyboard steps

    /// Seeks `delta` seconds from where the song is now (Shift+←/→).
    pub fn seek_by(&mut self, delta: f64) {
        if self.playback.song.is_some() && self.playback.duration > 0.0 {
            let t = self.playback.position_now() + delta;
            self.seek_seconds(t);
        }
    }

    /// Changes the volume by `delta` (0..1 scale; Ctrl+↑/↓).
    pub fn volume_by(&mut self, delta: f64) {
        self.set_volume(self.playback.volume + delta);
    }

    /// Adds `songs` to one of the user's library playlists.
    pub fn add_to_playlist(&mut self, playlist: &Collection, songs: &[Song]) {
        let Some(b) = &self.bridge else { return };
        if !playlist.id.starts_with("p.") {
            self.toast = Some("Only your own library playlists can be edited.".into());
            return;
        }
        if songs.iter().all(Song::is_local) {
            self.toast = Some("Local files can't be added to Apple Music playlists yet.".into());
            return;
        }
        let data = super::playlists::song_refs(songs);
        if data.is_empty() {
            return;
        }
        let (id, name, count) = (playlist.id.clone(), playlist.title.clone(), data.len());
        let account = Some(self.session.account);
        b.api_post(&format!("/v1/me/library/playlists/{id}/tracks"), serde_json::json!({ "data": data }), move |r| {
            Some(super::library::stamp(account, Msg::AddedToPlaylist { id, name, count, result: r.map(|_| ()) }))
        });
    }

    /// The lyrics of `song` as loaded so far (None: never asked, e.g. a local file or a library
    /// song Apple didn't match). Same key as `want_lyrics`.
    pub fn lyrics_of(&self, song: &Song) -> Option<&Load<Option<crate::lyrics::Lyrics>>> {
        let key = match &song.catalog_id {
            Some(c) => c.as_str(),
            None if song.is_local() || song.id.starts_with("i.") => return None,
            None => song.id.as_str(),
        };
        self.lyrics.get(key)
    }

    /// Fetches lyrics for `song` once (catalog songs only; needs a subscription).
    pub(super) fn want_lyrics(&mut self, song: &Song) {
        if song.is_local() {
            return; // lyrics are Apple's only
        }
        let id = match &song.catalog_id {
            Some(c) => c.clone(),
            None if !song.id.starts_with("i.") => song.id.clone(),
            None => return,
        };
        if !self.session.authorized || self.lyrics.contains_key(&id) {
            return;
        }
        if self.lyrics.len() > 200 {
            self.lyrics.clear();
        }
        self.lyrics.insert(id.clone(), Load::Loading);
        self.request_lyrics(id, true);
    }

    /// Word-timed ("syllable") lyrics first, for the per-word highlight; songs that only have
    /// line timing answer that request with an error, so then ask for the line-timed ones.
    pub(super) fn request_lyrics(&self, id: String, words: bool) {
        let kind = if words { "syllable-lyrics" } else { "lyrics" };
        let path = format!("/v1/catalog/{{{{storefrontId}}}}/songs/{id}/{kind}");
        self.request(
            (path, serde_json::json!({})),
            |v| v["data"][0]["attributes"]["ttml"].as_str().and_then(crate::lyrics::parse_ttml),
            move |r| match r {
                Ok(l) => Msg::Lyrics { id, result: Ok(l) },
                Err(_) if words => Msg::LyricsRetry { id },
                // No lyrics at all: that's "none", not a failure.
                Err(_) => Msg::Lyrics { id, result: Ok(None) },
            },
        );
    }

    /// Downloads the big Now Playing cover of the current song and the next two, so opening
    /// Now Playing or skipping shows them at once (they also land in the disk cache).
    pub(super) fn preload_art(&self) {
        let big = crate::art::NOW_PLAYING_PX;
        let current = self.playback.song.iter();
        for song in current.chain(self.queue.up_next().take(2)) {
            if let Some(a) = &song.artwork {
                self.art.prefetch(a, big);
                self.art.prefetch(a, crate::art::SMALL_PX);
            }
        }
    }

    // ---------------------------------------------------------------- radio, albums, pins

    /// Plays a radio station (live or personal). Apple picks its songs as it goes, so there is
    /// no list to continue: Up Next shows the songs the station already played instead.
    pub fn play_station(&mut self, station: &Station) {
        self.start_station(&station.id, &station.name, None);
    }

    fn start_station(&mut self, id: &str, name: &str, keep: Option<StationRun>) {
        self.stop_local();
        self.queue = PlayQueue::for_station(id, name, keep);
        self.playback.state = PlayState::Loading;
        self.starting = Some(crate::clock::now());
        if let Some(b) = &self.bridge {
            b.play_station(id);
        }
    }

    /// A song played on its own from a station's history has ended and nothing else is queued:
    /// the station picks up again (a new song of its choosing; its history carries on).
    pub(super) fn resume_station(&mut self) {
        let Some(run) = self.queue.station.clone().filter(|r| r.one_off) else { return };
        self.start_station(&run.id.clone(), &run.name.clone(), Some(run));
    }

    /// Opens the album a song is on: catalog songs know it from their URL; library songs ask.
    pub fn go_to_album(&mut self, song: &Song) {
        if song.is_local() {
            // Local only mode has an album page for it; Apple mode keeps Local Files.
            return match &song.album_id {
                Some(id) if self.local_only() && crate::local::view::is_album_id(id) => {
                    self.navigate(Route::Collection { kind: CollectionKind::Album, id: id.clone() })
                }
                _ => self.navigate(Route::LocalFiles),
            };
        }
        if let Some(id) = &song.album_id {
            return self.navigate(Route::Collection { kind: CollectionKind::Album, id: id.clone() });
        }
        if song.id.starts_with("i.") {
            self.request(
                api::library_song_album(&song.id),
                |v| v["data"][0]["id"].as_str().map(String::from),
                |result| Msg::AlbumOf { result },
            );
        } else {
            self.toast = Some("Couldn't find the album of this song.".into());
        }
    }

    /// A local song's artist page (local only mode).
    pub fn go_to_local_artist(&mut self, song: &Song) {
        let a = crate::local::view::artist_of(song);
        self.navigate(Route::Artist { id: a.id, name: a.name });
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.settings.is_pinned(id)
    }

    pub fn pin_collection(&mut self, c: &Collection) {
        let kind = match c.kind {
            CollectionKind::Album => PinKind::Album,
            CollectionKind::Playlist => PinKind::Playlist,
        };
        self.pin(Pin { kind, id: c.id.clone(), title: c.title.clone(), artwork: c.artwork.clone() });
    }

    pub fn pin_artist(&mut self, a: &Artist) {
        self.pin(Pin { kind: PinKind::Artist, id: a.id.clone(), title: a.name.clone(), artwork: None });
    }

    pub fn pin_station(&mut self, st: &Station) {
        self.pin(Pin { kind: PinKind::Station, id: st.id.clone(), title: st.name.clone(), artwork: st.artwork.clone() });
    }

    fn pin(&mut self, pin: Pin) {
        if !self.settings.is_pinned(&pin.id) {
            self.toast = Some(format!("Pinned “{}”", pin.title));
            self.settings.pins.insert(0, pin); // newest pin on top
            self.save_settings();
        }
    }

    pub fn unpin(&mut self, id: &str) {
        self.settings.pins.retain(|p| p.id != id);
        self.save_settings();
    }

    /// Moves a pin up or down by `delta` places (sidebar reordering).
    pub fn move_pin(&mut self, id: &str, delta: isize) {
        let pins = &mut self.settings.pins;
        if let Some(i) = pins.iter().position(|p| p.id == id) {
            let j = (i as isize + delta).clamp(0, pins.len() as isize - 1) as usize;
            let p = pins.remove(i);
            pins.insert(j, p);
            self.save_settings();
        }
    }

    /// Drag-and-drop reordering: moves the pin `id` to position `to` (in the list without it).
    pub fn move_pin_to(&mut self, id: &str, to: usize) {
        let pins = &mut self.settings.pins;
        if let Some(i) = pins.iter().position(|p| p.id == id) {
            let p = pins.remove(i);
            pins.insert(to.min(pins.len()), p);
            self.save_settings();
        }
    }

    /// Opens what a pin points to.
    pub fn open_pin(&mut self, pin: &Pin) {
        match pin.kind {
            PinKind::Album => self.navigate(Route::Collection { kind: CollectionKind::Album, id: pin.id.clone() }),
            PinKind::Playlist => self.navigate(Route::Collection { kind: CollectionKind::Playlist, id: pin.id.clone() }),
            PinKind::Artist => self.navigate(Route::Artist { id: pin.id.clone(), name: pin.title.clone() }),
            PinKind::Station => {
                let st = Station { id: pin.id.clone(), name: pin.title.clone(), artwork: pin.artwork.clone(), live: false };
                self.play_station(&st);
            }
        }
    }

    pub fn toggle_sidebar(&mut self) {
        self.settings.sidebar_collapsed = !self.settings.sidebar_collapsed;
        self.save_settings();
    }

    pub fn toggle_pins_open(&mut self) {
        self.settings.pins_open = !self.settings.pins_open;
        self.save_settings();
    }

    /// Full-screen Now Playing with or without the lyrics column. While Up Next has the column,
    /// this switches it back to the lyrics.
    pub fn toggle_now_playing_lyrics(&mut self) {
        if self.settings.now_playing_queue {
            self.settings.now_playing_queue = false;
            self.settings.now_playing_lyrics = true;
        } else {
            self.settings.now_playing_lyrics = !self.settings.now_playing_lyrics;
        }
        self.save_settings();
    }

    /// Full-screen Now Playing: Up Next in the column beside the cover, or back to what was there
    /// (the lyrics, or nothing when they were off).
    pub fn toggle_now_playing_queue(&mut self) {
        self.settings.now_playing_queue = !self.settings.now_playing_queue;
        self.save_settings();
    }

    /// Lyrics: word-by-word highlight or whole lines.
    pub fn toggle_lyrics_words(&mut self) {
        self.settings.lyrics_words = !self.settings.lyrics_words;
        self.save_settings();
    }

    pub(crate) fn save_settings(&self) {
        if self.persist_settings {
            self.settings.save();
        }
    }

    /// Seeks to `fraction` (0..1) of the current track and clears the drag preview.
    pub fn seek_to(&mut self, fraction: f64) {
        self.seek_seconds(fraction.clamp(0.0, 1.0) * self.playback.duration);
    }

    /// Seeks to `seconds` from the start of the current track (MPRIS SetPosition).
    pub fn seek_seconds(&mut self, seconds: f64) {
        if self.resume.is_some() {
            return self.seek_resumed(seconds);
        }
        self.playback.seek_preview = None;
        let target = seconds.clamp(0.0, self.playback.duration.max(0.0));
        self.playback.position = target;
        self.playback.position_at = Some(crate::clock::now());
        self.playback.slew = 0.0;
        self.playback.seek_target = Some((target, crate::clock::now()));
        if self.playback.local {
            #[cfg(test)]
            self.local_sent.borrow_mut().push(format!("seek {target}"));
            if let Some(p) = &self.local {
                p.seek(target);
            }
        } else if let Some(b) = &self.bridge {
            b.seek(target);
        }
    }

    /// One volume for both players (MusicKit keeps its own copy across launches).
    pub fn set_volume(&mut self, volume: f64) {
        self.playback.volume = volume.clamp(0.0, 1.0);
        self.volume_set_at = Some(crate::clock::now());
        if let Some(p) = &self.local {
            p.set_volume(self.playback.volume);
        }
        if let Some(b) = &self.bridge {
            b.set_volume(self.playback.volume);
        }
    }

    /// Sign in (the app's banner, Settings → Account): the guided sign-in step on its own
    /// (`ui::onboarding::sign_in_only`). Started with `--login`, the bridge's full window
    /// instead, as before (debug).
    pub fn sign_in(&mut self) {
        if self.session.debug_login {
            self.show_login(true);
        } else {
            crate::ui::onboarding::sign_in_only(self);
        }
    }

    /// Shows or hides the Apple Music sign-in window.
    pub fn show_login(&mut self, visible: bool) {
        self.session.login_window = visible;
        if let Some(b) = &self.bridge {
            b.show_window(visible);
        }
    }
}
