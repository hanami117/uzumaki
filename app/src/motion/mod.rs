//! Animated album artwork ("motion art"): Apple's square video loop of an album, played in the
//! full-screen Now Playing cover in place of the still.
//!
//! - `MotionArt` is what the catalog says (`extend=editorialVideo`): the master playlist URL.
//! - `hls` picks the variant (768 H.264) and its one `.mp4`; `clips` keeps those on disk and
//!   downloads them on its own worker.
//! - `want` decides, from plain facts, whether the clip plays, holds still or is gone.
//! - `Playback` shows a clip: Rune's native `Player` on macOS (AVFoundation, hardware decode),
//!   else (and as the Mac's fallback) `frames`, a spawned `ffmpeg` writing RGBA into
//!   `rune::Pixels` from a worker thread. No ffmpeg: the still cover stays.
//!
//! `state/motion.rs` drives it from `poll`; `ui/now_playing` draws the layer.

pub mod clips;
mod frames;
pub mod hls;
#[cfg(target_os = "macos")]
mod native;

use serde_json::Value;

pub use clips::Clip;

/// An album's motion art, from its `editorialVideo` attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionArt {
    /// The square clip's HLS master playlist (`…_default.m3u8`).
    pub video: String,
    /// A still of the clip's first frame (an artwork template, up to 3840 px), for a poster.
    /// Not drawn yet: the cover itself is the poster in Now Playing.
    #[allow(dead_code)]
    pub preview: Option<crate::models::Artwork>,
}

impl MotionArt {
    /// From `attributes.editorialVideo`: the square clip (`motionDetailSquare`, else
    /// `motionSquareVideo1x1`). None without one, or with anything unexpected.
    pub fn parse(editorial: &Value) -> Option<Self> {
        ["motionDetailSquare", "motionSquareVideo1x1"].iter().find_map(|k| {
            let e = &editorial[*k];
            let video = e["video"].as_str().filter(|u| u.starts_with("https://") && u.contains(".m3u8"))?;
            let p = &e["previewFrame"];
            let preview = p["url"].as_str().filter(|u| !u.is_empty()).map(|u| crate::models::Artwork {
                template: u.to_string(),
                bg: p["bgColor"].as_str().map(String::from),
            });
            Some(Self { video: video.to_string(), preview })
        })
    }

    /// From an album request's reply (`data[0].attributes.editorialVideo`).
    pub fn from_reply(v: &Value) -> Option<Self> {
        Self::parse(&v["data"][0]["attributes"]["editorialVideo"])
    }
}

/// What decides whether the clip moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Settings → Appearance → Animated artwork.
    pub enabled: bool,
    /// The OS asks for reduced motion.
    pub reduced_motion: bool,
    /// Full-screen Now Playing is open.
    pub now_playing: bool,
    /// The window is hidden or closed (to the tray).
    pub hidden: bool,
    /// The window is shown but can't draw: minimised, covered, on another Space.
    pub covered: bool,
    /// The music plays.
    pub playing: bool,
}

/// What the cover slot does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// The still cover only; no player, no decoder, no ffmpeg.
    Off,
    /// A clip already showing holds its frame (its decoder waits); none is started.
    Paused,
    Playing,
}

pub fn want(f: &Facts) -> Want {
    if !f.enabled || f.reduced_motion || !f.now_playing || f.hidden {
        Want::Off
    } else if f.covered || !f.playing {
        Want::Paused
    } else {
        Want::Playing
    }
}

/// How a clip is shown.
enum Backend {
    #[cfg(target_os = "macos")]
    Native(native::Native),
    Frames(frames::Frames),
}

/// The one clip on screen: the album it belongs to, its player, and whether its first frame is
/// there to fade in over the still.
pub struct Playback {
    /// Catalog album id the clip belongs to.
    pub album: String,
    /// Bumped per playback, so a late message from an older one is ignored.
    pub generation: u64,
    backend: Backend,
    paused: bool,
    shown: bool,
}

/// Messages from a player's worker back to the state (through `AppHandle::update`).
pub type Report = Box<dyn Fn(u64, Event) + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The first frame is in: fade the clip in.
    FirstFrame,
    /// The player gave up (no ffmpeg, a decode error): keep the still for this album.
    Failed(String),
}

/// `UZUMAKI_MOTION=ffmpeg` uses the ffmpeg path on the Mac too (to try the Linux path there).
#[cfg(target_os = "macos")]
fn force_frames() -> bool {
    std::env::var("UZUMAKI_MOTION").is_ok_and(|v| v == "ffmpeg")
}

impl Playback {
    /// Starts showing `clip`, playing. `report` hears the first frame and failures.
    pub fn start(album: String, generation: u64, clip: &Clip, report: Report) -> Self {
        #[cfg(target_os = "macos")]
        if !force_frames() {
            return Self { album, generation, backend: Backend::Native(native::Native::start(clip)), paused: false, shown: true };
        }
        let backend = Backend::Frames(frames::Frames::start(clip, generation, report));
        Self { album, generation, backend, paused: false, shown: false }
    }

