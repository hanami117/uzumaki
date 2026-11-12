//! Local files in the app state: the music-folder library and the hand-over between the two
//! players. The bridge plays Apple songs and MusicKit moves through them on its own; the
//! in-process player (`local::player`) plays files, and after each one the app picks the next
//! song itself (`PlayQueue::advance`). `Playback::local` says which player is in charge.
//!
//! Where a run of Apple songs meets a local file, MusicKit was only given the Apple songs
//! (`PlayQueue::upcoming_ids`), so its queue runs out there: `bridge_queue_ended` notices that
//! and starts the file. After a file, an Apple song starts a fresh bridge queue.

use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::local::scan;
use crate::local::view::{self, LocalView};

/// Songs in the music folders (Settings → Local files).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalLibrary {
    /// Every local song, sorted for display (album artist, album, disc, track).
    pub songs: Vec<Song>,
    /// The index was read (from disk or a scan); before that, "no songs" means "don't know yet".
    pub loaded: bool,
    /// A scan is running; `scanned` files were looked at so far.
    pub scanning: bool,
    pub scanned: usize,
    /// The folders changed during a scan: scan again when it ends.
    pub(super) again: bool,
    /// Albums, artists and folders over `songs` (local only mode's tabs), derived with them.
    pub view: Arc<LocalView>,
    /// The songs of the local artist or folder page on screen (`State::fill_local_page`).
    pub page: Option<(Route, Vec<Song>)>,
    /// Counts index changes (virtual lists over the view rebuild their rows).
    pub generation: u64,
}

impl LocalLibrary {
    /// The songs of the artist or folder page `route`, once `fill_local_page` has run for it.
    pub fn page_songs(&self, route: &Route) -> &[Song] {
        match &self.page {
            Some((r, songs)) if r == route => songs,
            _ => &[],
        }
    }
}

/// How close to the end (seconds) an Apple song must be for an end-of-queue state to count as
/// "MusicKit's queue ran out" rather than the old song stopping as another one starts.
const END_SLACK: f64 = 5.0;

/// How far into a song (seconds) an end-of-queue state must come to be that song's own end even
/// when it is not within `END_SLACK` of the listed duration. MusicKit can end a song well before
/// the duration the library lists (a long uploaded mix stopped ~30 s early, live 2026-11-10), and
/// the late end state of the song before comes within a second or two of the new song's
/// `nowPlaying`, which set the position back to 0.
const OWN_END_AFTER: f64 = 10.0;

impl State {
    // ---------------------------------------------------------------- the two players

    /// Starts a local file in the in-process player, pausing the bridge if it was in charge.
    pub(super) fn start_local(&mut self, song: &Song, at: f64) {
        if !self.playback.local {
            if let Some(b) = &self.bridge {
                b.pause();
            }
            self.playback.local = true;
        }
        self.local_gen += 1;
        #[cfg(test)]
        self.local_sent.borrow_mut().push(if at > 0.0 { format!("play {} at {at}", song.title) } else { format!("play {}", song.title) });
        if let Some(p) = &self.local {
            p.play(self.local_gen, song, at, self.playback.volume);
        }
    }

    /// Hands playback back to the bridge: stops the in-process player and closes the device.
    pub(super) fn stop_local(&mut self) {
        if self.playback.local {
            self.playback.local = false;
            self.local_gen += 1; // whatever it still sends is stale
            #[cfg(test)]
            self.local_sent.borrow_mut().push("stop".into());
            if let Some(p) = &self.local {
                p.stop();
            }
        }
    }

    /// An event of the local player. Returns whether the view must redraw.
    pub(super) fn apply_local(&mut self, generation: u64, event: Event) -> bool {
        if generation != self.local_gen || !self.playback.local {
            return false; // about a song that was replaced since
        }
        match event {
            Event::NowPlaying(Some(song)) => {
                self.history.heard(&song);
                if song.duration > 0.0 {
                    self.playback.duration = song.duration;
                }
                if let Some(t) = &mut self.tray {
                    t.set_song(Some(&song));
                }
            }
            Event::PlayState(PlayState::Ended) => self.local_ended(),
            Event::PlayState(state) => {
                if state == PlayState::Playing {
                    self.starting = None;
                }
                if state == self.playback.state {
                    return false;
                }
                self.playback.set_state(state);
                if let Some(t) = &mut self.tray {
                    t.set_playing(state.is_active());
                }
            }
            Event::Time { current, duration } => return self.apply_time(current, duration),
            Event::Error(e) => {
                self.starting = None;
                self.toast = Some(e);
            }
            _ => return false,
        }
        true
    }

