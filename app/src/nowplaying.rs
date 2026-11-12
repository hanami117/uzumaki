//! macOS media keys and Control Center: the counterpart of `mpris.rs`.
//!
//! `MPRemoteCommandCenter` receives the keyboard media keys, AirPods taps and the Control Center
//! / lock screen buttons; `MPNowPlayingInfoCenter` tells the system what is playing. Same
//! surface as `Mpris`: `start(on_command)` and `observe(&Event)`.

use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::{AnyThread, Message};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::NSImage;
use objc2_foundation::{
    NSActivityOptions, NSData, NSDictionary, NSNumber, NSObjectProtocol, NSProcessInfo, NSSize, NSString,
};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle,
    MPMediaItemPropertyArtist, MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration,
    MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingInfoPropertyElapsedPlaybackTime,
    MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter,
    MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
};

use crate::bridge::{Event, PlayState};
use crate::models::Song;
use crate::mpris::Command;

/// A Time event this far from where playback should be means the user (or the app) seeked.
const SEEK_JUMP: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Playing,
    Paused,
    Stopped,
}

fn status_of(state: PlayState) -> Status {
    if state.is_active() {
        Status::Playing
    } else if state == PlayState::Paused {
        Status::Paused
    } else {
        Status::Stopped
    }
}

/// What Control Center should show right now. Plain data, so the mapping is testable.
#[derive(Clone, Debug, PartialEq)]
struct Info {
    song_id: String,
    title: String,
    artist: String,
    album: String,
    /// Seconds.
    duration: f64,
    /// Seconds at the moment of this snapshot; the system extrapolates from `rate`.
    elapsed: f64,
    status: Status,
}

impl Info {
    /// 1.0 while playing, 0.0 otherwise.
    fn rate(&self) -> f64 {
        if self.status == Status::Playing { 1.0 } else { 0.0 }
    }
}

/// Follows the bridge events and decides when the system needs to hear something.
#[derive(Default)]
struct Model {
    song: Option<Song>,
    duration: f64,
    status: Option<Status>,
    /// Seconds, from the last Time event, and when it arrived.
    position: f64,
    position_at: Option<Instant>,
}

impl Model {
    fn status(&self) -> Status {
        self.status.unwrap_or(Status::Stopped)
    }

    /// Where playback should be now, extrapolating from the last Time event while playing.
    fn expected_position(&self, now: Instant) -> f64 {
        match (self.status(), self.position_at) {
            (Status::Playing, Some(at)) => self.position + now.saturating_duration_since(at).as_secs_f64(),
            _ => self.position,
        }
    }

    fn info(&self, now: Instant) -> Option<Info> {
        let song = self.song.as_ref()?;
        Some(Info {
            song_id: song.id.clone(),
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: song.album.clone(),
            duration: self.duration,
            elapsed: self.expected_position(now),
            status: self.status(),
        })
    }

