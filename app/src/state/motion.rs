//! Animated artwork in the state: which albums have a clip (asked once a session), which clips
//! are on disk, the `Playback` in the Now Playing cover and the one in the album page's cover.
//! `ui::poll` calls `poll_motion` and `poll_page_motion` before each frame; they only compare,
//! ask and start or stop a player. Downloads run on the `motion::clips` worker, decoding on the
//! player's own thread. The page's clip is off while Now Playing shows, so one decodes at most.
//!
//! Nothing touches the network or spawns a player in headless runs and tests (no bridge, no
//! handle), except a clip a scene puts in `clips` from a local fixture.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Load, Msg, State};
use crate::bridge::PlayState;
use crate::models::{api, Song};
use crate::settings::AnimatedArtwork;
use crate::models::CollectionKind;
use crate::motion::{self, Clip, Event, Facts, MotionArt, PageFacts, Playback, Want};
use super::Route;

#[derive(Default)]
pub struct MotionState {
    /// Catalog album id → its motion art (None: the album has none). Asked once a session.
    pub albums: HashMap<String, Load<Option<MotionArt>>>,
    /// Master playlist URL → the clip on disk.
    pub clips: HashMap<String, Load<Clip>>,
    pub playback: Option<Playback>,
    /// Now Playing's clip while its overlay slides away (`park`): held on its frame, so the
    /// leaving overlay (which Rune keeps drawing as it was) shows a live video that moves with
    /// it; dropped once the slide is over, or taken back if Now Playing reopens meanwhile.
    closing: Option<(Playback, std::time::Instant)>,
    /// The clip in the cover of the album page being viewed.
    pub page: Option<Playback>,
    /// Library album id (`l.…`) → its catalog album, for its page's clip; pending and
    /// negative results are cached too.
    page_albums: HashMap<String, Load<Option<String>>>,
    /// The album page (its route id) scrolled past its cover (`ui::collection`, from the
    /// scroll offset). Another page shown starts unscrolled.
    pub scrolled_away: Option<String>,
    /// Playing song id → verified catalog album; pending/negative results are cached too.
    song_albums: HashMap<String, Load<Option<String>>>,
    /// Albums whose clip failed to play this session (no ffmpeg, a decode error).
    failed: HashMap<String, String>,
    /// Lookups that failed (MusicKit not ready, offline, a timeout): asked again from this time
    /// on, instead of keeping the album still for the whole session. Keys `a:`/`p:`/`s:` + id.
    retry_at: HashMap<String, std::time::Instant>,
    generation: u64,
    /// The OS's Reduce Motion, as the view last saw it.
    pub reduced_motion: bool,
    /// The window can't draw (minimised, covered), from the frames Rune skips.
    pub covered: bool,
    downloads: Arc<motion::clips::Clips>,
}

/// How long Now Playing's clip stays parked after closing: longer than the overlay's slide out.
pub const CLOSING: std::time::Duration = std::time::Duration::from_millis(900);

/// How long a failed lookup waits before it is asked again.
pub const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

impl MotionState {
    /// Whether the lookup `key` may be asked now (none failed lately).
    fn may_ask(&self, key: &str) -> bool {
        self.retry_at.get(key).is_none_or(|t| crate::clock::now() >= *t)
    }

    /// A lookup failed: forget it (so it is asked again) and wait `RETRY_AFTER` first.
    fn failed_lookup(&mut self, key: String, error: &str) {
        eprintln!("uzumaki: animated artwork: {key} failed ({error}), asking again in {} s", RETRY_AFTER.as_secs());
        self.retry_at.insert(key, crate::clock::now() + RETRY_AFTER);
    }

    /// Time until the next failed lookup may be asked again, for a timer frame.
    pub fn next_retry(&self) -> Option<std::time::Duration> {
        let now = crate::clock::now();
        self.retry_at.values().filter(|t| **t > now).map(|t| *t - now).min()
    }

    /// Stops the clip at once (the setting, the window hiding): the player and its decoder
    /// go; the still cover shows. True when something stopped.
    pub fn stop(&mut self) -> bool {
        let parked = self.closing.take().is_some();
        self.playback.take().is_some() || parked
    }

    /// Now Playing closes: its clip holds its frame (no decoding) through the overlay's slide
    /// out, then goes (`expire_closing`). Stopping it at once left the leaving overlay drawing
    /// a video whose player was gone: on a quick Cmd+Shift+F its area was cut out mid-slide.
    pub fn park(&mut self) {
        if let Some(mut p) = self.playback.take() {
            p.set_paused(true);
            self.closing = Some((p, crate::clock::now() + CLOSING));
        }
    }

    /// Drops the parked clip once the slide is over. True when it went.
    fn expire_closing(&mut self) -> bool {
        if self.closing.as_ref().is_some_and(|(_, until)| crate::clock::now() >= *until) {
            self.closing = None;
            return true;
        }
        false
    }

    /// Time left of the parked clip, for a timer frame that drops it.
    pub fn closing_left(&self) -> Option<std::time::Duration> {
        self.closing.as_ref().map(|(_, until)| until.saturating_duration_since(crate::clock::now()))
    }

    #[cfg(test)]
    pub fn parked(&self) -> bool {
        self.closing.is_some()
    }

    /// Stops the album page's clip (leaving the page, the window hiding). True when one stopped.
    pub fn stop_page(&mut self) -> bool {
        self.page.take().is_some()
    }

    /// The clip layer for the cover of the album page `album` (catalog id), once its first
    /// frame is in.
    pub fn page_layer_for(&self, album: Option<&str>) -> Option<rune::El> {
        let p = self.page.as_ref()?;
        (Some(p.album.as_str()) == album && p.shown()).then(|| p.layer())
    }

    /// The album page's clip before its first frame is drawn (`Playback::warm_layer`).
    pub fn page_warm_layer_for(&self, album: Option<&str>) -> Option<rune::El> {
        let p = self.page.as_ref()?;
        if Some(p.album.as_str()) == album { p.warm_layer() } else { None }
    }

    /// Now Playing's clip before its first frame is drawn (`Playback::warm_layer`).
    pub fn warm_layer_for(&self, album: Option<&str>) -> Option<rune::El> {
        let p = self.playback.as_ref()?;
        if Some(p.album.as_str()) == album { p.warm_layer() } else { None }
    }

    /// Changes when the page's layer comes or goes, for the page's `virtual_rows_version`.
    pub fn page_rev(&self) -> Option<(u64, bool)> {
        self.page.as_ref().map(|p| (p.generation, p.shown()))
    }

    fn slot(&mut self, slot: Slot) -> &mut Option<Playback> {
        match slot {
            Slot::NowPlaying => &mut self.playback,
            Slot::Page => &mut self.page,
        }
    }

    /// The clip layer for the cover of a song on `album`, once its first frame is in.
    pub fn layer_for(&self, album: Option<&str>) -> Option<rune::El> {
        let p = self.playback.as_ref()?;
        (Some(p.album.as_str()) == album && p.shown()).then(|| p.layer())
    }
}

/// Where a clip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    NowPlaying,
    Page,
}

/// A catalog album id (digits only); library (`l.…`) and local ids are not.
fn is_catalog_album(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())
}

/// The catalog album whose clip a song would show: catalog songs know it from their URL (and
/// library songs Apple matched, when the bridge passed the URL on). Local files and library
/// songs with no catalog link have none.
pub fn album_of(song: &crate::models::Song) -> Option<&str> {
    if song.is_local() {
        return None;
    }
    song.album_id.as_deref().filter(|id| is_catalog_album(id))
}