    /// A local file played to its end: the app decides what's next.
    fn local_ended(&mut self) {
        if self.playback.repeat == Repeat::One {
            if let Some(song) = self.playback.song.clone() {
                return self.begin_current(song);
            }
        }
        match self.queue.advance(self.playback.repeat) {
            Some(next) => self.begin_current(next),
            // A local file queued by hand on a station was the last one: the station carries on.
            None if self.queue.station.as_ref().is_some_and(|r| r.one_off) => self.resume_station(),
            None => {
                // End of the list. The player stays in charge (Play starts the song again).
                self.playback.state = PlayState::Ended;
                self.playback.position = self.playback.duration;
                if let Some(t) = &mut self.tray {
                    t.set_playing(false);
                }
            }
        }
    }

    /// The bridge reported `state` while in charge. When it means MusicKit's queue ran out
    /// because the next song is one MusicKit was never given (a local file, or the wrap of a
    /// list with local files under repeat all), starts that song and returns true.
    pub(super) fn bridge_queue_ended(&mut self, state: PlayState) -> bool {
        if !matches!(state, PlayState::Ended | PlayState::Completed | PlayState::Stopped) {
            return false;
        }
        let repeat = self.playback.repeat;
        if self.last_station_one_off() {
            // The last song played on its own on a station (replayed from its history, or queued
            // by hand) is over: the station carries on.
            if self.one_off_over() {
                self.resume_station();
                return true;
            }
            return false;
        }
        if repeat == Repeat::One || self.queue.bridge_has_next(repeat) || self.queue.peek_next(repeat).is_none() {
            return false;
        }
        // An old song stopping just after the next one started (its nowPlaying reset the
        // position) is not the end of the queue.
        let near_end = self.playback.duration <= 0.0 || self.playback.position_now() >= self.playback.duration - END_SLACK;
        if !near_end {
            return false;
        }
        match self.queue.advance(repeat) {
            Some(next) => {
                self.begin_current(next);
                true
            }
            None => false,
        }
    }

    /// A station is in a one-off (a song from its history, or songs queued by hand) and the
    /// current song is the last of it: when it ends, the app starts the station again.
    pub(super) fn last_station_one_off(&self) -> bool {
        let repeat = self.playback.repeat;
        repeat != Repeat::One && self.queue.station.as_ref().is_some_and(|r| r.one_off) && self.queue.peek_next(repeat).is_none()
    }

    /// MusicKit reported the end of its queue (an end state, or no song any more) during the last
    /// one-off song: true when that is the song's own end, not the late end state of the song
    /// before it. Near the listed end, or (unless the user paused it) after `OWN_END_AFTER`.
    fn one_off_over(&self) -> bool {
        let p = &self.playback;
        let at = p.position_now();
        p.duration <= 0.0 || at >= p.duration - END_SLACK || (at >= OWN_END_AFTER && p.state != PlayState::Paused)
    }

    /// MusicKit's queue has no song any more (`nowPlaying` none) while no start is in flight:
    /// at the end of the last one-off song on a station, the station carries on. Returns whether
    /// the app took over.
    pub(super) fn bridge_queue_emptied(&mut self) -> bool {
        if self.last_station_one_off() && self.one_off_over() {
            self.resume_station();
            return true;
        }
        false
    }

    // ---------------------------------------------------------------- the library

    /// Adds a music folder (from the folder picker) and rescans.
    pub fn add_music_folder(&mut self, path: PathBuf) {
        if self.settings.music_folders.iter().any(|f| f == &path) {
            return;
        }
        // A folder inside one already listed adds nothing; one containing listed folders replaces them.
        if self.settings.music_folders.iter().any(|f| path.starts_with(f)) {
            self.toast = Some(format!("{} is already inside a music folder.", path.display()));
            return;
        }
        self.settings.music_folders.retain(|f| !f.starts_with(&path));
        self.settings.music_folders.push(path);
        self.save_settings();
        self.rescan_local();
    }

    /// Removes the `i`-th music folder: its songs disappear at once, then the index is rebuilt.
    pub fn remove_music_folder(&mut self, i: usize) {
        if i >= self.settings.music_folders.len() {
            return;
        }
        let gone = self.settings.music_folders.remove(i);
        // Its songs go at once (the view is derived again here, a rare click); the rescan follows.
        let lib = &mut self.local_lib;
        let keep: Vec<usize> = (0..lib.songs.len()).filter(|&k| !lib.songs[k].local_path().is_some_and(|p| p.starts_with(&gone))).collect();
        let album_artists: Vec<String> = keep
            .iter()
            .map(|&k| lib.view.song_album.get(k).and_then(|&a| lib.view.albums.get(a as usize)).map(|c| c.subtitle.clone()).unwrap_or_default())
            .collect();
        let mut songs: Vec<Song> = keep.iter().map(|&k| lib.songs[k].clone()).collect();
        let view = view::derive(&mut songs, &album_artists, &self.settings.music_folders);
        self.set_local_index(songs, view);
        self.save_settings();
        self.rescan_local();
    }

