//! MPRIS2 service (org.mpris.MediaPlayer2.uzumaki): media keys and desktop player widgets.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, Value};

use crate::bridge::{Event, PlayState};
use crate::models::Song;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.uzumaki";
/// The name the app had before it was renamed (an old copy may still be running).
/// The name `start` claims. Tests use their own, so a test run never takes the running app's
/// name (and its media keys and Plasma widget) away from it.
fn bus_name() -> String {
    if cfg!(test) { format!("{BUS_NAME}.test{}", std::process::id()) } else { BUS_NAME.to_string() }
}
const LEGACY_BUS_NAME: &str = "org.mpris.MediaPlayer2.cadence";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
/// A Time event this far from where playback should be means the user (or the app) seeked.
const SEEK_JUMP: f64 = 2.0;


/// One Uzumaki at a time: when another one already owns the MPRIS name, asks it to show its
/// window (it may be closed to the tray) and returns true. A second copy would start a second
/// Electron on the same profile.
pub fn raise_running() -> Option<Running> {
    find_running(true)
}

/// Like `raise_running`, but leaves the other copy's window alone (`uzumaki --hidden`).
pub fn running() -> Option<Running> {
    find_running(false)
}

fn find_running(raise: bool) -> Option<Running> {
    let conn = Connection::session().ok()?;
    let found = |name: &str| {
        if raise {
            conn.call_method(Some(name), PATH, Some("org.mpris.MediaPlayer2"), "Raise", &()).is_ok()
        } else {
            let owner = conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "NameHasOwner", &(name,));
            owner.ok().and_then(|m| m.body().deserialize::<bool>().ok()).unwrap_or(false)
        }
    };
    if found(BUS_NAME) {
        Some(Running::Current)
    } else if found(LEGACY_BUS_NAME) {
        Some(Running::Legacy)
    } else {
        None
    }
}

/// Which copy already runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Running {
    Current,
    /// A build from before the rename (still called Cadence). Its data must not be moved
    /// while it runs.
    Legacy,
}

/// What the desktop asked for. `main.rs` routes these to the bridge / app handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Next,
    Previous,
    /// Absolute position, seconds.
    SeekTo(f64),
    /// 0..1
    Volume(f64),
    Raise,
    Quit,
}


type Callback = Arc<dyn Fn(Command) + Send + Sync>;

#[derive(Default)]
struct Shared {
    status: &'static str,
    song: Option<Song>,
    /// Seconds, from the last Time event, and when it arrived.
    position: f64,
    position_at: Option<Instant>,
    volume: f64,
}

impl Shared {
    fn new() -> Self {
        Self { status: "Stopped", volume: 1.0, ..Default::default() }
    }

    fn track_id(&self) -> String {
        match &self.song {
            Some(s) => track_path(&s.id),
            None => NO_TRACK.to_string(),
        }
    }

    fn metadata(&self) -> HashMap<String, Value<'static>> {
        let mut m: HashMap<String, Value<'static>> = HashMap::new();
        let id = ObjectPath::try_from(self.track_id()).unwrap_or_else(|_| ObjectPath::from_static_str_unchecked(NO_TRACK));
        m.insert("mpris:trackid".into(), Value::from(id));
        if let Some(s) = &self.song {
            m.insert("mpris:length".into(), Value::from((s.duration * 1e6) as i64));
            m.insert("xesam:title".into(), Value::from(s.title.clone()));
            m.insert("xesam:artist".into(), Value::from(vec![s.artist.clone()]));
            m.insert("xesam:album".into(), Value::from(s.album.clone()));
            if let Some(a) = &s.artwork {
                m.insert("mpris:artUrl".into(), Value::from(a.url(600)));
            }
            if s.is_local() {
                m.insert("xesam:url".into(), Value::from(s.id.clone()));
            }
        }
        m
    }

    /// Where playback should be now, extrapolating from the last Time event while playing.
    fn expected_position(&self) -> f64 {
        match (self.status, self.position_at) {
            ("Playing", Some(at)) => self.position + at.elapsed().as_secs_f64(),
            _ => self.position,
        }
    }
}

/// `/org/uzumaki/track/<id with anything but [A-Za-z0-9_] turned into _>`.
fn track_path(id: &str) -> String {
    let mut clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if clean.is_empty() {
        clean.push('_');
    }
    format!("/org/uzumaki/track/{clean}")
}