impl State {
    /// Uses only exact Apple ids, never album names or guessed editions.
    pub fn motion_album<'a>(&'a self, song: &'a Song) -> Option<&'a str> {
        album_of(song).or_else(|| {
            if song.is_local() { return None; }
            match self.motion.song_albums.get(&song.id) {
                Some(Load::Ready(Some(album))) => Some(album.as_str()),
                _ => None,
            }
        })
    }

    /// Enrich only the playing song, once. List parsing and Up Next remain cheap.
    fn resolve_motion_album(&mut self) {
        let Some(song) = self.playback.song.as_ref() else { return };
        if song.is_local() || album_of(song).is_some() || self.motion.song_albums.contains_key(&song.id) {
            return;
        }
        if !self.motion_can_ask(&format!("s:{}", song.id)) {
            return;
        }
        // MusicKit's nowPlaying item may omit metadata already present in a loaded list.
        let mapped = self.library_songs.items.iter()
            .find(|other| song.same_track(other) && other.catalog_ref().is_some())
            .unwrap_or(song);
        let known = self.library_songs.items.iter()
            .chain(self.collections.values().filter_map(Load::ready).flat_map(|c| c.tracks.iter()))
            .find(|other| mapped.same_track(other) && album_of(other).is_some());
        if let Some(album) = known.and_then(album_of).map(String::from) {
            self.motion.song_albums.insert(song.id.clone(), Load::Ready(Some(album)));
            return;
        }
        let catalog = song.catalog_ref().map(String::from).or_else(|| {
            self.library_songs.items.iter().find(|other| song.same_track(other))
                .and_then(Song::catalog_ref).map(String::from)
        });
        let path = match catalog {
            Some(id) => format!("/v1/catalog/{{{{storefrontId}}}}/songs/{id}"),
            None if song.id.starts_with("i.") => format!("/v1/me/library/songs/{}/catalog", song.id),
            None => return,
        };
        let id = song.id.clone();
        self.motion.song_albums.insert(id.clone(), Load::Loading);
        self.request((path, serde_json::json!({})), |v| {
            v["data"].as_array().and_then(|items| items.iter().filter_map(Song::parse)
                .find_map(|s| album_of(&s).map(String::from)))
        }, move |result| Msg::MotionSongAlbum { id, result });
    }

    pub(super) fn motion_song_album_known(&mut self, id: String, result: Result<Option<String>, String>) {
        match result {
            Ok(album) => {
                self.motion.song_albums.insert(id, Load::Ready(album));
            }
            Err(e) => {
                self.motion.song_albums.remove(&id);
                self.motion.failed_lookup(format!("s:{id}"), &e);
            }
        }
        self.prefetch_motion();
    }

    /// Apple can be asked about `key`: the bridge is up (a request before MusicKit's page is
    /// ready fails, and one before the bridge exists is dropped without a reply) and the
    /// lookup hasn't failed lately.
    fn motion_can_ask(&self, key: &str) -> bool {
        self.session.bridge_up && self.motion.may_ask(key)
    }

    fn motion_facts(&self) -> Facts {
        Facts {
            enabled: self.settings.animated_artwork.in_now_playing(),
            reduced_motion: self.motion.reduced_motion,
            now_playing: self.ui.now_playing,
            // Closing to the tray also closes Now Playing (`hide_on_close`); this catches the rest.
            hidden: self.handle.as_ref().is_some_and(|h| !h.is_visible()),
            covered: self.motion.covered,
            playing: matches!(self.playback.state, PlayState::Playing),
        }
    }

    /// Before each frame: what the cover should do, and the step towards it (ask Apple, ask the
    /// download worker, start, pause or stop the player). True when the view must change.
    pub fn poll_motion(&mut self) -> bool {
        self.prefetch_motion();
        let want = motion::want(&self.motion_facts());
        let album = self.playback.song.as_ref().and_then(|song| self.motion_album(song)).map(String::from);
        let expired = self.motion.expire_closing();
        let (Some(album), false) = (album, want == Want::Off) else {
            return self.motion.playback.take().is_some() || expired;
        };
        // Reopened while the old overlay still slides away: the same clip, no new fade.
        if self.motion.playback.is_none() && self.motion.closing.as_ref().is_some_and(|(p, _)| p.album == album) {
            self.motion.playback = self.motion.closing.take().map(|(p, _)| p);
        }
        self.drive(Slot::NowPlaying, album, want) || expired
    }

    /// One step of `slot`'s clip towards `want` (not Off) for `album`: stop another album's,
    /// ask for this one's clip, then start, pause, resume or fall back. True when the view changes.
    fn drive(&mut self, slot: Slot, album: String, want: Want) -> bool {
        let mut changed = false;
        if self.motion.slot(slot).as_ref().is_some_and(|p| p.album != album) {
            changed |= self.motion.slot(slot).take().is_some(); // another album now: back to its still
        }
        let Some(clip) = self.motion_clip(&album) else { return changed };
        if self.motion.failed.contains_key(&album) {
            return changed;
        }
        let paused = want == Want::Paused;
        let handle = self.handle.clone();
        let next = self.motion.generation + 1;
        match self.motion.slot(slot) {
            Some(p) => {
                p.set_paused(paused);
                changed |= p.catch_up();
                #[cfg(target_os = "macos")]
                if p.check_native(&clip, report(handle)) {
                    changed = true;
                }
            }
            // A clip starts only to play; a paused one keeps its frame, but none is started.
            None if !paused => {
                let wait = handle.is_some() && !rune::is_headless();
                let p = Playback::start(album, next, &clip, report(handle), wait);
                changed |= p.shown();
                *self.motion.slot(slot) = Some(p);
                self.motion.generation = next;
            }
            None => {}
        }
        changed
    }

    fn page_facts(&self) -> PageFacts {
        PageFacts {
            enabled: self.settings.animated_artwork.on_album_pages(),
            reduced_motion: self.motion.reduced_motion,
            local_only: self.local_only(),
            album_page: matches!(&self.route, Route::Collection { kind: CollectionKind::Album, .. }),
            now_playing: self.ui.now_playing,
            hidden: self.handle.as_ref().is_some_and(|h| !h.is_visible()),
            covered: self.motion.covered,
            scrolled_away: match &self.route {
                Route::Collection { id, .. } => self.motion.scrolled_away.as_ref() == Some(id),
                _ => false,
            },
        }
    }

    /// An album page is shown (its cover may want a clip).
    pub fn page_facts_album_page(&self) -> bool {
        matches!(&self.route, Route::Collection { kind: CollectionKind::Album, .. })
    }

    /// Before each frame, for the album page being viewed: its cover's clip (asked about, then
    /// downloaded, only for this page; never for lists or grids). True when the view must change.
    pub fn poll_page_motion(&mut self) -> bool {
        let want = motion::want_page(&self.page_facts());
        let album = if want == Want::Off { None } else { self.page_album() };
        match album {
            Some(album) => self.drive(Slot::Page, album, want),
            None => self.motion.stop_page(),
        }
    }

    /// The catalog album of the album page shown: its own id, or for a library album the
    /// catalog album it maps to (exact ids only, asked once a session).
    pub fn page_album(&mut self) -> Option<String> {
        let Route::Collection { kind: CollectionKind::Album, id } = &self.route else { return None };
        if is_catalog_album(id) {
            return Some(id.clone());
        }
        if !id.starts_with("l.") {
            return None; // a local album
        }
        let id = id.clone();
        if !self.motion.page_albums.contains_key(&id) {
            self.resolve_page_album(&id); // known at once from loaded songs, or asked
        }
        match self.motion.page_albums.get(&id) {
            Some(Load::Ready(album)) => album.clone(),
            _ => None,
        }
    }

