//! AutoPlay (∞ in Up Next, Settings → Playback): when the list ends, similar songs keep playing.
//!
//! Apple's way, as MusicKit JS does it on music.apple.com (`SerialPlaybackController.startAutoplay`
//! and `queueAutoplayTracks` in musickit.js v3): a "continuous" station is made from the last songs
//! of the queue (`POST /v1/me/stations/continuous`, at most 10 seeds), then its songs are asked for
//! (`POST /v1/me/stations/next-tracks/{id}?limit=n`) to keep 10 of them after the queue. None are
//! asked for with more than 50 songs still queued, with repeat on, or on a radio station.
//!
//! MusicKit's own switch (`autoplayEnabled`) isn't used: the bridge hands MusicKit one song at a
//! time (see "queue" in bridge/preload.js), so MusicKit would seed its station from that one song
//! on every click and its suggestions would sit in its queue where the bridge's top-up expects the
//! list's next song. The app asks the same two endpoints itself and keeps the suggestions in
//! `PlayQueue::autoplay`, so Up Next, Next, MPRIS and History treat them like any other song.

use serde_json::{Value, json};

use super::{Msg, Repeat, State};
use crate::models::{Page, Song};

/// Apple's `maxQueueSizeInRequest`: songs of the queue a station is made from.
pub const SEEDS: usize = 10;
/// Apple's `maxUpcomingTracksToMaintain`: suggestions kept after the queue.
pub const UPCOMING: usize = 10;
/// Ask for more once fewer than this many are left (Apple asks after every song; this halves it).
const REFILL_BELOW: usize = 5;
/// Apple's `maxQueueSizeForAutoplay`: with more songs than this still queued, nothing is asked.
pub const MAX_QUEUED: usize = 50;

/// AutoPlay's station for one queue (`PlayQueue::run`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    run: u64,
    /// Apple's continuous station, once made.
    station: Option<String>,
    /// A request is on its way.
    busy: bool,
    /// Apple refused (no subscription, nothing to seed from): no more tries for this queue.
    dead: bool,
}

/// The `/v1/me/stations/continuous` body for `songs` (the last `SEEDS` Apple songs, in order),
/// typed as MusicKit types them; None when there is no Apple song to seed from.
pub fn seeds_body(songs: &[Song]) -> Option<Value> {
    let apple: Vec<&Song> = songs.iter().filter(|s| !s.is_local() && s.playable).collect();
    let data: Vec<Value> = apple[apple.len().saturating_sub(SEEDS)..]
        .iter()
        .map(|s| {
            let id = s.play_id();
            json!({"id": id, "type": if id.starts_with("i.") { "library-songs" } else { "songs" }})
        })
        .collect();
    (!data.is_empty()).then(|| json!({ "data": data }))
}

/// The station id in Apple's answer (`results.station`, one resource or a list of one).
pub fn station_id(v: &Value) -> Option<String> {
    let st = &v["results"]["station"];
    let st = if st.is_array() { &st[0] } else { st };
    st["id"].as_str().or_else(|| v["data"][0]["id"].as_str()).map(String::from)
}

/// The songs in a next-tracks answer (`data`, or `results.tracks`).
pub fn tracks(v: &Value) -> Vec<Song> {
    let mut songs = Page::parse(v, Song::parse).items;
    if songs.is_empty()
        && let Some(a) = v["results"]["tracks"].as_array()
    {
        songs = a.iter().filter_map(Song::parse).collect();
    }
    songs.into_iter().filter(|s| s.playable).collect()
}

impl State {
    /// AutoPlay can play after this queue: the setting is on, repeat is off, it isn't a station
    /// and Apple Music is in use, signed in.
    pub fn autoplay_applies(&self) -> bool {
        self.settings.autoplay
            && self.playback.repeat == Repeat::Off
            && self.queue.station.is_none()
            && !self.local_only()
            && self.session.authorized
    }

    /// Whether the ∞ button shows: Apple Music in use, and not on a radio station (which never ends).
    pub fn autoplay_offered(&self) -> bool {
        !self.local_only() && self.queue.station.is_none()
    }

    /// The ∞ button and Settings → Playback → AutoPlay.
    pub fn set_autoplay(&mut self, on: bool) {
        if self.settings.autoplay == on {
            return;
        }
        self.settings.autoplay = on;
        self.save_settings();
        if !on {
            self.autoplay = Run { run: self.queue.run, ..Run::default() };
        }
        self.sync_upcoming(); // drops the suggestions (off), or asks for them (on)
    }

    pub fn toggle_autoplay(&mut self) {
        self.set_autoplay(!self.settings.autoplay);
    }

    /// Takes AutoPlay song `k` off Up Next; another one is asked for when few are left.
    pub fn remove_autoplay(&mut self, k: usize) {
        if self.queue.remove_autoplay(k) {
            self.sync_upcoming();
        }
    }

