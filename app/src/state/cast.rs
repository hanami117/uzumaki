//! Chromecast in the app state: where the Cast session is, the devices found, the device volume.
//!
//! `crate::cast` holds the controller; its events arrive as `Msg::Cast`. The picker
//! (`ui/airplay.rs`, one picker for AirPlay and Cast devices) and `uzu cast` drive it, only while
//! Settings → Playback → Chromecast is on. One output at a time: a Cast device ends the AirPlay
//! speakers and the other way round. Headless runs and tests have no controller: the methods
//! only change state, and scenes fill in made-up devices.
//!
//! The device plays a couple of seconds behind the player (its receiver waits for a buffer):
//! lyrics and the progress bar follow the measured delay (`Playback::heard_now`), pause and
//! resume are instant (the stream holds), and a seek, a skip or a picked song starts the
//! device afresh (`Controller::restart`: a short silence, then the new place in step).

use std::time::{Duration, Instant};

use uzumaki_cast::{CastDevice, Event, Meta, State as CastState};

use super::State;
use crate::models::Song;

#[derive(Default)]
pub struct Cast {
    pub state: CastState,
    /// The bridge must capture for the Cast session's socket.
    pub capture: bool,
    /// The Cast devices found while the picker is open (kept after it closes).
    pub devices: Vec<CastDevice>,
    /// Somebody looks for Cast devices (the picker is open).
    pub browsing: bool,
    /// The device's own volume, 0-100, as it last said (None until a session hears it).
    pub volume: Option<u8>,
    /// A skip sent to the bridge: the next song MusicKit announces starts the device afresh.
    cut: Option<Instant>,
}

impl Cast {
    /// A session is connecting, playing or paused.
    pub fn active(&self) -> bool {
        self.state != CastState::Off
    }
}

/// How long a skip waits for MusicKit's next song before it is forgotten.
const CUT_WAIT: Duration = Duration::from_secs(10);

impl State {
    /// Settings → Playback → Chromecast is on (or `UZUMAKI_CAST_SPIKE=1`), on a system that has it.
    pub fn cast_enabled(&self) -> bool {
        crate::cast::SUPPORTED && (self.settings.chromecast || crate::cast::forced())
    }

    /// Settings → Playback → Chromecast. Off ends a session and stops looking for devices.
    pub fn set_chromecast(&mut self, on: bool) {
        if self.settings.chromecast == on {
            return;
        }
        self.settings.chromecast = on;
        self.save_settings();
        if self.cast_enabled() {
            if self.ui.airplay.open {
                self.cast_browse(true);
            }
        } else {
            self.cast_stop();
            self.cast_browse(false);
            self.cast.devices.clear();
        }
    }

    /// An event of the controller. True when the view changes.
    pub(super) fn cast_event(&mut self, e: Event) -> bool {
        match e {
            Event::State(s) => {
                // Lyrics and the progress bar follow what the device plays (its measured buffer).
                let delay = match &s {
                    CastState::Playing { latency_ms: Some(ms), .. } => f64::from(*ms) / 1000.0,
                    CastState::Playing { .. } | CastState::Buffering { .. } | CastState::Paused { .. } => self.playback.output_delay,
                    _ => 0.0,
                };
                if s == CastState::Off {
                    self.cast.volume = None;
                    self.cast.cut = None;
                }
                let changed = (delay - self.playback.output_delay).abs() > 0.01 || std::mem::discriminant(&s) != std::mem::discriminant(&self.cast.state);
                self.playback.output_delay = delay;
                self.cast.state = s;
                changed
            }
            Event::Capture(on) => {
                self.cast.capture = on;
                self.cast_capture();
                false
            }
            Event::Failed { name, why, detail } => {
                eprintln!("cast: {why:?} {detail}");
                self.toast = Some(format!("{} Playing on {}.", why.message(&name), crate::ui::this_computer()));
                true
            }
            Event::Devices(list) => {
                self.cast.devices = list;
                true
            }
            Event::Volume { level, .. } => {
                let v = (level * 100.0).round().clamp(0.0, 100.0) as u8;
                let changed = self.cast.volume != Some(v);
                self.cast.volume = Some(v);
                changed && self.ui.airplay.open
            }
        }
    }

    /// Tells the bridge to capture for the Cast session (or to stop, unless AirPlay wants it).
    pub(super) fn cast_capture(&mut self) {
        let Some(c) = crate::cast::controller() else { return };
        c.set_bridge(self.bridge.is_some());
        let Some(b) = &self.bridge else { return };
        if self.cast.capture {
            b.airplay_to(true, Some(&c.socket()));
        } else if !self.airplay.capture {
            b.airplay(false);
        }
    }

    pub(super) fn cast_capture_ready(&self, ready: bool, error: Option<&str>) {
        if self.cast.capture
            && let Some(c) = crate::cast::controller()
        {
            c.capture_ready(ready, error);
        }
    }