    /// The catalog album a library album's page shows the clip of: a catalog album id one of
    /// its loaded songs already carries, else asked: the catalog song of its first track that
    /// has one (the album that exact song is on), else the library album's catalog link.
    fn resolve_page_album(&mut self, id: &str) {
        let Some(c) = self.collections.get(id).and_then(Load::ready) else { return }; // wait for the page
        let known = c.tracks.iter().find_map(|t| album_of(t).map(String::from)).or_else(|| {
            // The same songs in a loaded catalog album or playlist, by catalog id.
            let ids: Vec<&str> = c.tracks.iter().filter_map(Song::catalog_ref).collect();
            self.collections.values().filter_map(Load::ready).flat_map(|o| o.tracks.iter())
                .find(|t| t.catalog_ref().is_some_and(|r| ids.contains(&r)) && album_of(t).is_some())
                .and_then(album_of).map(String::from)
        });
        if let Some(album) = known {
            self.motion.page_albums.insert(id.to_string(), Load::Ready(Some(album)));
            return;
        }
        if !self.motion_can_ask(&format!("p:{id}")) {
            return;
        }
        let path = match c.tracks.iter().find_map(Song::catalog_ref) {
            Some(song) => format!("/v1/catalog/{{{{storefrontId}}}}/songs/{song}"),
            None => format!("/v1/me/library/albums/{id}/catalog"),
        };
        self.motion.page_albums.insert(id.to_string(), Load::Loading);
        let reply = id.to_string();
        self.request((path, serde_json::json!({})), |v| {
            let items = v["data"].as_array();
            // A song: the album in its URL. The album link: the album's own id.
            items.and_then(|items| items.iter().filter_map(Song::parse).find_map(|s| album_of(&s).map(String::from)))
                .or_else(|| v["data"][0]["id"].as_str().filter(|_| v["data"][0]["type"] == "albums")
                    .filter(|a| is_catalog_album(a)).map(String::from))
        }, move |result| Msg::MotionPageAlbum { id: reply, result });
    }

    /// For the view (asks nothing): the catalog album whose clip album page `c` shows, if known.
    pub fn page_album_known(&self, c: &crate::models::Collection) -> Option<String> {
        if is_catalog_album(&c.id) {
            return Some(c.id.clone());
        }
        match self.motion.page_albums.get(&c.id) {
            Some(Load::Ready(album)) => album.clone(),
            _ => None,
        }
    }

    /// A library album's catalog album arrived. True when its page shows (a frame then takes
    /// the next step).
    pub(super) fn motion_page_album_known(&mut self, id: String, result: Result<Option<String>, String>) -> bool {
        let shown = matches!(&self.route, Route::Collection { id: r, .. } if *r == id);
        match result {
            Ok(album) => {
                self.motion.page_albums.insert(id, Load::Ready(album));
            }
            Err(e) => {
                self.motion.page_albums.remove(&id);
                self.motion.failed_lookup(format!("p:{id}"), &e);
            }
        }
        shown
    }

    /// Whether `album` (catalog id) is the album page's, so news about it needs a frame.
    pub(super) fn page_shows(&self, album: &str) -> bool {
        let Route::Collection { kind: CollectionKind::Album, id } = &self.route else { return false };
        id == album || matches!(self.motion.page_albums.get(id), Some(Load::Ready(Some(a))) if a == album)
    }

    /// Gets clips onto the disk before Now Playing opens: while a song plays, the clip of its
    /// album and of the next song's in Up Next (each album asked once a session). Only asks
    /// and downloads; a player starts only in Now Playing (`poll_motion`). Off with the
    /// setting or Reduce Motion, and for local files (`album_of`).
    fn prefetch_motion(&mut self) {
        let playing = matches!(self.playback.state, PlayState::Playing);
        if !playing || !self.settings.animated_artwork.in_now_playing() || self.motion.reduced_motion {
            return;
        }
        self.resolve_motion_album();
        let current = self.playback.song.as_ref().and_then(|song| self.motion_album(song)).map(String::from);
        let next = self.queue.peek_next(self.playback.repeat).and_then(album_of).map(String::from);
        for album in [current, next].into_iter().flatten() {
            self.motion_clip(&album);
        }
    }

    /// `album`'s clip once it is on disk; until then the next step towards it: ask Apple about
    /// the album, then download its clip (each once a session).
    fn motion_clip(&mut self, album: &str) -> Option<Clip> {
        let video = match self.motion.albums.get(album) {
            Some(Load::Ready(Some(art))) => art.video.clone(),
            Some(_) => return None,
            None => {
                if self.motion_can_ask(&format!("a:{album}")) {
                    self.ask_motion(album);
                }
                return None;
            }
        };
        match self.motion.clips.get(&video) {
            Some(Load::Ready(clip)) => Some(clip.clone()),
            Some(_) => None,
            None => {
                if self.motion.may_ask(&format!("c:{video}")) {
                    self.download_clip(&video);
                }
                None
            }
        }
    }

    /// Asks Apple for an album's motion art (only its `editorialVideo`).
    fn ask_motion(&mut self, album: &str) {
        self.motion.albums.insert(album.to_string(), Load::Loading);
        let reply = album.to_string();
        self.request(api::album_motion(album), MotionArt::from_reply, move |result| Msg::Motion { album: reply, result });
    }

    fn download_clip(&mut self, master: &str) {
        self.motion.clips.insert(master.to_string(), Load::Loading);
        let Some(h) = self.handle.clone().filter(|_| !rune::is_headless()) else { return };
        let key = master.to_string();
        self.motion.downloads.request(master.to_string(), move |result| {
            h.update(move |s: &mut State, _| {
                match result {
                    Ok(c) => {
                        s.motion.clips.insert(key, Load::Ready(c));
                    }
                    Err(e) => {
                        s.motion.clips.remove(&key);
                        s.motion.failed_lookup(format!("c:{key}"), &e);
                    }
                }
            });
        });
    }

    /// An album's motion art arrived (its own request, or an album page asked with it).
    pub(super) fn motion_known(&mut self, album: String, result: Result<Option<MotionArt>, String>) {
        match result {
            Ok(m) => {
                self.motion.albums.insert(album, Load::Ready(m));
            }
            Err(e) => {
                self.motion.albums.remove(&album);
                self.motion.failed_lookup(format!("a:{album}"), &e);
            }
        }
        // The download starts now, not at the next frame (none may come while Now Playing is closed).
        self.prefetch_motion();
    }

    /// A player's report (`motion::Event`) for playback `generation`. True when the view changes.
    pub fn motion_event(&mut self, generation: u64, event: Event) -> bool {
        let slot = [Slot::NowPlaying, Slot::Page].into_iter()
            .find(|&slot| self.motion.slot(slot).as_ref().is_some_and(|p| p.generation == generation));
        let Some(slot) = slot else {
            // A parked clip failing: just let it go.
            if matches!(event, Event::Failed(_)) && self.motion.closing.as_ref().is_some_and(|(p, _)| p.generation == generation) {
                self.motion.closing = None;
            }
            return false;
        };
        let p = self.motion.slot(slot).as_mut().expect("found above");
        match event {
            Event::FirstFrame => p.first_frame(),
            Event::Failed(e) => {
                let album = p.album.clone();
                self.motion.failed.insert(album, e);
                self.motion.slot(slot).take().is_some()
            }
        }
    }

