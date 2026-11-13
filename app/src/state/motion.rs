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
use crate::models::api;
use crate::motion::{self, Clip, Event, Facts, MotionArt, Playback, Want};

#[derive(Default)]
pub struct MotionState {
    /// Catalog album id → its motion art (None: the album has none). Asked once a session.
    pub albums: HashMap<String, Load<Option<MotionArt>>>,
    /// Master playlist URL → the clip on disk.
    pub clips: HashMap<String, Load<Clip>>,
    pub playback: Option<Playback>,
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
        let want = motion::want(&self.motion_facts());
        let album = self.playback.song.as_ref().and_then(album_of).map(String::from);
        let (Some(album), false) = (album, want == Want::Off) else {
            return self.motion.stop();
        };
        let mut changed = false;
        if self.motion.playback.as_ref().is_some_and(|p| p.album != album) {
            changed |= self.motion.stop(); // another album now: back to its still
        }
        let art = match self.motion.albums.get(&album) {
            Some(Load::Ready(Some(art))) => art.clone(),
            Some(_) => return changed,
            None => {
                self.ask_motion(&album);
                return changed;
            }
        };
        let clip = match self.motion.clips.get(&art.video) {
            Some(Load::Ready(clip)) => clip.clone(),
            Some(_) => return changed,
            None => {
                self.download_clip(&art.video);
                return changed;
            }
        };
        if self.motion.failed.contains_key(&album) {
            return changed;
        }
        let paused = want == Want::Paused;
        match &mut self.motion.playback {
            Some(p) => {
                p.set_paused(paused);
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
            ("now playing closed", |s| s.ui.now_playing = false),
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