    /// The picker opened or closed: Cast devices are looked for only meanwhile (and only while
    /// the setting is on: off, no mDNS at all).
    pub fn cast_browse(&mut self, on: bool) {
        let on = on && self.cast_enabled();
        if self.cast.browsing == on {
            return;
        }
        self.cast.browsing = on;
        if let Some(c) = crate::cast::controller() {
            c.browse(on);
        }
    }

    /// The picker's choice: the Cast device `id` (AirPlay speakers end first).
    pub fn cast_pick(&mut self, id: &str) {
        if !self.cast_enabled() {
            return;
        }
        let Some(device) = self.cast.devices.iter().find(|d| d.id == id).cloned() else { return };
        if self.airplay.active() {
            self.airplay_pick(None);
        }
        if self.cast.state.name() == Some(device.name.as_str()) {
            return; // already there (or on its way)
        }
        self.cast.volume = None;
        match crate::cast::controller() {
            Some(c) => c.connect(device, self.cast_meta()),
            // Headless: the choice shows at once.
            None => self.cast.state = CastState::Connecting { name: device.name },
        }
    }

    /// Back to this computer (the session there was ends; the TV stops at once).
    pub fn cast_stop(&mut self) {
        if !self.cast.active() {
            return;
        }
        match crate::cast::controller() {
            Some(c) => c.stop(),
            None => self.cast.state = CastState::Off,
        }
    }

    /// The device's own volume (the picker's slider under its row), 0-100.
    pub fn cast_volume(&mut self, v: u8) {
        let v = v.min(100);
        if self.cast.volume == Some(v) {
            return;
        }
        self.cast.volume = Some(v);
        if let Some(c) = crate::cast::controller() {
            c.set_volume(v);
        }
    }

    /// AirPlay was chosen: the Cast session ends.
    pub(super) fn cast_yield(&mut self) {
        self.cast_stop();
    }

    /// The device has a session that plays or will (a command now reaches it).
    fn cast_live(&self) -> bool {
        matches!(self.cast.state, CastState::Loading { .. } | CastState::Playing { .. } | CastState::Buffering { .. } | CastState::Paused { .. })
    }

    /// The user paused (true) or played on: the TV does the same at once. Its buffer holds the
    /// next seconds and the stream sends nothing meanwhile, so it resumes in step with the player.
    pub(super) fn cast_pause(&self, on: bool) {
        if !matches!(self.cast.state, CastState::Playing { .. } | CastState::Buffering { .. } | CastState::Paused { .. }) {
            return;
        }
        if let Some(c) = crate::cast::controller() {
            if on {
                c.pause();
            } else {
                c.resume();
            }
        }
    }

    /// A seek: the device drops the seconds it still had and starts at the new place.
    pub(super) fn cast_seeked(&self) {
        if self.cast_live()
            && let Some(c) = crate::cast::controller()
        {
            c.restart(Some(self.cast_meta()));
        }
    }

    /// `song` is about to start in place of the playing one. Unless the playing one reached its
    /// end by itself (then its last seconds are still on their way), the device starts afresh,
    /// with the new song's title.
    pub(super) fn cast_song_change(&self, song: &Song) {
        if !self.cast_live() || self.song_ended() {
            return;
        }
        if let Some(c) = crate::cast::controller() {
            c.restart(Some(meta_of(Some(song))));
        }
    }

    /// Next or Previous sent to MusicKit: the old audio is dropped now, and the device starts
    /// afresh when the new song is announced (`cast_new_song`), so it shows its title.
    pub(super) fn cast_skip(&mut self) {
        if !self.cast_live() {
            return;
        }
        self.cast.cut = Some(Instant::now());
        if let Some(c) = crate::cast::controller() {
            c.forget();
        }
    }

    /// MusicKit announced another song. After a skip: the device starts afresh with it. A song
    /// that follows on by itself plays on without a gap (the device keeps the old title).
    pub(super) fn cast_new_song(&mut self) {
        let Some(t) = self.cast.cut.take() else { return };
        if t.elapsed() > CUT_WAIT || !self.cast_live() {
            return;
        }
        if let Some(c) = crate::cast::controller() {
            c.restart(Some(self.cast_meta()));
        }
    }

    fn cast_meta(&self) -> Meta {
        meta_of(self.playback.song.as_ref())
    }

    /// `uzu cast …` (on the UI thread; `List` is answered by `ctl.rs` itself, it waits).
    pub fn ctl_cast(&mut self, to: uzumaki_ctl::CastTo) -> Result<Vec<String>, String> {
        use uzumaki_ctl::CastTo;
        if !self.cast_enabled() {
            return Err(crate::cast::OFF.into());
        }
        let c = crate::cast::controller().ok_or("Chromecast isn't running")?;
        let sent = |ok: bool| if ok { Ok(vec!["sent".to_string()]) } else { Err("no Cast session (uzu cast <name> first)".to_string()) };
        match to {
            CastTo::List | CastTo::Status => Ok(c.status_lines()),
            CastTo::Off => {
                c.stop();
                Ok(vec!["back to this computer".into()])
            }
            CastTo::Pause => sent(c.pause()),
            CastTo::Resume => sent(c.resume()),
            CastTo::Reload => sent(c.restart(Some(self.cast_meta()))),
            CastTo::Volume(v) => sent(c.set_volume(v)),
            CastTo::Name(name) => {
                if self.airplay.active() {
                    self.airplay_pick(None);
                }
                c.connect_named(&name, self.cast_meta());
                Ok(vec![format!("connecting to “{name}” (uzu cast status)")])
            }
        }
    }
}