fn status_of(state: PlayState) -> &'static str {
    if state.is_active() {
        "Playing"
    } else if state == PlayState::Paused {
        "Paused"
    } else {
        "Stopped"
    }
}

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Root {
    on_command: Callback,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        (self.on_command)(Command::Raise);
    }

    fn quit(&self) {
        (self.on_command)(Command::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_set_fullscreen(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> &str {
        "Uzumaki"
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        crate::APP_ID
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    shared: Arc<Mutex<Shared>>,
    on_command: Callback,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play(&self) {
        (self.on_command)(Command::Play);
    }

    fn pause(&self) {
        (self.on_command)(Command::Pause);
    }

    fn play_pause(&self) {
        let playing = lock(&self.shared).status == "Playing";
        (self.on_command)(if playing { Command::Pause } else { Command::Play });
    }

    fn stop(&self) {
        (self.on_command)(Command::Pause);
    }

    fn next(&self) {
        (self.on_command)(Command::Next);
    }

    fn previous(&self) {
        (self.on_command)(Command::Previous);
    }

    /// Relative seek, microseconds.
    fn seek(&self, offset: i64) {
        let now = lock(&self.shared).expected_position();
        let target = (now + offset as f64 / 1e6).max(0.0);
        (self.on_command)(Command::SeekTo(target));
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        if track_id.as_str() == lock(&self.shared).track_id() && position >= 0 {
            (self.on_command)(Command::SeekTo(position as f64 / 1e6));
        }
    }

    fn open_uri(&self, _uri: &str) {}

    #[zbus(property)]
    fn playback_status(&self) -> String {
        lock(&self.shared).status.to_string()
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        lock(&self.shared).metadata()
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        lock(&self.shared).volume
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        let v = volume.clamp(0.0, 1.0);
        lock(&self.shared).volume = v;
        (self.on_command)(Command::Volume(v));
    }

    /// Microseconds; not signalled (clients poll, or use `Seeked`).
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        (lock(&self.shared).expected_position() * 1e6) as i64
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

pub struct Mpris {
    conn: Connection,
    shared: Arc<Mutex<Shared>>,
}

impl Mpris {
    /// Registers on the session bus. `on_command` runs on a D-Bus thread.
    pub fn start(on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<Self, String> {
        let on_command: Callback = Arc::new(on_command);
        let shared = Arc::new(Mutex::new(Shared::new()));
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name(bus_name()))
            .and_then(|b| b.serve_at(PATH, Root { on_command: on_command.clone() }))
            .and_then(|b| b.serve_at(PATH, Player { shared: shared.clone(), on_command }))
            .and_then(|b| b.build())
            .map_err(|e| format!("{e}"))?;
        Ok(Self { conn, shared })
    }

    /// Called on the bridge reader thread for every bridge event (now playing, play state, time,
    /// ready/volume), before the UI sees it. Must not block.
    pub fn observe(&self, event: &Event) {
        let mut changed: HashMap<&str, Value<'_>> = HashMap::new();
        let mut seeked: Option<i64> = None;
        {
            let mut s = lock(&self.shared);
            match event {
                // None = MusicKit swapping queues; keep the last track instead of flickering.
                Event::NowPlaying(song) if song.is_some() => {
                    s.song = song.clone();
                    s.position = 0.0;
                    s.position_at = Some(Instant::now());
                    changed.insert("Metadata", Value::from(s.metadata()));
                }
                Event::PlayState(state) => {
                    let status = status_of(*state);
                    if status != s.status {
                        // Fold elapsed time in so the extrapolation stays right across the change.
                        s.position = s.expected_position();
                        s.position_at = Some(Instant::now());
                        s.status = status;
                        changed.insert("PlaybackStatus", Value::from(status));
                    }
                }
                Event::Time { current, .. } => {
                    if s.position_at.is_some() && (current - s.expected_position()).abs() > SEEK_JUMP {
                        seeked = Some((current * 1e6) as i64);
                    }
                    s.position = *current;
                    s.position_at = Some(Instant::now());
                }
                // Stopped for local only mode: nothing plays any more.
                Event::Stopped if s.status != "Stopped" => {
                    s.position = s.expected_position();
                    s.position_at = Some(Instant::now());
                    s.status = "Stopped";
                    changed.insert("PlaybackStatus", Value::from("Stopped"));
                }
                Event::Ready { volume: Some(v), .. } | Event::Volume(v) => {
                    if (*v - s.volume).abs() > f64::EPSILON {
                        s.volume = *v;
                        changed.insert("Volume", Value::from(*v));
                    }
                }
                _ => {}
            }
        }
        if !changed.is_empty() {
            let _ = self.conn.emit_signal(
                None::<&str>,
                PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(PLAYER_IFACE, changed, Vec::<&str>::new()),
            );
        }
        if let Some(pos) = seeked {
            let _ = self.conn.emit_signal(None::<&str>, PATH, PLAYER_IFACE, "Seeked", &(pos,));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_path_is_valid_object_path() {
        for id in ["1440857781", "i.abc-DEF/x y", "", "é"] {
            assert!(ObjectPath::try_from(track_path(id)).is_ok(), "{id}");
        }
    }

    #[test]
    fn statuses() {
        assert_eq!(status_of(PlayState::Playing), "Playing");
        assert_eq!(status_of(PlayState::Paused), "Paused");
        for s in [PlayState::None, PlayState::Stopped, PlayState::Ended, PlayState::Completed] {
            assert_eq!(status_of(s), "Stopped");
        }
    }

    fn busctl(args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("busctl").arg("--user").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Talks to the real session bus with busctl. Skipped without a bus.
    #[test]
    fn busctl_roundtrip() {
        let name = bus_name();
        let name = name.as_str();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let Ok(m) = Mpris::start(move |c| {
            let _ = tx.lock().unwrap().send(c);
        }) else {
            eprintln!("no session bus (or name taken): skipping");
            return;
        };
        let song = Song {
            id: "i.ab-1".into(),
            catalog_id: None,
            title: "Title".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_is_fallback: false,
            duration: 200.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: Some(crate::models::Artwork { template: "https://x/{w}x{h}bb.jpg".into(), bg: None }),
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: Vec::new(),
        };
        m.observe(&Event::Ready { authorized: true, storefront: None, volume: Some(0.5) });
        m.observe(&Event::NowPlaying(Some(song)));
        m.observe(&Event::PlayState(PlayState::Playing));
        m.observe(&Event::Time { current: 10.0, duration: 200.0 });

        let get = |p: &str| busctl(&["get-property", name, PATH, PLAYER_IFACE, p]).unwrap();
        assert_eq!(get("PlaybackStatus"), "s \"Playing\"");
        assert_eq!(get("Volume"), "d 0.5");
        let md = get("Metadata");
        for want in ["\"Title\"", "\"Artist\"", "\"Album\"", "https://x/600x600bb.jpg", "/org/uzumaki/track/i_ab_1", "200000000"] {
            assert!(md.contains(want), "{want} missing in {md}");
        }
        assert!(get("Position").starts_with("x 10"), "{}", get("Position"));
        assert_eq!(busctl(&["get-property", name, PATH, "org.mpris.MediaPlayer2", "Identity"]).unwrap(), "s \"Uzumaki\"");

        let call = |method: &str, sig_args: &[&str]| {
            let mut a = vec!["call", name, PATH, PLAYER_IFACE, method];
            a.extend_from_slice(sig_args);
            busctl(&a).expect(method);
            rx.recv_timeout(std::time::Duration::from_secs(2)).expect(method)
        };
        assert_eq!(call("PlayPause", &[]), Command::Pause);
        m.observe(&Event::PlayState(PlayState::Paused));
        assert_eq!(get("PlaybackStatus"), "s \"Paused\"");
        assert_eq!(call("PlayPause", &[]), Command::Play);
        assert_eq!(call("Next", &[]), Command::Next);
        assert_eq!(call("Previous", &[]), Command::Previous);
        assert_eq!(call("Stop", &[]), Command::Pause);
        match call("Seek", &["x", "5000000"]) {
            Command::SeekTo(t) => assert!((t - 15.0).abs() < 0.5, "{t}"),
            c => panic!("{c:?}"),
        }
        assert_eq!(
            call("SetPosition", &["ox", "/org/uzumaki/track/i_ab_1", "30000000"]),
            Command::SeekTo(30.0)
        );
        busctl(&["set-property", name, PATH, PLAYER_IFACE, "Volume", "d", "0.25"]).unwrap();
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), Command::Volume(0.25));
    }
}