    /// The Mac's player failed: the same clip through ffmpeg.
    #[cfg(target_os = "macos")]
    fn fall_back(&mut self, clip: &Clip, report: Report) {
        self.backend = Backend::Frames(frames::Frames::start(clip, self.generation, report));
        self.shown = false;
        self.paused = false;
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            match &self.backend {
                #[cfg(target_os = "macos")]
                Backend::Native(n) => n.set_paused(paused),
                Backend::Frames(f) => f.set_paused(paused),
            }
        }
    }

    #[cfg(test)]
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The first frame is ready (native: from the start, its poster is decoded up front).
    pub fn shown(&self) -> bool {
        self.shown
    }

    pub fn first_frame(&mut self) -> bool {
        !std::mem::replace(&mut self.shown, true)
    }

    /// The first frame is in but not yet marked (its report may not have landed, or can't:
    /// headless runs have no handle). True when it changed.
    pub fn catch_up(&mut self) -> bool {
        let ready = match &self.backend {
            #[cfg(target_os = "macos")]
            Backend::Native(_) => true,
            Backend::Frames(f) => f.has_frame(),
        };
        ready && self.first_frame()
    }

    /// Native only: the platform player failed; switch to ffmpeg. True when it switched.
    #[cfg(target_os = "macos")]
    pub fn check_native(&mut self, clip: &Clip, report: Report) -> bool {
        match &self.backend {
            Backend::Native(n) if n.failed() => {
                eprintln!("uzumaki: animated artwork: native player failed, trying ffmpeg");
                self.fall_back(clip, report);
                true
            }
            _ => false,
        }
    }

    /// The element that draws the clip, filling its parent.
    pub fn layer(&self) -> rune::El {
        let e = rune::el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
        match &self.backend {
            #[cfg(target_os = "macos")]
            Backend::Native(n) => e.video(n.player(), rune::SurfaceFit::Fill),
            Backend::Frames(f) => e.pixels_fit(f.pixels(), rune::SurfaceFit::Fill),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn square(video: &str) -> Value {
        json!({
            "previewFrame": {"url": "https://is1.example/x/{w}x{h}bb.jpg", "width": 3840, "height": 3840, "bgColor": "fffef2"},
            "video": video
        })
    }

    #[test]
    fn parses_the_square_clip_of_an_album_reply() {
        // Shaped like Apple's reply for Rumours with `extend=editorialVideo` (2026-11-13).
        let reply = json!({"data": [{"id": "594061854", "type": "albums", "attributes": {
            "name": "Rumours",
            "editorialVideo": {
                "motionDetailSquare": square("https://mvod.example/a/P1_default.m3u8"),
                "motionSquareVideo1x1": square("https://mvod.example/a/P1_default.m3u8"),
                "motionDetailTall": square("https://mvod.example/t/P2_default.m3u8"),
                "motionTallVideo3x4": square("https://mvod.example/t/P3_default.m3u8"),
            }
        }}]});
        let m = MotionArt::from_reply(&reply).unwrap();
        assert_eq!(m.video, "https://mvod.example/a/P1_default.m3u8");
        let p = m.preview.unwrap();
        assert_eq!(p.url(600), "https://is1.example/x/600x600bb.jpg");
        assert_eq!(p.bg.as_deref(), Some("fffef2"));
    }

    #[test]
    fn falls_back_to_the_other_square_key_and_never_to_tall() {
        let e = json!({"motionSquareVideo1x1": square("https://mvod.example/s.m3u8")});
        assert_eq!(MotionArt::parse(&e).unwrap().video, "https://mvod.example/s.m3u8");
        let tall_only = json!({"motionDetailTall": square("https://mvod.example/t.m3u8")});
        assert!(MotionArt::parse(&tall_only).is_none());
    }

    #[test]
    fn no_editorial_video_or_an_odd_one_is_none() {
        assert!(MotionArt::from_reply(&json!({"data": [{"attributes": {"name": "Kind of Blue"}}]})).is_none());
        assert!(MotionArt::from_reply(&json!({})).is_none());
        assert!(MotionArt::parse(&json!({"motionDetailSquare": {"video": 3}})).is_none());
        assert!(MotionArt::parse(&json!({"motionDetailSquare": {"video": "http://plain.example/a.m3u8"}})).is_none());
        // No preview is fine: the cover is the poster.
        let m = MotionArt::parse(&json!({"motionDetailSquare": {"video": "https://v.example/a.m3u8"}})).unwrap();
        assert!(m.preview.is_none());
    }

    #[test]
    fn plays_only_when_everything_allows_it() {
        let on = Facts { enabled: true, now_playing: true, playing: true, ..Facts::default() };
        assert_eq!(want(&on), Want::Playing);
        assert_eq!(want(&Facts { enabled: false, ..on }), Want::Off, "the setting");
        assert_eq!(want(&Facts { reduced_motion: true, ..on }), Want::Off, "Reduce Motion: the still");
        assert_eq!(want(&Facts { now_playing: false, ..on }), Want::Off, "Now Playing closed");
        assert_eq!(want(&Facts { hidden: true, ..on }), Want::Off, "hidden to the tray: stopped");
        assert_eq!(want(&Facts { covered: true, ..on }), Want::Paused, "minimised or covered: holds");
        assert_eq!(want(&Facts { playing: false, ..on }), Want::Paused, "music paused: holds");
        assert_eq!(want(&Facts { hidden: true, covered: true, playing: false, ..on }), Want::Off);
    }
}
