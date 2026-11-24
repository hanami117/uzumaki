//! Windows media keys and the media overlay: the counterpart of `mpris.rs` and `nowplaying.rs`.
//!
//! The System Media Transport Controls (SMTC) receive the keyboard's media keys, headset buttons
//! and the buttons of the volume overlay, the lock screen and Quick Settings, and tell Windows
//! what is playing (title, artist, album, cover, position). Same surface as `Mpris`:
//! `start(on_command)` and `observe(&Event)`.
//!
//! SMTC belongs to a window. Uzumaki's own may not exist (started hidden, closed to the tray), so
//! this module keeps a window of its own that is never shown, on its own thread
//! (`uzumaki-smtc`), which also does every SMTC call. `observe` only updates the model and posts
//! what changed to that thread: it never blocks the UI thread.
//!
//! What Windows shows as the player's name and icon comes from the app id (`win::set_app_id`,
//! `win::register_app_id`). Electron's own media session is off (`bridge/main.js`), so there
//! should be one player in the overlay, not two.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs, SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs, SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WS_OVERLAPPED};
use windows::core::{HSTRING, w};

use crate::bridge::{Event, PlayState};
use crate::media::Command;
use crate::models::{Artwork, Song};

/// A Time event this far from where playback should be means the user (or the app) seeked.
const SEEK_JUMP: f64 = 2.0;
/// While playing, Windows is told the position this often (it does not count by itself).
const TIMELINE_EVERY: Duration = Duration::from_secs(5);

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

/// What Windows should show right now. Plain data, so the mapping is testable.
#[derive(Clone, Debug, PartialEq)]
struct Info {
    song_id: String,
    title: String,
    artist: String,
    album: String,
    /// Seconds.
    duration: f64,
    /// Seconds at the moment of this snapshot.
    elapsed: f64,
    status: Status,
}

/// What to tell the SMTC thread.
#[derive(Debug, PartialEq)]
enum Change {
    /// Song, state or a seek: everything is written again.
    All(Info),
    /// Only the position moved on (the periodic tick while playing).
    Position { elapsed: f64, duration: f64 },
}

/// Follows the playback events (`media::Feed`) and decides when Windows needs to hear something.
#[derive(Default)]
struct Model {
    song: Option<Song>,
    duration: f64,
    status: Option<Status>,
    /// Seconds, from the last Time event, and when it arrived.
    position: f64,
    position_at: Option<Instant>,
    /// When the position was last sent.
    told_at: Option<Instant>,
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

    fn info(&mut self, now: Instant) -> Option<Change> {
        let song = self.song.as_ref()?;
        let info = Info {
            song_id: song.id.clone(),
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: song.album.clone(),
            duration: self.duration,
            elapsed: self.expected_position(now),
            status: self.status(),
        };
        self.told_at = Some(now);
        Some(Change::All(info))
    }

    /// What Windows has to be told after `event`, if anything: a song change, play/pause and a
    /// seek at once; the plain passing of time every `TIMELINE_EVERY`.
    fn apply(&mut self, event: &Event, now: Instant) -> Option<Change> {
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
                if jumped || learned {
                    return self.info(now);
                }
                let due = self.told_at.is_none_or(|t| now.saturating_duration_since(t) >= TIMELINE_EVERY);
                if due && self.song.is_some() {
                    self.told_at = Some(now);
                    return Some(Change::Position { elapsed: *current, duration: self.duration });
                }
                None
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

enum Msg {
    Change(Change),
    /// The cover's bytes (JPEG or PNG) for the song with this id.
    Cover(String, Vec<u8>),
}

pub struct Smtc {
    model: Arc<Mutex<Model>>,
    tx: Mutex<Sender<Msg>>,
    /// The SMTC thread's id, to wake its message loop.
    thread: u32,
}

/// Seconds as a WinRT time span (100 ns units).
fn span(seconds: f64) -> TimeSpan {
    TimeSpan { Duration: (seconds.max(0.0) * 10_000_000.0) as i64 }
}

/// Wakes the SMTC thread's message loop.
fn wake(thread: u32) {
    // SAFETY: a plain message to our own thread's queue; a failure (the thread is gone) only
    // means nobody listens any more.
    unsafe {
        let _ = PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0));
    }
}