    /// After any change of the queue, the song or the settings (from `sync_upcoming` and
    /// `nowPlaying`): drops the suggestions where AutoPlay doesn't apply, else makes the station
    /// or asks for more songs when few are left. Never sends anything to the bridge's queue
    /// itself (the reply does, through `sync_upcoming`).
    pub(super) fn autoplay_check(&mut self) {
        if self.autoplay.run != self.queue.run {
            self.autoplay = Run { run: self.queue.run, ..Run::default() };
        }
        if !self.autoplay_applies() {
            self.queue.autoplay.clear();
            return;
        }
        let wanted = self.queue.autoplay.len() < REFILL_BELOW
            && self.queue.up_next_len() <= MAX_QUEUED
            && self.playback.song.is_some()
            && self.resume.is_none() // a resumed song waits for Play
            && self.growing.is_none() // the list still gets its later pages
            && !self.autoplay.busy
            && !self.autoplay.dead
            && (self.bridge.is_some() || cfg!(test));
        if !wanted {
            return;
        }
        match self.autoplay.station.clone() {
            Some(id) => self.autoplay_more(&id),
            None => {
                let mut seeds: Vec<Song> = self.queue.context.clone();
                seeds.extend(self.queue.user.iter().cloned());
                let Some(body) = seeds_body(&seeds) else {
                    self.autoplay.dead = true; // only local files: nothing to seed from
                    return;
                };
                self.autoplay.busy = true;
                let run = self.queue.run;
                self.autoplay_send("/v1/me/stations/continuous".into(), Some(body), move |r| Msg::AutoplayStation {
                    run,
                    result: r.and_then(|v| station_id(&v).ok_or_else(|| "no station in Apple's answer".into())),
                });
            }
        }
    }

    fn autoplay_more(&mut self, station: &str) {
        let n = UPCOMING.saturating_sub(self.queue.autoplay.len()).max(1);
        self.autoplay.busy = true;
        let run = self.queue.run;
        self.autoplay_send(format!("/v1/me/stations/next-tracks/{station}?limit={n}"), None, move |r| Msg::AutoplayTracks {
            run,
            result: r.map(|v| tracks(&v)),
        });
    }

    /// A POST through the page's MusicKit tokens, its answer stamped with the account it was asked under.
    fn autoplay_send(&self, path: String, body: Option<Value>, done: impl FnOnce(Result<Value, String>) -> Msg + Send + 'static) {
        #[cfg(test)]
        self.sent.borrow_mut().push(match &body {
            Some(b) => format!("POST {path} {b}"),
            None => format!("POST {path}"),
        });
        let account = Some(self.session.account);
        if let Some(b) = &self.bridge {
            b.api_send("POST", &path, body, move |r| Some(super::library::stamp(account, done(r))));
        }
    }

    pub(super) fn autoplay_station(&mut self, run: u64, result: Result<String, String>) {
        if run != self.autoplay.run {
            return; // made for a queue that has been replaced
        }
        self.autoplay.busy = false;
        match result {
            Ok(id) => {
                self.autoplay.station = Some(id);
                self.autoplay_check();
            }
            Err(e) => {
                eprintln!("autoplay: no station: {e}");
                self.autoplay.dead = true;
            }
        }
    }

    pub(super) fn autoplay_tracks(&mut self, run: u64, result: Result<Vec<Song>, String>) {
        if run != self.autoplay.run || run != self.queue.run {
            return;
        }
        self.autoplay.busy = false;
        match result {
            Ok(songs) => {
                if !self.autoplay_applies() {
                    return; // turned off (or repeat on) while asking
                }
                if self.queue.add_autoplay(songs) == 0 {
                    self.autoplay.dead = true; // the station has nothing new: don't ask in a loop
                }
                self.sync_upcoming();
            }
            Err(e) => {
                eprintln!("autoplay: no songs: {e}");
                self.autoplay.dead = true;
            }
        }
    }

    /// Create Station on a song (song menu): Apple's song station `ra.<catalog id>`, as
    /// music.apple.com's "Create Station" plays it. The name is Apple's pattern ("<title> Station").
    pub fn create_song_station(&mut self, song: &Song) {
        let Some(id) = song.catalog_ref() else {
            self.toast = Some(format!("“{}” isn't in the Apple Music catalog.", song.title));
            return;
        };
        let station = crate::models::Station { id: format!("ra.{id}"), name: format!("{} Station", song.title), artwork: song.artwork.clone(), live: false };
        self.play_station(&station);
    }

    /// Create Station on an artist (artist page menu): `ra.<catalog artist id>`, which Apple names
    /// "<name> & Similar Artists Station".
    pub fn create_artist_station(&mut self, catalog_id: &str, name: &str) {
        let station = crate::models::Station { id: format!("ra.{catalog_id}"), name: format!("{name} & Similar Artists Station"), artwork: None, live: false };
        self.play_station(&station);
    }
}