    /// Returns a new snapshot only when the system has to be told: song change, play/pause,
    /// seek. Plain time ticks are not sent (the system extrapolates from the rate).
    fn apply(&mut self, event: &Event, now: Instant) -> Option<Info> {
        match event {
            // None = MusicKit swapping queues; keep the last track instead of flickering.
            Event::NowPlaying(Some(song)) => {
                self.duration = song.duration;
                self.song = Some(song.clone());
                self.position = 0.0;
                self.position_at = Some(now);
                self.info(now)
            }
            Event::PlayState(state) => {
                let status = status_of(*state);
                if self.status == Some(status) {
                    return None;
                }
                // Fold elapsed time in so the extrapolation stays right across the change.
                self.position = self.expected_position(now);
                self.position_at = Some(now);
                self.status = Some(status);
                self.info(now)
            }
            Event::Time { current, duration } => {
                let jumped = self.position_at.is_some() && (current - self.expected_position(now)).abs() > SEEK_JUMP;
                self.position = *current;
                self.position_at = Some(now);
                // Songs without a known length (some local files) learn it from the player.
                let learned = self.duration <= 0.0 && *duration > 0.0;
                if learned {
                    self.duration = *duration;
                }
                if jumped || learned { self.info(now) } else { None }
            }
            // The bridge is gone (or was stopped for local only mode): nothing is playing any more.
            Event::Exited | Event::Stopped => {
                self.position = self.expected_position(now);
                self.position_at = Some(now);
                self.status = Some(Status::Stopped);
                self.info(now)
            }
            _ => None,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Objects that are only touched from one thread at a time (or never again after `start`).
struct Unshared<T>(T);
// SAFETY: see the uses: the command targets are never touched after `start`; the activity is
// behind a mutex and NSProcessInfo activities may be begun and ended from any thread.
unsafe impl<T> Send for Unshared<T> {}
unsafe impl<T> Sync for Unshared<T> {}

type Activity = Retained<ProtocolObject<dyn NSObjectProtocol>>;

pub struct NowPlaying {
    model: Arc<Mutex<Model>>,
    /// Held while playing so macOS doesn't nap the app (window closed = no visible windows).
    activity: Mutex<Unshared<Option<Activity>>>,
    /// The command center keeps the handlers; the returned targets are kept for the app's lifetime.
    _targets: Unshared<Vec<Retained<AnyObject>>>,
}

/// State that only the main thread touches: the cover for the song on screen.
struct MainState {
    artwork: Option<(String, Retained<MPMediaItemArtwork>)>,
    info: Option<Info>,
}

thread_local! {
    static MAIN: std::cell::RefCell<MainState> = const { std::cell::RefCell::new(MainState { artwork: None, info: None }) };
}

type EventToCommand = Box<dyn Fn(NonNull<MPRemoteCommandEvent>) -> Command + Send + Sync>;

impl NowPlaying {
    /// Registers the remote commands. Call on the main thread. `on_command` may run on any thread.
    pub fn start(on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<Self, String> {
        let on_command = Arc::new(on_command);
        let model = Arc::new(Mutex::new(Model::default()));
        let mut targets = Vec::new();
        // SAFETY: plain Objective-C calls on the shared command center; the blocks only touch
        // Send + Sync data and return a valid status.
        unsafe {
            let center = MPRemoteCommandCenter::sharedCommandCenter();
            let mut on = |cmd: &MPRemoteCommand, f: EventToCommand| {
                let on_command = on_command.clone();
                let block = RcBlock::new(move |ev: NonNull<MPRemoteCommandEvent>| {
                    on_command(f(ev));
                    MPRemoteCommandHandlerStatus::Success
                });
                cmd.setEnabled(true);
                targets.push(cmd.addTargetWithHandler(&block));
            };
            on(&center.playCommand(), Box::new(|_| Command::Play));
            on(&center.pauseCommand(), Box::new(|_| Command::Pause));
            let m = model.clone();
            on(
                &center.togglePlayPauseCommand(),
                Box::new(move |_| if lock(&m).status() == Status::Playing { Command::Pause } else { Command::Play }),
            );
            on(&center.nextTrackCommand(), Box::new(|_| Command::Next));
            on(&center.previousTrackCommand(), Box::new(|_| Command::Previous));
            // This command only ever delivers MPChangePlaybackPositionCommandEvent.
            on(
                &center.changePlaybackPositionCommand(),
                Box::new(|ev| Command::SeekTo(ev.cast::<MPChangePlaybackPositionCommandEvent>().as_ref().positionTime())),
            );

            // Not supported: keep them off so Control Center doesn't show dead buttons.
            for cmd in [
                center.stopCommand(),
                Retained::into_super(center.skipForwardCommand()),
                Retained::into_super(center.skipBackwardCommand()),
                center.seekForwardCommand(),
                center.seekBackwardCommand(),
                Retained::into_super(center.ratingCommand()),
                Retained::into_super(center.likeCommand()),
                Retained::into_super(center.dislikeCommand()),
                Retained::into_super(center.bookmarkCommand()),
                Retained::into_super(center.changeRepeatModeCommand()),
                Retained::into_super(center.changeShuffleModeCommand()),
                Retained::into_super(center.changePlaybackRateCommand()),
                center.enableLanguageOptionCommand(),
                center.disableLanguageOptionCommand(),
            ] {
                cmd.setEnabled(false);
            }
        }
        Ok(Self { model, activity: Mutex::new(Unshared(None)), _targets: Unshared(targets) })
    }

    /// Called on the bridge reader thread for every event. Must not block.
    pub fn observe(&self, event: &Event) {
        let info = lock(&self.model).apply(event, Instant::now());
        let Some(info) = info else { return };
        self.hold_activity(info.status == Status::Playing);
        if let Event::NowPlaying(Some(song)) = event {
            if let Some(art) = &song.artwork {
                self.fetch_artwork(info.song_id.clone(), art.clone());
            }
        }
        // MPNowPlayingInfoCenter may be used from any thread, but updates stay ordered and the
        // system's notifications are happiest on the main queue, so hop there.
        DispatchQueue::main().exec_async(move || publish(info));
    }

    /// While playing, tell macOS the app is doing user-visible work (no App Nap), but let the
    /// Mac sleep when idle.
    fn hold_activity(&self, playing: bool) {
        let mut slot = lock(&self.activity);
        match (playing, slot.0.is_some()) {
            (true, false) => {
                let reason = NSString::from_str("Playing music");
                let a = NSProcessInfo::processInfo()
                    .beginActivityWithOptions_reason(NSActivityOptions::UserInitiatedAllowingIdleSystemSleep, &reason);
                slot.0 = Some(a);
            }
            (false, true) => {
                if let Some(a) = slot.0.take() {
                    // SAFETY: `a` came from beginActivityWithOptions and is ended once.
                    unsafe { NSProcessInfo::processInfo().endActivity(&a) };
                }
            }
            _ => {}
        }
    }

    /// Loads the cover (disk cache first, else downloaded once and cached; the same bytes the Now
    /// Playing view uses) on a worker thread, then hands it to the main thread.
    fn fetch_artwork(&self, song_id: String, art: crate::models::Artwork) {
        let model = self.model.clone();
        std::thread::spawn(move || {
            let Ok(bytes) = crate::art::bytes_blocking(&art.url(crate::art::NOW_PLAYING_PX)) else { return };
            // The song may have changed while this was loading.
            if lock(&model).song.as_ref().map(|s| s.id.as_str()) != Some(song_id.as_str()) {
                return;
            }
            DispatchQueue::main().exec_async(move || set_artwork(song_id, bytes));
        });
    }
}

/// Main thread: writes the info dictionary and the playback state.
fn publish(info: Info) {
    MAIN.with(|m| {
        let mut m = m.borrow_mut();
        // A new song drops the old cover until its own arrives.
        if m.artwork.as_ref().is_some_and(|(id, _)| *id != info.song_id) {
            m.artwork = None;
        }
        m.info = Some(info);
        apply_main(&m);
    });
}

/// Main thread: the cover arrived (ignored if the song already changed).
fn set_artwork(song_id: String, bytes: Vec<u8>) {
    MAIN.with(|m| {
        let mut m = m.borrow_mut();
        if m.info.as_ref().map(|i| i.song_id.as_str()) != Some(song_id.as_str()) {
            return;
        }
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&bytes)) else { return };
        let size: NSSize = image.size();
        let handler = RcBlock::new(move |_: NSSize| NonNull::from(&*image));
        // SAFETY: the block returns a pointer to an image it keeps alive.
        let art = unsafe { MPMediaItemArtwork::initWithBoundsSize_requestHandler(MPMediaItemArtwork::alloc(), size, &handler) };
        m.artwork = Some((song_id, art));
        apply_main(&m);
    });
}

fn apply_main(m: &MainState) {
    let Some(info) = &m.info else { return };
    // Dictionary values are plain objects.
    fn any<T: Message>(r: Retained<T>) -> Retained<AnyObject> {
        // SAFETY: every Objective-C object is an AnyObject.
        unsafe { Retained::cast_unchecked(r) }
    }
    let num = |x: f64| any(NSNumber::new_f64(x));
    let text = |s: &str| any(NSString::from_str(s));
    // SAFETY: the keys are the framework's own constants and each value has the documented type.
    unsafe {
        let mut items: Vec<(&NSString, Retained<AnyObject>)> = vec![
            (MPMediaItemPropertyTitle, text(&info.title)),
            (MPMediaItemPropertyArtist, text(&info.artist)),
            (MPMediaItemPropertyAlbumTitle, text(&info.album)),
            (MPMediaItemPropertyPlaybackDuration, num(info.duration)),
            (MPNowPlayingInfoPropertyElapsedPlaybackTime, num(info.elapsed)),
            (MPNowPlayingInfoPropertyPlaybackRate, num(info.rate())),
        ];
        if let Some((_, art)) = &m.artwork {
            items.push((MPMediaItemPropertyArtwork, any(art.clone())));
        }
        let keys: Vec<&NSString> = items.iter().map(|(k, _)| *k).collect();
        let vals: Vec<&AnyObject> = items.iter().map(|(_, v)| &**v).collect();
        let dict: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(&keys, &vals);
        let center = MPNowPlayingInfoCenter::defaultCenter();
        center.setNowPlayingInfo(Some(&dict));
        // On macOS this decides which app the media keys go to.
        center.setPlaybackState(match info.status {
            Status::Playing => MPNowPlayingPlaybackState::Playing,
            Status::Paused => MPNowPlayingPlaybackState::Paused,
            Status::Stopped => MPNowPlayingPlaybackState::Stopped,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn song(id: &str, duration: f64) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: "Title".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: Vec::new(),
        }
    }

    #[test]
    fn statuses() {
        assert_eq!(status_of(PlayState::Playing), Status::Playing);
        assert_eq!(status_of(PlayState::Loading), Status::Playing);
        assert_eq!(status_of(PlayState::Paused), Status::Paused);
        for s in [PlayState::None, PlayState::Stopped, PlayState::Ended, PlayState::Completed] {
            assert_eq!(status_of(s), Status::Stopped);
        }
    }

    #[test]
    fn song_change_sends_metadata_from_zero() {
        let t0 = Instant::now();
        let mut m = Model::default();
        assert!(m.apply(&Event::NowPlaying(None), t0).is_none());
        let i = m.apply(&Event::NowPlaying(Some(song("a", 200.0))), t0).unwrap();
        assert_eq!((i.title.as_str(), i.artist.as_str(), i.album.as_str()), ("Title", "Artist", "Album"));
        assert_eq!((i.duration, i.elapsed, i.rate()), (200.0, 0.0, 0.0));
        // A queue swap keeps the last song.
        assert!(m.apply(&Event::NowPlaying(None), t0).is_none());
        assert_eq!(m.song.as_ref().unwrap().id, "a");
    }

    #[test]
    fn play_pause_folds_elapsed_time_and_sets_rate() {
        let t0 = Instant::now();
        let mut m = Model::default();
        m.apply(&Event::NowPlaying(Some(song("a", 200.0))), t0);
        let p = m.apply(&Event::PlayState(PlayState::Playing), t0).unwrap();
        assert_eq!((p.status, p.rate()), (Status::Playing, 1.0));
        // Same state again: nothing to tell.
        assert!(m.apply(&Event::PlayState(PlayState::Loading), t0).is_none());
        m.apply(&Event::Time { current: 10.0, duration: 200.0 }, t0);
        let paused = m.apply(&Event::PlayState(PlayState::Paused), t0 + Duration::from_secs(5)).unwrap();
        assert_eq!((paused.status, paused.rate()), (Status::Paused, 0.0));
        assert!((paused.elapsed - 15.0).abs() < 1e-6, "{}", paused.elapsed);
        // Paused: the clock stands still.
        assert_eq!(m.expected_position(t0 + Duration::from_secs(60)), paused.elapsed);
    }

    #[test]
    fn time_ticks_are_quiet_but_seeks_are_sent() {
        let t0 = Instant::now();
        let mut m = Model::default();
        m.apply(&Event::NowPlaying(Some(song("a", 200.0))), t0);
        m.apply(&Event::PlayState(PlayState::Playing), t0);
        for s in 1..=10u64 {
            let tick = Event::Time { current: s as f64, duration: 200.0 };
            assert!(m.apply(&tick, t0 + Duration::from_secs(s)).is_none(), "tick {s}");
        }
        let at = t0 + Duration::from_secs(11);
        let i = m.apply(&Event::Time { current: 90.0, duration: 200.0 }, at).unwrap();
        assert_eq!(i.elapsed, 90.0);
    }

    #[test]
    fn unknown_duration_is_learned_once() {
        let t0 = Instant::now();
        let mut m = Model::default();
        m.apply(&Event::NowPlaying(Some(song("a", 0.0))), t0);
        let i = m.apply(&Event::Time { current: 0.5, duration: 180.0 }, t0).unwrap();
        assert_eq!(i.duration, 180.0);
        assert!(m.apply(&Event::Time { current: 1.5, duration: 180.0 }, t0 + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn exit_stops_and_other_events_are_ignored() {
        let t0 = Instant::now();
        let mut m = Model::default();
        assert!(m.apply(&Event::Exited, t0).is_none(), "no song yet");
        m.apply(&Event::NowPlaying(Some(song("a", 200.0))), t0);
        m.apply(&Event::PlayState(PlayState::Playing), t0);
        assert!(m.apply(&Event::Volume(0.5), t0).is_none());
        assert_eq!(m.apply(&Event::Exited, t0).unwrap().status, Status::Stopped);
    }
}