    /// Rescans the music folders in the background (unchanged files are not reopened).
    pub fn rescan_local(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        if self.local_lib.scanning {
            self.local_lib.again = true;
            return;
        }
        self.local_lib.scanning = true;
        self.local_lib.scanned = 0;
        spawn_scan(handle, self.settings.music_folders.clone(), false);
    }

    /// Startup: shows the index from disk, then rescans.
    pub fn load_local(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        self.local_lib.scanning = true;
        spawn_scan(handle, self.settings.music_folders.clone(), true);
    }

    /// New songs and their view (a scan, or a folder removed): pages showing local albums,
    /// artists or folders follow.
    pub fn set_local_index(&mut self, songs: Vec<Song>, view: LocalView) {
        self.local_lib.songs = songs;
        self.local_lib.view = Arc::new(view);
        self.local_lib.loaded = true;
        self.local_lib.generation += 1;
        // Local albums opened before hold the old songs: drop them (the open one is filled again).
        self.collections.retain(|id, _| !view::is_album_id(id));
        self.fill_local_page();
    }

    /// The songs of a local album (`view::ALBUM_PREFIX` id), artist (`view::ARTIST_PREFIX` id) or
    /// folder, in play order. Handlers call this (a few hundred clones); the view never does.
    pub fn local_songs(&self, what: &LocalWhat) -> Vec<Song> {
        let lib = &self.local_lib;
        let v = &lib.view;
        let list: &[u32] = match what {
            LocalWhat::Album(id) => v.album(id).map_or(&[], |a| &v.album_songs[a]),
            LocalWhat::Artist(id) => v.artist(id).map_or(&[], |a| &v.artist_songs[a]),
            LocalWhat::Folder(path) => return view::pick(&lib.songs, &v.folder_songs(path)),
            LocalWhat::FolderFiles(path) => v.folder(path).map_or(&[], |f| &f.files),
        };
        view::pick(&lib.songs, list)
    }

    /// A local album as the album page shows it: the view's entry with its songs.
    pub fn local_album(&self, id: &str) -> Option<Collection> {
        let v = &self.local_lib.view;
        let mut c = v.albums.get(v.album(id)?)?.clone();
        c.tracks = self.local_songs(&LocalWhat::Album(id.to_string()));
        Some(c)
    }

    /// Gives the page on screen its songs, when it is a local album, artist or folder: on every
    /// navigation and whenever the index changes. Never from the view.
    pub(super) fn fill_local_page(&mut self) {
        let what = match &self.route {
            Route::Collection { id, .. } if view::is_album_id(id) => {
                if !matches!(self.collections.get(id), Some(Load::Ready(_))) {
                    let load = match self.local_album(id) {
                        Some(c) => Load::Ready(c),
                        None => Load::Failed("This album is no longer in your music folders.".into()),
                    };
                    self.collections.insert(id.clone(), load);
                }
                return;
            }
            Route::Artist { id, .. } if view::is_artist_id(id) => LocalWhat::Artist(id.clone()),
            Route::Folders { path: Some(p) } => LocalWhat::FolderFiles(p.clone()),
            _ => return,
        };
        let songs = self.local_songs(&what);
        self.local_lib.page = Some((self.route.clone(), songs));
    }
}

/// What `State::local_songs` lists.
#[derive(Clone, Debug, PartialEq)]
pub enum LocalWhat {
    Album(String),
    Artist(String),
    /// Everything in the folder and below it.
    Folder(PathBuf),
    /// The folder's own files only.
    FolderFiles(PathBuf),
}

/// Reads the index (sending it first when `show_cached`), scans, saves and sends the result.
fn spawn_scan(handle: Handle, folders: Vec<PathBuf>, show_cached: bool) {
    std::thread::Builder::new()
        .name("local-scan".into())
        .spawn(move || {
            let index = scan::index_path();
            let previous = scan::load_index(&index);
            if show_cached && !previous.is_empty() {
                let (songs, view) = view::from_index(&previous, &folders);
                handle.update(move |s: &mut State, tx| {
                    s.landing(tx);
                    s.apply_msg(Msg::LocalIndex { songs, view: Box::new(view), scanning: true });
                });
            }
            let progress = {
                let handle = handle.clone();
                move |n: usize| handle.update_if(move |s: &mut State, _| s.apply_msg(Msg::LocalScanProgress(n)))
            };
            let tracks = scan::scan(&folders, &previous, &scan::art_dir(), progress);
            if tracks != previous {
                scan::save_index(&index, &tracks);
            }
            // Grouping runs here, once per scan, never on the UI thread.
            let (songs, view) = view::from_index(&tracks, &folders);
            handle.update(move |s: &mut State, tx| {
                s.landing(tx);
                s.apply_msg(Msg::LocalIndex { songs, view: Box::new(view), scanning: false });
            });
        })
        .ok();
}