impl Smtc {
    /// Creates the hidden window and the controls on a thread of their own. `on_command` runs on
    /// a Windows thread-pool thread.
    pub fn start(on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        let on_command: Arc<dyn Fn(Command) + Send + Sync> = Arc::new(on_command);
        std::thread::Builder::new()
            .name("uzumaki-smtc".into())
            .spawn(move || run(rx, ready_tx, on_command))
            .map_err(|e| e.to_string())?;
        let thread = ready_rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the media controls didn't start".to_string())??;
        Ok(Self { model: Arc::default(), tx: Mutex::new(tx), thread })
    }

    /// Called on the UI thread with what the app state shows (`media::Feed`). Must not block.
    pub fn observe(&self, event: &Event) {
        let change = lock(&self.model).apply(event, Instant::now());
        let Some(change) = change else { return };
        if let (Event::NowPlaying(Some(song)), Change::All(info)) = (event, &change)
            && let Some(art) = &song.artwork
        {
            self.fetch_cover(info.song_id.clone(), art.clone());
        }
        if lock(&self.tx).send(Msg::Change(change)).is_ok() {
            wake(self.thread);
        }
    }

    /// Loads the cover (disk cache first, else downloaded once and cached; the same bytes the Now
    /// Playing view uses) on a worker thread, then hands it to the SMTC thread.
    fn fetch_cover(&self, song_id: String, art: Artwork) {
        let model = self.model.clone();
        let tx = lock(&self.tx).clone();
        let thread = self.thread;
        std::thread::spawn(move || {
            let Ok(bytes) = crate::art::bytes_blocking(&art.url(crate::art::NOW_PLAYING_PX)) else { return };
            // The song may have changed while this was loading.
            if lock(&model).song.as_ref().map(|s| s.id.as_str()) != Some(song_id.as_str()) {
                return;
            }
            if tx.send(Msg::Cover(song_id, bytes)).is_ok() {
                wake(thread);
            }
        });
    }
}

/// The controls of a hidden window of ours.
fn controls(on_command: Arc<dyn Fn(Command) + Send + Sync>) -> windows::core::Result<(HWND, SystemMediaTransportControls)> {
    // SAFETY: a plain top-level window of a system class, never shown; destroyed by `run`.
    let hwnd = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Uzumaki"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, None, None)? };
    let interop = windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
    // SAFETY: `hwnd` is the window made above.
    let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd)? };
    smtc.SetIsEnabled(true)?;
    smtc.SetIsPlayEnabled(true)?;
    smtc.SetIsPauseEnabled(true)?;
    smtc.SetIsNextEnabled(true)?;
    smtc.SetIsPreviousEnabled(true)?;
    smtc.SetIsStopEnabled(false)?;
    smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;
    let pressed = on_command.clone();
    smtc.ButtonPressed(&TypedEventHandler::<SystemMediaTransportControls, SystemMediaTransportControlsButtonPressedEventArgs>::new(move |_, args| {
        let Some(button) = args.as_ref().and_then(|a| a.Button().ok()) else { return Ok(()) };
        if let Some(cmd) = command_of(button) {
            pressed(cmd);
        }
        Ok(())
    }))?;
    let seek = on_command;
    smtc.PlaybackPositionChangeRequested(&TypedEventHandler::<SystemMediaTransportControls, PlaybackPositionChangeRequestedEventArgs>::new(move |_, args| {
        if let Some(to) = args.as_ref().and_then(|a| a.RequestedPlaybackPosition().ok()) {
            seek(Command::SeekTo(to.Duration as f64 / 10_000_000.0));
        }
        Ok(())
    }))?;
    Ok((hwnd, smtc))
}