    /// Settings → Appearance → Animated artwork. A loop where the choice no longer lets it play
    /// stops at once (the player and decoder go, the still cover shows); where it still may, it
    /// plays on. Choosing a place back starts its loop the normal way, from `poll`.
    pub fn set_animated_artwork(&mut self, mode: AnimatedArtwork) {
        if self.settings.animated_artwork == mode {
            return;
        }
        self.settings.animated_artwork = mode;
        self.save_settings();
        if !mode.in_now_playing() {
            self.motion.stop();
        }
        if !mode.on_album_pages() {
            self.motion.stop_page();
        }
    }

    /// From the view, which alone knows the OS's Reduce Motion: hands a change over on the
    /// next turn (the view can't change state).
    pub fn report_reduced_motion(&self, reduced: bool) {
        if let (true, Some(h)) = (self.motion.reduced_motion != reduced, &self.handle) {
            h.update(move |s: &mut State, _| s.motion.reduced_motion = reduced);
        }
    }
}

/// A player's reports go to the UI thread (nowhere without a handle: headless).
fn report(handle: Option<super::Handle>) -> motion::Report {
    Box::new(move |generation, event| {
        if let Some(h) = &handle {
            h.update_if(move |s: &mut State, _| s.motion_event(generation, event));
        }
    })
}

