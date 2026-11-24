//! What the desktop's media controls ask for, on every system: MPRIS on Linux (`mpris.rs`),
//! MediaPlayer.framework on macOS (`nowplaying.rs`), the System Media Transport Controls on
//! Windows (`smtc.rs`). `main.rs` routes the commands to the app state.

/// Which copy already runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Running {
    Current,
    /// A build from before the rename (still called Cadence). Its data must not be moved
    /// while it runs.
    #[cfg_attr(not(unix), allow(dead_code))]
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
    #[cfg_attr(windows, allow(dead_code))]
    Volume(f64),
    #[cfg_attr(windows, allow(dead_code))]
    Raise,
    #[cfg_attr(windows, allow(dead_code))]
    Quit,
}

use crate::bridge::{Event, PlayState};
use crate::models::Song;

/// Tells the platform's media controls about playback (`State::media_out`). Runs on the UI thread;
/// must not block.
pub type Observer = Box<dyn Fn(&Event) + Send>;

/// What the window shows of playback, the one thing the media controls may show.
pub struct Now<'a> {
    pub song: Option<&'a Song>,
    pub state: PlayState,
    /// Seconds, as heard (`Playback::heard_now`).
    pub position: f64,
    pub duration: f64,
    pub volume: f64,
    /// A time report was just applied: the position moved on (the controls' clock and seeks).
    pub tick: bool,
}

/// Turns the app's playback into the events the media controls understand (`observe(&Event)` in
/// `mpris.rs`, `nowplaying.rs`, `smtc.rs`), sending only what changed since the last call. The
/// controls then show what the window shows, not what the bridge or the local player said
/// before the app filtered it (the old queue winding down, a Cast device's delay, …).
#[derive(Default)]
pub struct Feed {
    song: Option<Song>,
    state: Option<PlayState>,
    volume: Option<f64>,
}

/// The three states the controls know.
fn shown_state(state: PlayState) -> PlayState {
    match state {
        s if s.is_active() => PlayState::Playing,
        PlayState::Paused => PlayState::Paused,
        _ => PlayState::Stopped,
    }
}

/// The parts of a song the controls show.
fn same_shown(a: &Song, b: &Song) -> bool {
    a.id == b.id
        && a.title == b.title
        && a.artist == b.artist
        && a.album == b.album
        && a.duration == b.duration
        && a.artwork.as_ref().map(|x| &x.template) == b.artwork.as_ref().map(|x| &x.template)
}

impl Feed {
    pub fn update(&mut self, now: Now<'_>) -> Vec<Event> {
        let mut out = Vec::new();
        if self.volume.is_none_or(|v| (v - now.volume).abs() > 0.001) {
            self.volume = Some(now.volume);
            out.push(Event::Volume(now.volume));
        }
        // Nothing to show (local only mode dropped the Apple song): the controls stop on the last one.
        let state = if now.song.is_some() { shown_state(now.state) } else { PlayState::Stopped };
        let mut changed = false;
        if let Some(song) = now.song.filter(|n| !self.song.as_ref().is_some_and(|s| same_shown(s, n))) {
            self.song = Some(song.clone());
            out.push(Event::NowPlaying(Some(song.clone())));
            changed = true;
        }
        if self.state != Some(state) {
            self.state = Some(state);
            out.push(Event::PlayState(state));
            changed = true;
        }
        if self.song.is_some() && now.song.is_some() && (changed || now.tick) {
            out.push(Event::Time { current: now.position, duration: now.duration });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: id.into(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_is_fallback: false, guessed: Default::default(),
            duration: 200.0,
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

    fn now(song: Option<&Song>, state: PlayState, position: f64, tick: bool) -> Now<'_> {
        Now { song, state, position, duration: 200.0, volume: 1.0, tick }
    }

    fn kinds(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                Event::NowPlaying(Some(s)) => format!("song {}", s.id),
                Event::PlayState(p) => format!("{p:?}"),
                Event::Time { current, .. } => format!("at {current}"),
                Event::Volume(v) => format!("volume {v}"),
                e => format!("{e:?}"),
            })
            .collect()
    }

    #[test]
    fn only_what_changed_is_sent() {
        let mut f = Feed::default();
        let a = song("a");
        assert_eq!(kinds(&f.update(now(Some(&a), PlayState::Loading, 0.0, false))), ["volume 1", "song a", "Playing", "at 0"]);
        assert!(f.update(now(Some(&a), PlayState::Playing, 0.0, false)).is_empty(), "loading and playing look the same");
        assert_eq!(kinds(&f.update(now(Some(&a), PlayState::Playing, 1.0, true))), ["at 1"]);
        assert_eq!(kinds(&f.update(now(Some(&a), PlayState::Paused, 1.5, false))), ["Paused", "at 1.5"]);
        assert_eq!(kinds(&f.update(now(Some(&a), PlayState::Ended, 1.5, false))), ["Stopped", "at 1.5"]);
        let b = song("b");
        assert_eq!(kinds(&f.update(now(Some(&b), PlayState::Paused, 42.0, false))), ["song b", "Paused", "at 42"]);
        assert_eq!(kinds(&f.update(now(None, PlayState::Stopped, 0.0, false))), ["Stopped"]);
    }
}