/// A button of the overlay (or a media key) as a command.
fn command_of(button: SystemMediaTransportControlsButton) -> Option<Command> {
    if button == SystemMediaTransportControlsButton::Play {
        Some(Command::Play)
    } else if button == SystemMediaTransportControlsButton::Pause || button == SystemMediaTransportControlsButton::Stop {
        Some(Command::Pause)
    } else if button == SystemMediaTransportControlsButton::Next {
        Some(Command::Next)
    } else if button == SystemMediaTransportControlsButton::Previous {
        Some(Command::Previous)
    } else {
        None
    }
}

fn timeline(smtc: &SystemMediaTransportControls, elapsed: f64, duration: f64) -> windows::core::Result<()> {
    let props = SystemMediaTransportControlsTimelineProperties::new()?;
    let end = duration.max(0.0);
    props.SetStartTime(span(0.0))?;
    props.SetMinSeekTime(span(0.0))?;
    props.SetEndTime(span(end))?;
    props.SetMaxSeekTime(span(end))?;
    props.SetPosition(span(if end > 0.0 { elapsed.min(end) } else { elapsed }))?;
    smtc.UpdateTimelineProperties(&props)
}

fn publish(smtc: &SystemMediaTransportControls, info: &Info, shown: &mut Option<String>) -> windows::core::Result<()> {
    let updater = smtc.DisplayUpdater()?;
    if shown.as_deref() != Some(info.song_id.as_str()) {
        // A new song: its texts now, its cover when it has loaded (the old one must not stay).
        updater.ClearAll()?;
        updater.SetType(MediaPlaybackType::Music)?;
        let music = updater.MusicProperties()?;
        music.SetTitle(&HSTRING::from(info.title.as_str()))?;
        music.SetArtist(&HSTRING::from(info.artist.as_str()))?;
        music.SetAlbumTitle(&HSTRING::from(info.album.as_str()))?;
        updater.Update()?;
        *shown = Some(info.song_id.clone());
    }
    smtc.SetPlaybackStatus(match info.status {
        Status::Playing => MediaPlaybackStatus::Playing,
        Status::Paused => MediaPlaybackStatus::Paused,
        Status::Stopped => MediaPlaybackStatus::Stopped,
    })?;
    timeline(smtc, info.elapsed, info.duration)
}

fn set_cover(smtc: &SystemMediaTransportControls, bytes: &[u8]) -> windows::core::Result<()> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    let updater = smtc.DisplayUpdater()?;
    updater.SetThumbnail(&RandomAccessStreamReference::CreateFromStream(&stream)?)?;
    updater.Update()
}

