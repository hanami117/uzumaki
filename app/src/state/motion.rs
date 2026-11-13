//! Animated artwork in the state: which albums have a clip (asked once a session), which clips
//! are on disk, and the one `Playback` in the Now Playing cover. `ui::poll` calls `poll_motion`
//! before each frame; it only compares, asks and starts or stops a player. Downloads run on the
//! `motion::clips` worker, decoding on the player's own thread.
//!
//! Nothing touches the network or spawns a player in headless runs and tests (no bridge, no
//! handle), except a clip a scene puts in `clips` from a local fixture.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Load, Msg, State};
use crate::bridge::PlayState;
use crate::models::{api, Song};
use crate::motion::{self, Clip, Event, Facts, MotionArt, Playback, Want};

#[derive(Default)]
pub struct MotionState {
    /// Catalog album id → its motion art (None: the album has none). Asked once a session.
    pub albums: HashMap<String, Load<Option<MotionArt>>>,
    /// Master playlist URL → the clip on disk.
    pub clips: HashMap<String, Load<Clip>>,
    pub playback: Option<Playback>,
    /// Playing song id → verified catalog album; pending/negative results are cached too.
    song_albums: HashMap<String, Load<Option<String>>>,
    /// Albums whose clip failed to play this session (no ffmpeg, a decode error).
    failed: HashMap<String, String>,
    generation: u64,
    /// The OS's Reduce Motion, as the view last saw it.
    pub reduced_motion: bool,
    /// The window can't draw (minimised, covered), from the frames Rune skips.
    pub covered: bool,
    downloads: Arc<motion::clips::Clips>,
}

impl MotionState {
    /// Stops the clip at once (Now Playing closing, the window hiding): the player and its
    /// decoder go; the still cover shows. True when something stopped.
    pub fn stop(&mut self) -> bool {
        self.playback.take().is_some()
    }

    /// The clip layer for the cover of a song on `album`, once its first frame is in.
    pub fn layer_for(&self, album: Option<&str>) -> Option<rune::El> {
        let p = self.playback.as_ref()?;
        (Some(p.album.as_str()) == album && p.shown()).then(|| p.layer())
    }
}

/// The catalog album whose clip a song would show: catalog songs know it from their URL (and
/// library songs Apple matched, when the bridge passed the URL on). Local files and library
/// songs with no catalog link have none.
pub fn album_of(song: &crate::models::Song) -> Option<&str> {
    if song.is_local() {
        return None;
    }
    song.album_id.as_deref().filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
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
        self.motion.song_albums.insert(id, match result {
            Ok(album) => Load::Ready(album),
            Err(e) => Load::Failed(e),
        });
        self.prefetch_motion();
    }

    fn motion_facts(&self) -> Facts {
        Facts {
            enabled: self.settings.animated_artwork,
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
        let (Some(album), false) = (album, want == Want::Off) else {
            return self.motion.stop();
        };
        let mut changed = false;
        if self.motion.playback.as_ref().is_some_and(|p| p.album != album) {
            changed |= self.motion.stop(); // another album now: back to its still
        }
        let Some(clip) = self.motion_clip(&album) else { return changed };
        if self.motion.failed.contains_key(&album) {
            return changed;
        }
        let paused = want == Want::Paused;
        match &mut self.motion.playback {
            Some(p) => {
                p.set_paused(paused);
                changed |= p.catch_up();
                #[cfg(target_os = "macos")]
                if p.check_native(&clip, report(self.handle.clone())) {
                    changed = true;
                }
            }
            // A clip starts only to play; a paused one keeps its frame, but none is started.
            None if !paused => {
                self.motion.generation += 1;
                let p = Playback::start(album, self.motion.generation, &clip, report(self.handle.clone()));
                changed |= p.shown();
                self.motion.playback = Some(p);
            }
            None => {}
        }
        changed
    }

    /// Gets clips onto the disk before Now Playing opens: while a song plays, the clip of its
    /// album and of the next song's in Up Next (each album asked once a session). Only asks
    /// and downloads; a player starts only in Now Playing (`poll_motion`). Off with the
    /// setting or Reduce Motion, and for local files (`album_of`).
    fn prefetch_motion(&mut self) {
        let playing = matches!(self.playback.state, PlayState::Playing);
        if !playing || !self.settings.animated_artwork || self.motion.reduced_motion {
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
                self.ask_motion(album);
                return None;
            }
        };
        match self.motion.clips.get(&video) {
            Some(Load::Ready(clip)) => Some(clip.clone()),
            Some(_) => None,
            None => {
                self.download_clip(&video);
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
                s.motion.clips.insert(key, match result {
                    Ok(c) => Load::Ready(c),
                    Err(e) => Load::Failed(e),
                });
            });
        });
    }

    /// An album's motion art arrived (its own request, or an album page asked with it).
    pub(super) fn motion_known(&mut self, album: String, result: Result<Option<MotionArt>, String>) {
        self.motion.albums.insert(album, match result {
            Ok(m) => Load::Ready(m),
            Err(e) => Load::Failed(e),
        });
        // The download starts now, not at the next frame (none may come while Now Playing is closed).
        self.prefetch_motion();
    }

    /// A player's report (`motion::Event`) for playback `generation`. True when the view changes.
    pub fn motion_event(&mut self, generation: u64, event: Event) -> bool {
        let Some(p) = self.motion.playback.as_mut().filter(|p| p.generation == generation) else { return false };
        match event {
            Event::FirstFrame => p.first_frame(),
            Event::Failed(e) => {
                let album = p.album.clone();
                self.motion.failed.insert(album, e);
                self.motion.stop()
            }
        }
    }

    /// Settings → Appearance → Animated artwork.
    pub fn set_animated_artwork(&mut self, on: bool) {
        if self.settings.animated_artwork != on {
            self.settings.animated_artwork = on;
            self.save_settings();
            if !on {
                self.motion.stop();
            }
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
            s.poll_motion() || !covered
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
            ("setting off", |s| s.settings.animated_artwork = false),
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
        assert!(s.motion.playback.is_none(), "closing Now Playing stops it at once");
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
        s.set_animated_artwork(false);
        assert!(s.motion.playback.is_none());
        s.set_animated_artwork(true);
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
            ("setting off", |s| s.settings.animated_artwork = false),
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
    fn unmatched_library_lookup_and_negative_or_failed_results_are_cached() {
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
    }
}