/// `App::on_frame_encoded`: Rune skips frames while the window is minimised, covered or on
/// another Space. Two skips in a row pause the clip; the next frame drawn (or found
/// unchanged) resumes it. While paused for that, a timer frame each second checks again
/// (`ui::now_playing::poll`).
pub fn frame_outcome(handle: &super::Handle, outcome: rune::FrameOutcome) {
    use std::cell::Cell;
    thread_local! {
        static SKIPS: Cell<u32> = const { Cell::new(0) };
        static COVERED: Cell<bool> = const { Cell::new(false) };
    }
    let skips = match outcome {
        rune::FrameOutcome::Skipped => SKIPS.get() + 1,
        _ => 0,
    };
    SKIPS.set(skips);
    let covered = if skips == 0 { false } else if skips >= 2 { true } else { COVERED.get() };
    if covered != COVERED.replace(covered) {
        handle.update_if(move |s: &mut State, _| {
            s.motion.covered = covered;
            let np = s.poll_motion();
            s.poll_page_motion() || np || !covered
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Song;

    const MASTER: &str = "https://mvod.example/a/P1_default.m3u8";

    fn song(id: &str, album: Option<&str>) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: format!("Song {id}"),
            artist: "A".into(),
            album: "B".into(),
            album_is_fallback: false,
            duration: 200.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: album.map(String::from),
            artists: Vec::new(),
        }
    }

    fn motion_asks(s: &State) -> usize {
        s.sent.borrow().iter().filter(|p| p.contains("editorialVideo") && p.contains("fields[albums]")).count()
    }

    /// Now Playing open on a playing catalog song of album 594061854.
    fn playing() -> State {
        let mut s = State::offline();
        s.playback.song = Some(song("1", Some("594061854")));
        s.playback.state = PlayState::Playing;
        s.ui.now_playing = true;
        s.session.bridge_up = true;
        s
    }

    /// `playing()` with the album's clip known and on disk (a path nothing reads: tests start
    /// no decoder; the Mac's player only opens a file when drawn).
    fn ready() -> State {
        let mut s = playing();
        s.motion.albums.insert("594061854".into(), Load::Ready(Some(MotionArt { video: MASTER.into(), preview: None })));
        let clip = Clip { path: "/nonexistent/clip.mp4".into(), width: 768, height: 768, fps: 23.976 };
        s.motion.clips.insert(MASTER.into(), Load::Ready(clip));
        s
    }

    #[test]
    fn opening_now_playing_asks_for_the_albums_motion_art_once() {
        let mut s = playing();
        s.poll_motion();
        s.poll_motion();
        assert_eq!(motion_asks(&s), 1);
        assert!(s.sent.borrow()[0].starts_with("/v1/catalog/{{storefrontId}}/albums/594061854 "));
        assert_eq!(s.motion.albums["594061854"], Load::Loading);
    }

    #[test]
    fn nothing_is_asked_when_it_could_not_play() {
        type Case = (&'static str, fn(&mut State));
        let cases: [Case; 5] = [
            ("setting off", |s| s.settings.animated_artwork = AnimatedArtwork::Off),
            ("reduce motion", |s| s.motion.reduced_motion = true),
            ("now playing closed, song paused", |s| {
                s.ui.now_playing = false;
                s.playback.state = PlayState::Paused;
            }),
            ("local file", |s| s.playback.song = Some(song("file:///m/a.flac", Some("local-album")))),
            ("library song with no catalog album", |s| s.playback.song = Some(song("i.abc", None))),
        ];
        for (what, f) in cases {
            let mut s = playing();
            f(&mut s);
            s.poll_motion();
            assert_eq!(motion_asks(&s), 0, "{what}");
            assert!(s.motion.albums.is_empty(), "{what}");
        }
    }

    #[test]
    fn an_album_without_motion_art_keeps_its_still() {
        let mut s = playing();
        s.poll_motion();
        s.apply_msg(Msg::Motion { album: "594061854".into(), result: Ok(None) });
        assert!(!s.poll_motion());
        assert!(s.motion.clips.is_empty(), "no download");
        assert!(s.motion.playback.is_none());
        assert_eq!(motion_asks(&s), 1, "not asked again");
    }

    #[test]
    fn motion_art_found_asks_for_the_clip_without_downloading_headless() {
        let mut s = playing();
        s.poll_motion();
        s.apply_msg(Msg::Motion { album: "594061854".into(), result: Ok(Some(MotionArt { video: MASTER.into(), preview: None })) });
        s.poll_motion();
        assert_eq!(s.motion.clips[MASTER], Load::Loading, "headless: marked, never fetched");
        assert!(s.motion.playback.is_none());
    }

    #[test]
    fn an_album_page_brings_its_motion_art_along() {
        let mut s = State::offline();
        let c = crate::models::Collection {
            kind: crate::models::CollectionKind::Album,
            id: "594061854".into(),
            title: "Rumours".into(),
            subtitle: "Fleetwood Mac".into(),
            artwork: None,
            year: None,
            genre: None,
            description: None,
            track_count: None,
            modified: None,
            editable: false,
            favorites: false,
            public: None,
            artists: Vec::new(),
            tracks: Vec::new(),
        };
        let art = MotionArt { video: MASTER.into(), preview: None };
        s.apply_msg(Msg::AlbumPage { id: "594061854".into(), motion: Some(art.clone()), result: Ok((c, None)) });
        assert_eq!(s.motion.albums["594061854"], Load::Ready(Some(art)));
        assert!(matches!(s.collections.get("594061854"), Some(Load::Ready(_))));
        // Its own request is then never needed.
        s.playback.song = Some(song("1", Some("594061854")));
        s.playback.state = PlayState::Playing;
        s.ui.now_playing = true;
        s.poll_motion();
        assert_eq!(motion_asks(&s), 0);
    }

    #[test]
    fn a_ready_clip_plays_pauses_and_stops_with_the_rules() {
        let mut s = ready();
        s.poll_motion();
        let p = s.motion.playback.as_ref().expect("started");
        assert_eq!(p.album, "594061854");
        assert!(!p.paused());

        s.playback.state = PlayState::Paused;
        s.poll_motion();
        assert!(s.motion.playback.as_ref().unwrap().paused(), "music paused: holds");
        s.playback.state = PlayState::Playing;
        s.poll_motion();
        assert!(!s.motion.playback.as_ref().unwrap().paused());

        s.motion.covered = true;
        s.poll_motion();
        assert!(s.motion.playback.as_ref().unwrap().paused(), "minimised or covered: holds");
        s.motion.covered = false;
        s.poll_motion();
        assert!(!s.motion.playback.as_ref().unwrap().paused());

        crate::ui::close_now_playing(&mut s);
        assert!(s.motion.playback.is_none(), "closing Now Playing takes it out of the slot at once");
        assert!(s.motion.parked(), "held on its frame while the overlay slides away");
    }

    #[test]
    fn a_closed_now_playing_clip_is_dropped_after_the_slide_or_taken_back() {
        crate::clock::freeze();
        let mut s = ready();
        s.poll_motion();
        let g = s.motion.playback.as_ref().unwrap().generation;
        crate::ui::close_now_playing(&mut s);
        s.poll_motion();
        assert!(s.motion.parked(), "kept through the slide");
        crate::ui::open_now_playing(&mut s);
        s.poll_motion();
        assert_eq!(s.motion.playback.as_ref().unwrap().generation, g, "reopened: the same clip");
        assert!(!s.motion.parked());
        assert!(!s.motion.playback.as_ref().unwrap().paused(), "and it plays on");

        crate::ui::close_now_playing(&mut s);
        crate::clock::advance(CLOSING);
        assert!(s.poll_motion(), "the view changes");
        assert!(!s.motion.parked() && s.motion.playback.is_none(), "gone after the slide");

        // Hiding to the tray: gone at once.
        let mut s = ready();
        s.poll_motion();
        s.background = true;
        assert!(s.hide_on_close());
        assert!(!s.motion.parked() && s.motion.playback.is_none());
        // The setting too.
        let mut s = ready();
        s.poll_motion();
        crate::ui::close_now_playing(&mut s);
        s.set_animated_artwork(AnimatedArtwork::Off);
        assert!(!s.motion.parked());
    }

    #[test]
    fn a_paused_song_starts_no_clip() {
        let mut s = ready();
        s.playback.state = PlayState::Paused;
        s.poll_motion();
        assert!(s.motion.playback.is_none());
    }

    #[test]
    fn the_setting_and_reduce_motion_stop_a_running_clip() {
        let mut s = ready();
        s.poll_motion();
        s.set_animated_artwork(AnimatedArtwork::Off);
        assert!(s.motion.playback.is_none());
        s.set_animated_artwork(AnimatedArtwork::Always);
        s.poll_motion();
        assert!(s.motion.playback.is_some());
        s.motion.reduced_motion = true;
        assert!(s.poll_motion(), "the view changes back to the still");
        assert!(s.motion.playback.is_none());
    }

    #[test]
    fn another_album_stops_the_clip_and_the_same_album_keeps_it() {
        let mut s = ready();
        s.poll_motion();
        let g0 = s.motion.playback.as_ref().unwrap().generation;
        s.playback.song = Some(song("2", Some("594061854")));
        s.poll_motion();
        assert_eq!(s.motion.playback.as_ref().unwrap().generation, g0, "same album: same clip");
        s.playback.song = Some(song("3", Some("1440857781")));
        s.poll_motion();
        assert!(s.motion.playback.is_none());
        assert_eq!(motion_asks(&s), 1, "the new album is asked about");
    }

    #[test]
    fn a_failed_player_leaves_the_still_for_that_album() {
        let mut s = ready();
        s.poll_motion();
        let g0 = s.motion.playback.as_ref().unwrap().generation;
        assert!(!s.motion_event(g0 + 1, Event::Failed("old".into())), "a late report of an older clip is ignored");
        assert!(s.motion_event(g0, Event::Failed("ffmpeg not found".into())));
        assert!(s.motion.playback.is_none());
        s.poll_motion();
        assert!(s.motion.playback.is_none(), "not retried this session");
    }

    #[test]
    fn the_layer_shows_only_for_its_album_once_the_first_frame_is_in() {
        let mut s = ready();
        s.poll_motion();
        let g0 = s.motion.playback.as_ref().unwrap().generation;
        let native = cfg!(target_os = "macos") && std::env::var("UZUMAKI_MOTION").as_deref() != Ok("ffmpeg");
        assert_eq!(s.motion.layer_for(Some("594061854")).is_some(), native, "the Mac's player shows its poster at once");
        s.motion_event(g0, Event::FirstFrame);
        assert!(s.motion.layer_for(Some("594061854")).is_some());
        assert!(s.motion.layer_for(Some("1440857781")).is_none());
        assert!(s.motion.layer_for(None).is_none());
    }

    fn art() -> MotionArt {
        MotionArt { video: MASTER.into(), preview: None }
    }

    /// A catalog song of album 594061854 playing, Now Playing closed.
    fn playing_closed() -> State {
        let mut s = playing();
        s.ui.now_playing = false;
        s
    }

    fn albums_asked(s: &State) -> Vec<String> {
        let sent = s.sent.borrow();
        let asks = sent.iter().filter(|p| p.contains("fields[albums]"));
        asks.map(|p| p.split_whitespace().next().unwrap().rsplit('/').next().unwrap().to_string()).collect()
    }

    #[test]
    fn a_playing_song_gets_its_clip_downloaded_before_now_playing_opens() {
        let mut s = playing_closed();
        s.poll_motion();
        s.poll_motion();
        assert_eq!(albums_asked(&s), ["594061854"], "asked once");
        // The reply alone starts the download: no frame may come while Now Playing is closed.
        s.apply_msg(Msg::Motion { album: "594061854".into(), result: Ok(Some(art())) });
        assert_eq!(s.motion.clips[MASTER], Load::Loading, "download asked");
        assert!(s.motion.playback.is_none(), "no player while Now Playing is closed");
        s.poll_motion();
        assert!(s.motion.playback.is_none());
        assert_eq!(albums_asked(&s).len(), 1);
    }

    #[test]
    fn a_prefetched_clip_plays_when_now_playing_opens_without_asking_again() {
        let mut s = playing_closed();
        s.poll_motion();
        s.apply_msg(Msg::Motion { album: "594061854".into(), result: Ok(Some(art())) });
        let clip = Clip { path: "/nonexistent/clip.mp4".into(), width: 768, height: 768, fps: 23.976 };
        s.motion.clips.insert(MASTER.into(), Load::Ready(clip)); // the worker's reply
        s.poll_motion();
        assert!(s.motion.playback.is_none());
        s.ui.now_playing = true;
        s.poll_motion();
        assert!(s.motion.playback.is_some(), "starts at once");
        assert_eq!(albums_asked(&s).len(), 1);
    }

    #[test]
    fn nothing_is_prefetched_against_the_rules() {
        type Case = (&'static str, fn(&mut State));
        let cases: [Case; 6] = [
            ("setting off", |s| s.settings.animated_artwork = AnimatedArtwork::Off),
            ("reduce motion", |s| s.motion.reduced_motion = true),
            ("local file", |s| s.playback.song = Some(song("file:///m/a.flac", Some("123")))),
            ("paused", |s| s.playback.state = PlayState::Paused),
            ("loading (skipping through songs)", |s| s.playback.state = PlayState::Loading),
            ("stopped", |s| s.playback.state = PlayState::Stopped),
        ];
        for (what, f) in cases {
            let mut s = playing_closed();
            f(&mut s);
            s.poll_motion();
            assert!(albums_asked(&s).is_empty(), "{what}");
            // A reply that came anyway (an album page) starts no download either.
            s.apply_msg(Msg::Motion { album: "594061854".into(), result: Ok(Some(art())) });
            assert!(s.motion.clips.is_empty(), "{what}");
        }
    }

    #[test]
    fn the_next_song_in_up_next_is_prefetched_too() {
        let mut s = playing_closed();
        let list = [song("1", Some("594061854")), song("2", Some("594061854")), song("3", Some("1440783617"))];
        s.queue = crate::state::queue::PlayQueue::start(&list, 0, false, "test", Default::default());
        s.poll_motion();
        assert_eq!(albums_asked(&s), ["594061854"], "next song on the same album: asked once");
        s.queue.pos = 1; // song 2 plays; song 3 is next, on another album
        s.playback.song = Some(list[1].clone());
        s.poll_motion();
        assert_eq!(albums_asked(&s), ["594061854", "1440783617"]);
        s.poll_motion();
        assert_eq!(albums_asked(&s).len(), 2, "each album once a session");
        // A local file next asks nothing.
        s.queue.user.push_back(song("file:///m/b.flac", Some("99")));
        s.poll_motion();
        assert_eq!(albums_asked(&s).len(), 2);
    }

    #[test]
    fn library_playback_reuses_exact_standard_catalog_mapping() {
        let mut s = playing_closed();
        let mut library = song("i.library", None);
        library.catalog_id = Some("123".into());
        s.library_songs.items.push(library);
        // MusicKit omitted both catalogId and albumId in the playing event.
        s.playback.song = Some(song("i.library", None));
        let mut album = crate::models::Collection::parse(&serde_json::json!({
            "id":"456", "type":"albums", "attributes":{"name":"Standard"}
        })).unwrap();
        album.tracks.push(song("123", Some("456")));
        // A different edition with the same title must never win by name.
        album.tracks.push(song("999", Some("888")));
        s.collections.insert("456".into(), Load::Ready(album));
        s.poll_motion();
        assert_eq!(s.motion_album(s.playback.song.as_ref().unwrap()), Some("456"));
        assert_eq!(albums_asked(&s), ["456"]);
        assert_eq!(s.sent.borrow().len(), 1, "no song lookup when mapping is loaded");
        assert!(s.motion.playback.is_none(), "prefetch never decodes");
    }

    #[test]
    fn missing_album_uses_known_catalog_id_once_for_playing_song_only() {
        let mut s = playing_closed();
        let mut library = song("i.library", None);
        library.catalog_id = Some("123".into());
        s.playback.song = Some(library);
        s.queue.user.push_back(song("i.next", None));
        s.poll_motion();
        s.poll_motion();
        assert_eq!(s.sent.borrow().len(), 1);
        assert!(s.sent.borrow()[0].starts_with("/v1/catalog/{{storefrontId}}/songs/123 "));
        s.apply_msg(Msg::MotionSongAlbum { id: "i.library".into(), result: Ok(Some("456".into())) });
        assert_eq!(albums_asked(&s), ["456"]);
        assert!(s.motion.playback.is_none());
        s.poll_motion();
        assert_eq!(s.sent.borrow().len(), 2, "cached song and album lookups");
    }

    #[test]
    fn unmatched_library_lookup_and_negative_or_failed_results_are_not_asked_at_once() {
        for result in [Ok(None), Err("offline".into())] {
            let mut s = playing_closed();
            s.playback.song = Some(song("i.library", None));
            s.poll_motion();
            assert!(s.sent.borrow()[0].starts_with("/v1/me/library/songs/i.library/catalog "));
            s.apply_msg(Msg::MotionSongAlbum { id: "i.library".into(), result });
            s.poll_motion();
            s.poll_motion();
            assert_eq!(s.sent.borrow().len(), 1);
            assert!(s.motion.playback.is_none());
        }
    }

    #[test]
    fn late_mapping_for_previous_song_does_not_change_current_cover() {
        let mut s = playing_closed();
        s.playback.song = Some(song("i.old", None));
        s.poll_motion();
        s.playback.song = Some(song("2", Some("222")));
        s.apply_msg(Msg::MotionSongAlbum { id: "i.old".into(), result: Ok(Some("111".into())) });
        assert_eq!(s.motion_album(s.playback.song.as_ref().unwrap()), Some("222"));
        assert_eq!(albums_asked(&s), ["222"]);
    }

    fn collection(id: &str, tracks: Vec<Song>) -> crate::models::Collection {
        let mut c = crate::models::Collection::parse(&serde_json::json!({
            "id": id, "type": "albums", "attributes": {"name": "Rumours"}
        })).unwrap();
        c.tracks = tracks;
        c
    }

    /// Album page 594061854 shown (its page loaded), nothing playing, Now Playing closed.
    fn on_page(id: &str) -> State {
        let mut s = State::offline();
        s.collections.insert(id.into(), Load::Ready(collection(id, Vec::new())));
        s.route = Route::Collection { kind: CollectionKind::Album, id: id.into() };
        s.session.bridge_up = true;
        s
    }

    /// `on_page` with the album's clip known and on disk.
    fn page_ready() -> State {
        let mut s = on_page("594061854");
        s.motion.albums.insert("594061854".into(), Load::Ready(Some(art())));
        let clip = Clip { path: "/nonexistent/clip.mp4".into(), width: 768, height: 768, fps: 23.976 };
        s.motion.clips.insert(MASTER.into(), Load::Ready(clip));
        s
    }

    #[test]
    fn an_album_page_asks_for_its_own_clip_once_and_plays_it_with_no_music() {
        let mut s = on_page("594061854");
        s.poll_page_motion();
        s.poll_page_motion();
        assert_eq!(albums_asked(&s), ["594061854"], "asked once (a cached page has no motion art yet)");
        assert!(s.page_shows("594061854"), "its reply needs a frame");
        let mut s = page_ready();
        s.poll_page_motion();
        let p = s.motion.page.as_ref().expect("plays with nothing playing");
        assert_eq!(p.album, "594061854");
        assert!(s.motion.playback.is_none(), "not the Now Playing slot");
    }

    #[test]
    fn the_album_page_clip_follows_the_rules() {
        type Case = (&'static str, fn(&mut State));
        let off: [Case; 7] = [
            ("setting off", |s| s.settings.animated_artwork = AnimatedArtwork::Off),
            ("Now Playing only", |s| s.settings.animated_artwork = AnimatedArtwork::NowPlaying),
            ("reduce motion", |s| s.motion.reduced_motion = true),
            ("local only mode", |s| s.settings.music_source = crate::settings::MusicSource::LocalOnly),
            ("Now Playing open", |s| s.ui.now_playing = true),
            ("another page", |s| s.route = Route::Home),
            ("a playlist page", |s| s.route = Route::Collection { kind: CollectionKind::Playlist, id: "594061854".into() }),
        ];
        for (what, f) in off {
            let mut s = page_ready();
            s.poll_page_motion();
            assert!(s.motion.page.is_some());
            f(&mut s);
            assert!(s.poll_page_motion(), "{what}: the view goes back to the still");
            assert!(s.motion.page.is_none(), "{what}: stopped, decoder gone");
            // And nothing is asked from scratch.
            let mut s = on_page("594061854");
            f(&mut s);
            s.poll_page_motion();
            assert!(albums_asked(&s).is_empty(), "{what}");
        }
        let mut s = page_ready();
        s.poll_page_motion();
        s.motion.covered = true;
        s.poll_page_motion();
        assert!(s.motion.page.as_ref().unwrap().paused(), "minimised: holds");
        s.motion.covered = false;
        s.motion.scrolled_away = Some("594061854".into());
        s.poll_page_motion();
        assert!(s.motion.page.as_ref().unwrap().paused(), "scrolled past the cover: holds");
        s.motion.scrolled_away = Some("other".into());
        s.poll_page_motion();
        assert!(!s.motion.page.as_ref().unwrap().paused(), "another page's scroll doesn't count");
        // Scrolled away before it started: none starts.
        let mut s = page_ready();
        s.motion.scrolled_away = Some("594061854".into());
        s.poll_page_motion();
        assert!(s.motion.page.is_none());
    }

    #[test]
    fn only_one_clip_decodes_and_the_page_resumes_after_now_playing() {
        let mut s = page_ready();
        s.playback.song = Some(song("1", Some("594061854")));
        s.playback.state = PlayState::Playing;
        s.poll_page_motion();
        s.poll_motion();
        assert!(s.motion.page.is_some() && s.motion.playback.is_none());
        crate::ui::open_now_playing(&mut s);
        s.poll_motion();
        s.poll_page_motion();
        assert!(s.motion.playback.is_some() && s.motion.page.is_none());
        crate::ui::close_now_playing(&mut s);
        s.poll_motion();
        s.poll_page_motion();
        assert!(s.motion.playback.is_none() && s.motion.page.is_some());
    }

    #[test]
    fn closing_to_the_tray_stops_the_page_clip_at_once() {
        let mut s = page_ready();
        s.poll_page_motion();
        s.background = true; // as when the window hides instead of closing
        assert!(s.hide_on_close());
        assert!(s.motion.page.is_none(), "no frame may follow to stop it");
    }

    #[test]
    fn page_and_now_playing_reports_reach_their_own_clip() {
        let mut s = page_ready();
        s.poll_page_motion();
        let g = s.motion.page.as_ref().unwrap().generation;
        assert!(s.motion_event(g, Event::Failed("decode".into())));
        assert!(s.motion.page.is_none());
        s.poll_page_motion();
        assert!(s.motion.page.is_none(), "not retried this session");
        assert!(!s.motion_event(g, Event::FirstFrame), "gone");
    }

    #[test]
    fn opening_another_album_page_swaps_the_clip() {
        let mut s = page_ready();
        s.poll_page_motion();
        s.collections.insert("1440783617".into(), Load::Ready(collection("1440783617", Vec::new())));
        s.route = Route::Collection { kind: CollectionKind::Album, id: "1440783617".into() };
        assert!(s.poll_page_motion(), "the old clip stops");
        assert!(s.motion.page.is_none());
        assert_eq!(albums_asked(&s), ["1440783617"]);
        assert!(s.motion.page_layer_for(Some("594061854")).is_none());
    }

    #[test]
    fn a_library_album_page_maps_to_its_catalog_album_by_exact_ids() {
        // A track already carries its catalog album: no lookup at all.
        let mut s = on_page("l.lib");
        let mut t = song("i.1", Some("594061854"));
        t.catalog_id = Some("123".into());
        s.collections.insert("l.lib".into(), Load::Ready(collection("l.lib", vec![t])));
        s.poll_page_motion();
        assert_eq!(s.page_album(), Some("594061854".into()));
        assert_eq!(albums_asked(&s), ["594061854"]);

        // Its song in a loaded catalog album, by catalog id (a same-titled edition never wins).
        let mut s = on_page("l.lib");
        let mut t = song("i.1", None);
        t.catalog_id = Some("123".into());
        s.collections.insert("l.lib".into(), Load::Ready(collection("l.lib", vec![t])));
        s.collections.insert("456".into(), Load::Ready(collection("456", vec![song("999", Some("888")), song("123", Some("456"))])));
        s.poll_page_motion();
        assert_eq!(albums_asked(&s), ["456"]);
        assert_eq!(s.sent.borrow().len(), 1);

        // Else the catalog song of its first matched track, once, then that album.
        let mut s = on_page("l.lib");
        let mut t = song("i.1", None);
        t.catalog_id = Some("123".into());
        s.collections.insert("l.lib".into(), Load::Ready(collection("l.lib", vec![song("i.0", None), t])));
        s.poll_page_motion();
        s.poll_page_motion();
        assert_eq!(s.sent.borrow().len(), 1);
        assert!(s.sent.borrow()[0].starts_with("/v1/catalog/{{storefrontId}}/songs/123 "));
        assert!(s.apply_msg(Msg::MotionPageAlbum { id: "l.lib".into(), result: Ok(Some("456".into())) }), "page shown: a frame");
        s.poll_page_motion();
        assert_eq!(albums_asked(&s), ["456"]);
        assert!(s.page_shows("456"));

        // No matched track: the library album's catalog link.
        let mut s = on_page("l.lib");
        s.collections.insert("l.lib".into(), Load::Ready(collection("l.lib", vec![song("i.0", None)])));
        s.poll_page_motion();
        assert!(s.sent.borrow()[0].starts_with("/v1/me/library/albums/l.lib/catalog "));
    }

    #[test]
    fn library_page_lookups_wait_for_the_page_and_cache_misses() {
        let mut s = State::offline();
        s.route = Route::Collection { kind: CollectionKind::Album, id: "l.lib".into() };
        s.poll_page_motion();
        assert!(s.sent.borrow().is_empty(), "the page isn't loaded yet");
        for result in [Ok(None), Err("offline".into())] {
            let mut s = on_page("l.lib");
            s.poll_page_motion();
            s.apply_msg(Msg::MotionPageAlbum { id: "l.lib".into(), result });
            s.poll_page_motion();
            s.poll_page_motion();
            assert_eq!(s.sent.borrow().len(), 1, "asked once a session");
            assert!(s.motion.page.is_none());
        }
        // A local album page asks nothing.
        let mut s = on_page("local:album:x");
        s.poll_page_motion();
        assert!(s.sent.borrow().is_empty());
    }

    /// The CVLT regression (2026-11-14): the app restored an album page at launch and asked for
    /// its motion art before MusicKit's page was ready. The bridge answered "MusicKit not ready
    /// yet" (or, with no bridge yet, nothing), the album was then marked failed or loading for
    /// the whole session, and neither the page nor Now Playing ever animated it.
    #[test]
    fn nothing_is_asked_before_the_bridge_is_up() {
        let mut s = on_page("1712657863");
        s.session.bridge_up = false;
        s.poll_page_motion();
        s.playback.song = Some(song("1712658168", Some("1712657863")));
        s.playback.state = PlayState::Playing;
        s.poll_motion();
        assert!(s.sent.borrow().is_empty(), "asked before the bridge's ready: {:?}", s.sent.borrow());
        assert!(s.motion.albums.is_empty(), "nothing marked: it is asked once the bridge is up");
        s.session.bridge_up = true;
        s.poll_page_motion();
        s.poll_motion();
        assert_eq!(albums_asked(&s), ["1712657863"], "asked once, for both slots");
    }

    /// A failed lookup (MusicKit not ready after a reload, offline, a timeout) is asked again
    /// after `RETRY_AFTER`, not kept as "no clip" until the app restarts.
    #[test]
    fn a_failed_lookup_is_asked_again_later() {
        crate::clock::freeze();
        let mut s = on_page("1712657863");
        s.poll_page_motion();
        let not_ready = "MusicKit not ready yet (page still loading)";
        s.apply_msg(Msg::Motion { album: "1712657863".into(), result: Err(not_ready.into()) });
        s.poll_page_motion();
        assert_eq!(albums_asked(&s).len(), 1, "not hammered");
        assert!(s.motion.next_retry().is_some_and(|d| d <= RETRY_AFTER), "a timer frame is due");
        crate::clock::advance(RETRY_AFTER - std::time::Duration::from_secs(1));
        s.poll_page_motion();
        assert_eq!(albums_asked(&s).len(), 1);
        crate::clock::advance(std::time::Duration::from_secs(2));
        s.poll_page_motion();
        assert_eq!(albums_asked(&s).len(), 2, "asked again");
        s.apply_msg(Msg::Motion { album: "1712657863".into(), result: Ok(Some(art())) });
        s.poll_page_motion();
        assert_eq!(s.motion.clips[MASTER], Load::Loading, "the clip is on its way");

        // The same for a library page's catalog album and a playing song's album.
        let mut s = on_page("l.lib");
        let mut t = song("i.1", None);
        t.catalog_id = Some("123".into());
        s.collections.insert("l.lib".into(), Load::Ready(collection("l.lib", vec![t])));
        s.poll_page_motion();
        s.apply_msg(Msg::MotionPageAlbum { id: "l.lib".into(), result: Err(not_ready.into()) });
        s.poll_page_motion();
        assert_eq!(s.sent.borrow().len(), 1);
        crate::clock::advance(RETRY_AFTER);
        s.poll_page_motion();
        assert_eq!(s.sent.borrow().len(), 2, "the song lookup again");

        let mut s = playing_closed();
        s.playback.song = Some(song("i.library", None));
        s.poll_motion();
        s.apply_msg(Msg::MotionSongAlbum { id: "i.library".into(), result: Err(not_ready.into()) });
        s.poll_motion();
        assert_eq!(s.sent.borrow().len(), 1);
        crate::clock::advance(RETRY_AFTER);
        s.poll_motion();
        assert_eq!(s.sent.borrow().len(), 2);
    }

    #[test]
    fn album_of_takes_catalog_album_ids_only() {
        assert_eq!(album_of(&song("1", Some("594061854"))), Some("594061854"));
        assert_eq!(album_of(&song("i.x", Some("l.abc"))), None);
        assert_eq!(album_of(&song("file:///a.mp3", Some("123"))), None);
        assert_eq!(album_of(&song("1", None)), None);
    }

    #[test]
    fn catalog_album_requests_ask_for_motion_art_and_library_ones_do_not() {
        assert_eq!(api::album("42").1["extend"], "editorialVideo");
        assert!(api::album("l.x").1.get("extend").is_none());
        assert_eq!(api::album_motion("42").1["fields[albums]"], "editorialVideo");
        assert!(api::album_with("42", false).1.get("extend").is_none(), "the plain request, for Off");
        assert_eq!(api::album_with("42", false).1["include"], "artists", "the page itself is the same");
    }

    // ---- Settings → Appearance → Animated artwork: Always, Now Playing only, Off ----

    #[test]
    fn the_setting_stops_a_loop_only_where_it_no_longer_belongs() {
        // Now Playing's loop: kept with Now Playing only, gone with Off.
        let mut s = ready();
        s.poll_motion();
        assert!(s.motion.playback.is_some());
        s.set_animated_artwork(AnimatedArtwork::NowPlaying);
        assert!(!s.poll_motion() && s.motion.playback.is_some(), "Now Playing only: it plays on, untouched");
        s.set_animated_artwork(AnimatedArtwork::Off);
        assert!(s.motion.playback.is_none(), "Off: stopped at once, no frame needed");
        let asked = s.sent.borrow().len();
        s.poll_motion();
        assert!(s.motion.playback.is_none() && s.sent.borrow().len() == asked, "and nothing restarts or is asked");
        s.set_animated_artwork(AnimatedArtwork::NowPlaying);
        s.poll_motion();
        assert!(s.motion.playback.is_some(), "back on: started the normal way");

        // The album page's loop: gone with Now Playing only and with Off, back with Always.
        for stop_with in [AnimatedArtwork::NowPlaying, AnimatedArtwork::Off] {
            let mut s = page_ready();
            s.poll_page_motion();
            assert!(s.motion.page.is_some());
            s.set_animated_artwork(stop_with);
            assert!(s.motion.page.is_none(), "{stop_with:?}: the still cover at once");
            let asked = s.sent.borrow().len();
            assert!(!s.poll_page_motion() && s.sent.borrow().len() == asked, "{stop_with:?}: nothing restarts or is asked");
            s.set_animated_artwork(AnimatedArtwork::Always);
            s.poll_page_motion();
            assert!(s.motion.page.is_some(), "{stop_with:?}: back on with Always, started by poll");
        }

        // Always to Off with both open stops both (the page's slot is its own).
        let mut s = ready();
        s.poll_motion();
        s.motion.page = Some(Playback::start("594061854".into(), 99, &Clip { path: "/nonexistent/clip.mp4".into(), width: 768, height: 768, fps: 23.976 }, Box::new(|_, _| {}), true));
        s.set_animated_artwork(AnimatedArtwork::Off);
        assert!(s.motion.playback.is_none() && s.motion.page.is_none());
    }

    #[test]
    fn the_setting_is_saved_and_a_repeat_changes_nothing() {
        let mut s = ready();
        s.poll_motion();
        s.set_animated_artwork(AnimatedArtwork::Always);
        assert!(s.motion.playback.is_some(), "the same choice again: untouched");
        s.set_animated_artwork(AnimatedArtwork::NowPlaying);
        assert_eq!(s.settings.animated_artwork, AnimatedArtwork::NowPlaying);
    }

    #[test]
    fn off_asks_for_no_motion_art_and_downloads_no_clip() {
        // Playing, Now Playing open or closed: no lookups, no prefetch, no download.
        for open in [true, false] {
            let mut s = playing();
            s.ui.now_playing = open;
            s.settings.animated_artwork = AnimatedArtwork::Off;
            s.poll_motion();
            assert!(s.sent.borrow().is_empty() && s.motion.albums.is_empty(), "now_playing {open}");
            // A late reply (asked before the switch) starts no download.
            s.apply_msg(Msg::Motion { album: "594061854".into(), result: Ok(Some(art())) });
            assert!(s.motion.clips.is_empty(), "now_playing {open}");
        }
        // An album page, catalog or library: no motion lookup either.
        let mut s = on_page("594061854");
        s.settings.animated_artwork = AnimatedArtwork::Off;
        s.poll_page_motion();
        assert!(s.sent.borrow().is_empty() && s.motion.albums.is_empty());
    }

    #[test]
    fn an_album_page_is_requested_without_motion_art_when_off() {
        for (mode, extended) in [
            (AnimatedArtwork::Always, true),
            (AnimatedArtwork::NowPlaying, true), // Now Playing uses it: saves its own request
            (AnimatedArtwork::Off, false),
        ] {
            let mut s = State::offline();
            s.settings.animated_artwork = mode;
            s.fetch_collection(CollectionKind::Album, "594061854");
            let sent = s.sent.borrow();
            assert_eq!(sent.len(), 1, "{mode:?}");
            assert!(sent[0].starts_with("/v1/catalog/{{storefrontId}}/albums/594061854 "), "{mode:?}");
            assert_eq!(sent[0].contains("editorialVideo"), extended, "{mode:?}: {}", sent[0]);
        }
    }

    #[test]
    fn an_album_page_loaded_with_off_is_not_marked_as_having_no_loop() {
        // Off sends no extension, so its reply is a plain `Collection` and `motion.albums` stays
        // empty: switching on later asks Apple instead of believing "no motion art".
        let mut s = playing();
        s.settings.animated_artwork = AnimatedArtwork::Off;
        s.apply_msg(Msg::Collection { id: "594061854".into(), result: Ok((collection("594061854", Vec::new()), None)) });
        assert!(s.motion.albums.is_empty());
        s.set_animated_artwork(AnimatedArtwork::Always);
        s.poll_motion();
        assert_eq!(motion_asks(&s), 1, "asked now");
    }
}