/// The SMTC thread: makes the window and the controls, says how that went, then applies what
/// `observe` posts until the app ends.
fn run(rx: Receiver<Msg>, ready: Sender<Result<u32, String>>, on_command: Arc<dyn Fn(Command) + Send + Sync>) {
    let (hwnd, smtc) = match controls(on_command) {
        Ok(made) => made,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    // SAFETY: no preconditions.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let mut shown: Option<String> = None;
    let mut msg = MSG::default();
    // SAFETY: a plain message loop on the thread that owns the window.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message != WM_APP {
            // SAFETY: `msg` was filled by GetMessageW.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            continue;
        }
        for m in rx.try_iter() {
            let done = match &m {
                Msg::Change(Change::All(info)) => publish(&smtc, info, &mut shown),
                Msg::Change(Change::Position { elapsed, duration }) => timeline(&smtc, *elapsed, *duration),
                Msg::Cover(song_id, bytes) if shown.as_deref() == Some(song_id.as_str()) => set_cover(&smtc, bytes),
                Msg::Cover(..) => Ok(()),
            };
            if let Err(e) = done {
                eprintln!("uzumaki: media controls: {e}");
            }
        }
    }
    // SAFETY: the window made in `controls`, on its own thread.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, duration: f64) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: format!("Title {id}"),
            artist: "Artist".into(),
            album: "Album".into(),
            album_is_fallback: false,
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
    fn a_song_a_pause_and_a_seek_are_told_at_once() {
        let mut m = Model::default();
        let t0 = Instant::now();
        let Some(Change::All(info)) = m.apply(&Event::NowPlaying(Some(song("a", 200.0))), t0) else { panic!("a new song is news") };
        assert_eq!((info.title.as_str(), info.duration, info.status), ("Title a", 200.0, Status::Stopped));
        let Some(Change::All(info)) = m.apply(&Event::PlayState(PlayState::Playing), t0) else { panic!("playing is news") };
        assert_eq!(info.status, Status::Playing);
        assert_eq!(m.apply(&Event::PlayState(PlayState::Playing), t0), None, "the same state again is not");
        // One second later the player says 1 s: as expected, and the last report is fresh.
        let t1 = t0 + Duration::from_secs(1);
        assert_eq!(m.apply(&Event::Time { current: 1.0, duration: 200.0 }, t1), None);
        // A jump is a seek.
        let Some(Change::All(info)) = m.apply(&Event::Time { current: 90.0, duration: 200.0 }, t1 + Duration::from_secs(1)) else { panic!("a seek is news") };
        assert!((info.elapsed - 90.0).abs() < 0.01);
        let Some(Change::All(info)) = m.apply(&Event::PlayState(PlayState::Paused), t1 + Duration::from_secs(3)) else { panic!("pausing is news") };
        assert_eq!(info.status, Status::Paused);
        assert!((info.elapsed - 92.0).abs() < 0.01, "the time played is folded in: {}", info.elapsed);
    }

    #[test]
    fn the_position_is_told_every_few_seconds_while_time_passes() {
        let mut m = Model::default();
        let t0 = Instant::now();
        m.apply(&Event::NowPlaying(Some(song("a", 200.0))), t0);
        m.apply(&Event::PlayState(PlayState::Playing), t0);
        let mut told = 0;
        for s in 1..=12u64 {
            match m.apply(&Event::Time { current: s as f64, duration: 200.0 }, t0 + Duration::from_secs(s)) {
                Some(Change::Position { elapsed, duration }) => {
                    told += 1;
                    assert_eq!((elapsed, duration), (s as f64, 200.0));
                }
                Some(other) => panic!("only the position moves: {other:?}"),
                None => {}
            }
        }
        assert_eq!(told, 2, "at 5 s and at 10 s");
    }

    #[test]
    fn the_end_of_the_bridge_stops_and_nothing_is_said_without_a_song() {
        let mut m = Model::default();
        let t0 = Instant::now();
        assert_eq!(m.apply(&Event::PlayState(PlayState::Playing), t0), None, "no song yet");
        assert_eq!(m.apply(&Event::Time { current: 3.0, duration: 0.0 }, t0), None);
        m.apply(&Event::NowPlaying(Some(song("a", 0.0))), t0);
        // A local file's length arrives with the first tick.
        let Some(Change::All(info)) = m.apply(&Event::Time { current: 0.5, duration: 180.0 }, t0) else { panic!("the length is news") };
        assert_eq!(info.duration, 180.0);
        let Some(Change::All(info)) = m.apply(&Event::Exited, t0) else { panic!("the end is news") };
        assert_eq!(info.status, Status::Stopped);
        assert_eq!(span(1.5).Duration, 15_000_000);
        assert_eq!(span(-1.0).Duration, 0);
    }

    /// The real controls on this machine: the hidden window, the buttons, a song with a cover.
    /// A machine with no desktop session may refuse; that is reported, not a failure.
    #[test]
    fn the_real_controls_take_a_song() {
        let smtc = match Smtc::start(|_| {}) {
            Ok(s) => s,
            Err(e) => return eprintln!("skipped: no media controls here: {e}"),
        };
        smtc.observe(&Event::NowPlaying(Some(song("a", 200.0))));
        smtc.observe(&Event::PlayState(PlayState::Playing));
        smtc.observe(&Event::Time { current: 30.0, duration: 200.0 });
        smtc.observe(&Event::PlayState(PlayState::Paused));
        std::thread::sleep(Duration::from_millis(300));
    }
}