/// What the device shows for `song` (its cover only over https: the device fetches it itself).
fn meta_of(song: Option<&Song>) -> Meta {
    match song {
        Some(s) => Meta { title: s.title.clone(), artist: s.artist.clone(), album: s.album.clone(), image: s.artwork.as_ref().map(|a| a.url(600)).filter(|u| u.starts_with("https://")) },
        None => Meta { title: "Uzumaki".into(), ..Meta::default() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn tv() -> CastDevice {
        CastDevice::at(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 173)), 8009, "Smart TV Pro")
    }

    #[test]
    fn off_by_default_nothing_looks_and_uzu_says_how_to_turn_it_on() {
        let mut s = State::offline();
        assert!(!s.settings.chromecast, "off by default");
        assert_eq!(s.cast_enabled(), crate::cast::forced());
        if crate::cast::forced() {
            return; // a developer's shell with UZUMAKI_CAST_SPIKE=1
        }
        assert_eq!(s.ctl_cast(uzumaki_ctl::CastTo::Status), Err(crate::cast::OFF.to_string()));
        s.cast_browse(true);
        assert!(!s.cast.browsing, "no mDNS while off");
        s.cast.devices = vec![tv()];
        s.cast_pick(&tv().id);
        assert!(!s.cast.active(), "no pick while off");
        assert!(crate::cast::controller().is_none());
    }

    #[test]
    fn on_it_looks_while_asked_and_a_pick_ends_airplay() {
        let mut s = State::offline();
        s.settings.chromecast = true;
        if !crate::cast::SUPPORTED {
            return;
        }
        s.cast_browse(true);
        assert!(s.cast.browsing);
        assert!(s.cast_event(Event::Devices(vec![tv()])));
        // An AirPlay speaker plays: picking the TV ends it.
        s.airplay.cast = uzumaki_airplay::Cast::On { id: "aa".into(), name: "Bedroom".into(), latency_ms: None };
        s.cast_pick(&tv().id);
        assert_eq!(s.airplay.cast, uzumaki_airplay::Cast::Off);
        assert_eq!(s.cast.state, CastState::Connecting { name: "Smart TV Pro".into() });
        // And the other way round.
        s.airplay.devices = vec![uzumaki_airplay::discover::Output { id: "aa".into(), name: "Bedroom".into(), kind: uzumaki_airplay::discover::Kind::HomePod, pair: false, password: false }];
        s.airplay_pick(Some("aa"));
        assert!(!s.cast.active());
        // Turning the setting off ends the session and the search, and forgets the devices.
        s.cast_pick(&tv().id);
        s.set_chromecast(false);
        if !crate::cast::forced() {
            assert!(!s.cast.active() && !s.cast.browsing && s.cast.devices.is_empty());
        }
    }

    #[test]
    fn events_move_the_state_volume_delay_and_toasts() {
        let mut s = State::offline();
        assert!(s.cast_event(Event::Failed { name: "TV".into(), why: uzumaki_cast::Why::Lost, detail: String::new() }));
        let toast = s.toast.clone().unwrap();
        assert!(toast.contains("was lost") && toast.contains(crate::ui::this_computer()), "{toast}");
        assert!(s.cast_event(Event::State(CastState::Connecting { name: "TV".into() })));
        assert_eq!(s.cast.state.name(), Some("TV"));
        s.ui.airplay.open = true;
        assert!(s.cast_event(Event::Volume { level: 0.26, muted: false }));
        assert_eq!(s.cast.volume, Some(26));
        assert!(!s.cast_event(Event::Volume { level: 0.26, muted: false }), "the same volume: nothing to draw");
        // The measured delay shifts what lyrics and the progress bar show; back to 0 when off.
        s.playback.position = 10.0;
        assert!(s.cast_event(Event::State(CastState::Playing { name: "TV".into(), latency_ms: Some(2100) })));
        assert!((s.playback.heard_now() - 7.9).abs() < 1e-9);
        s.cast_event(Event::State(CastState::Buffering { name: "TV".into() }));
        assert!((s.playback.heard_now() - 7.9).abs() < 1e-9, "kept while it rebuffers");
        assert!(s.cast_event(Event::State(CastState::Off)));
        assert_eq!(s.playback.heard_now(), 10.0);
        assert_eq!(s.cast.volume, None);
    }

    #[test]
    fn a_skip_waits_for_the_next_song_once() {
        let mut s = State::offline();
        s.cast_skip();
        assert!(s.cast.cut.is_none(), "no session: nothing to cut");
        s.cast.state = CastState::Playing { name: "TV".into(), latency_ms: Some(2400) };
        s.cast_skip();
        assert!(s.cast.cut.is_some());
        s.cast_new_song();
        assert!(s.cast.cut.is_none(), "used once");
    }
}
