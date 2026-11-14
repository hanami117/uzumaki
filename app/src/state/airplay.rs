//! AirPlay: where the audio goes (this computer or one AirPlay speaker), as the app sees it.
//!
//! `crate::airplay` holds the controller; its events arrive as `Msg::AirPlay` and land in
//! `State::airplay` here. The picker (`ui/airplay.rs`), `uzu airplay` and the reconnect at
//! launch all go through the methods below. In headless runs and tests there is no controller:
//! the methods only change state, and scenes fill in made-up speakers.

use uzumaki_airplay::discover::Output;
use uzumaki_airplay::{Cast, Connect, Event, Need};

use super::State;
use crate::airplay::{Last, Prefs};

/// A password or a PIN the speaker asked for (the picker shows a field).
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub id: String,
    pub name: String,
    pub need: Need,
    /// The one just given was refused.
    pub wrong: bool,
}

#[derive(Default)]
pub struct AirPlay {
    /// The speakers found, while the picker is open (kept after it closes, for `uzu`).
    pub devices: Vec<Output>,
    pub cast: Cast,
    /// The bridge must capture (a speaker is connected). Sent again to a bridge that starts later.
    pub capture: bool,
    pub ask: Option<Ask>,
    /// Somebody is looking for speakers (the picker is open, or `uzu airplay` asked).
    pub browsing: bool,
    /// `uzu airplay` is looking: the picker closing doesn't end the search.
    pub ctl_browsing: bool,
    /// The helper program is installed. False: no AirPlay button, no speaker search, `uzu airplay` says so.
    pub available: bool,
    pub prefs: Prefs,
    /// How late the speaker plays, from the last start (for a later lyrics and progress offset).
    pub latency_ms: Option<u32>,
    /// Tests: how many flushes were asked for (there is no controller to count them).
    #[cfg(test)]
    pub flushes: std::cell::Cell<u32>,
}

/// What `uzu airplay` says when this install has no cliairplay helper.
pub const AIRPLAY_UNAVAILABLE: &str = "AirPlay is not available (this install has no cliairplay helper)";

impl AirPlay {
    pub fn new(persist: bool) -> Self {
        let prefs = if persist { Prefs::load() } else { Prefs::default() };
        AirPlay { available: crate::airplay::controller().is_some_and(|c| c.available()), prefs, ..Default::default() }
    }

    /// A speaker is connecting or playing.
    pub fn active(&self) -> bool {
        self.cast != Cast::Off
    }

    pub fn volume(&self) -> u8 {
        self.prefs.volume()
    }
}

impl State {
    /// One controller event. Returns whether the view changed.
    pub(super) fn airplay_event(&mut self, e: Event) -> bool {
        match e {
            Event::Devices(list) => self.airplay.devices = list,
            Event::State(cast) => {
                if let Cast::On { id, name, latency_ms } = &cast {
                    self.airplay.latency_ms = *latency_ms;
                    self.airplay.ask = None;
                    let last = Some(Last { id: id.clone(), name: name.clone() });
                    if self.airplay.prefs.last != last {
                        self.airplay.prefs.last = last;
                        self.airplay_save();
                    }
                }
                if cast == Cast::Off {
                    self.airplay.latency_ms = None;
                }
                self.airplay.cast = cast;
            }
            Event::Capture(on) => {
                self.airplay.capture = on;
                self.airplay_capture();
                return false;
            }
            Event::Failed { name, why } => {
                self.airplay.ask = None;
                self.toast = Some(format!("{} Playing on {}.", why.message(&name), crate::ui::this_computer()));
            }
            Event::Auth { id, name, need, wrong } => self.airplay.ask = Some(Ask { id, name, need, wrong }),
            Event::Paired { id, credentials } => {
                self.airplay.prefs.pairings.insert(id, credentials);
                self.airplay_save();
                return false;
            }
            Event::Forget { id } => {
                self.airplay.prefs.pairings.remove(&id);
                self.airplay_save();
                return false;
            }
        }
        true
    }

    fn airplay_save(&self) {
        if self.persist_settings {
            self.airplay.prefs.save();
        }
    }

    /// Tells the bridge whether to capture (when there is one; a later one is told at `Ready`).
    pub(super) fn airplay_capture(&self) {
        if let Some(b) = &self.bridge {
            b.airplay(self.airplay.capture);
        }
    }

    /// The picker opened or closed: speakers are looked for only meanwhile.
    pub fn airplay_browse(&mut self, on: bool) {
        if !self.airplay.available {
            return; // no helper: no mDNS traffic
        }
        let on = on || self.airplay.ctl_browsing;
        if self.airplay.browsing == on {
            return;
        }
        self.airplay.browsing = on;
        if let Some(c) = crate::airplay::controller() {
            c.browse(on);
        }
    }

    /// `uzu airplay`: looks for speakers for a moment, without the picker.
    pub fn airplay_ctl_browse(&mut self, on: bool) {
        self.airplay.ctl_browsing = on;
        if on {
            self.airplay_browse(true);
        } else if !self.ui.airplay.open {
            self.airplay_browse(false);
        }
    }

    fn airplay_connect(&self, id: &str) -> Connect {
        Connect { volume: self.airplay.volume(), password: None, auth: self.airplay.prefs.pairings.get(id).cloned() }
    }

    /// The picker's choice: a speaker's id, or None for this computer.
    pub fn airplay_pick(&mut self, id: Option<&str>) {
        self.airplay.ask = None;
        let Some(id) = id else {
            // "This computer" is a choice too: nothing to reconnect to next time.
            if self.airplay.prefs.last.take().is_some() {
                self.airplay_save();
            }
            if let Some(c) = crate::airplay::controller() {
                c.disconnect();
            } else {
                self.airplay.cast = Cast::Off;
            }
            return;
        };
        if self.airplay.cast.device().is_some_and(|(cur, _)| cur == id) {
            return; // already there (or on its way)
        }
        let Some(out) = self.airplay.devices.iter().find(|o| o.id == id).cloned() else { return };
        match crate::airplay::controller() {
            Some(c) => {
                if !c.connect(id, self.airplay_connect(id)) {
                    self.toast = Some(format!("“{}” is no longer on this network.", out.name));
                }
            }
            // Headless: the choice shows at once.
            None => self.airplay.cast = Cast::Connecting { id: out.id, name: out.name },
        }
    }

    /// `uzu airplay <name>` and the reconnect at launch: the speaker of that name.
    pub fn airplay_pick_named(&mut self, name: &str) -> Result<(), String> {
        if !self.airplay.available {
            return Err(AIRPLAY_UNAVAILABLE.into());
        }
        let Some(c) = crate::airplay::controller() else { return Err("AirPlay isn't running".into()) };
        self.airplay.ask = None;
        let id = self.airplay.devices.iter().find(|o| o.name.eq_ignore_ascii_case(name.trim())).map(|o| o.id.clone());
        let id = id.or_else(|| self.airplay.prefs.last.as_ref().filter(|l| l.name.eq_ignore_ascii_case(name.trim())).map(|l| l.id.clone()));
        let opts = match &id {
            Some(id) => self.airplay_connect(id),
            None => Connect { volume: self.airplay.volume(), password: None, auth: None },
        };
        c.connect_named(name, opts);
        Ok(())
    }

    /// The speaker's own volume, 0-100 (the app's volume slider stays the player's).
    pub fn airplay_volume(&mut self, volume: u8, done: bool) {
        let v = volume.min(100);
        if self.airplay.prefs.volume != Some(v) {
            self.airplay.prefs.volume = Some(v);
            if let Some(c) = crate::airplay::controller() {
                c.set_volume(v);
            }
        }
        if done {
            self.airplay_save();
        }
    }

    /// The password or PIN typed in the picker.
    pub fn airplay_answer(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(c) = crate::airplay::controller() {
            c.answer(text);
        }
        // The field goes; the row shows "Connecting…" until the speaker answers.
        if let Some(a) = self.airplay.ask.take() {
            self.airplay.cast = Cast::Connecting { id: a.id, name: a.name };
        }
    }

    /// The user seeked, paused or picked another song: the speaker drops what it still had
    /// queued, so the change is heard at once. Nothing while no speaker plays.
    pub(super) fn airplay_flush(&self) {
        if !self.airplay.active() {
            return;
        }
        #[cfg(test)]
        self.airplay.flushes.set(self.airplay.flushes.get() + 1);
        if let Some(c) = crate::airplay::controller() {
            c.flush();
        }
    }

    /// A song is about to start in place of the playing one. Flushes unless the playing one
    /// reached its end by itself: then its last second is still on its way to the speaker.
    pub(super) fn airplay_song_change(&self) {
        let pb = &self.playback;
        let dur = if pb.duration > 0.0 { pb.duration } else { pb.song.as_ref().map_or(0.0, |s| s.duration) };
        let ended = matches!(pb.state, crate::bridge::PlayState::Ended | crate::bridge::PlayState::Completed) || (dur > 0.0 && pb.position_now() >= dur - 1.5);
        if !ended {
            self.airplay_flush();
        }
    }

    pub fn set_airplay_reconnect(&mut self, on: bool) {
        self.airplay.prefs.reconnect = on;
        self.airplay_save();
    }

    /// At launch: back to the speaker of the last run, if the setting asks for it.
    pub fn airplay_reconnect(&mut self) {
        if !self.airplay.prefs.reconnect || !self.airplay.available {
            return;
        }
        if let Some(last) = self.airplay.prefs.last.clone() {
            eprintln!("airplay: reconnecting to \"{}\" (Settings → Playback)", last.name);
            let _ = self.airplay_pick_named(&last.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::PlayState;
    use uzumaki_airplay::Why;

    fn on(s: &mut State) {
        s.airplay_event(Event::State(Cast::On { id: "aa".into(), name: "Bedroom".into(), latency_ms: Some(480) }));
    }

    fn playing(s: &mut State, position: f64) {
        s.playback.duration = 200.0;
        s.playback.position = position;
        s.playback.position_at = None;
        s.playback.state = PlayState::Playing;
    }

    #[test]
    fn a_failure_says_why_and_that_the_sound_is_back_here() {
        let mut s = State::offline();
        on(&mut s);
        assert_eq!(s.airplay.latency_ms, Some(480), "kept for a later lyrics offset");
        assert_eq!(s.airplay.prefs.last.as_ref().map(|l| l.name.as_str()), Some("Bedroom"));
        assert!(s.airplay_event(Event::Failed { name: "Bedroom".into(), why: Why::Lost }));
        assert!(s.airplay_event(Event::State(Cast::Off)));
        let toast = s.toast.clone().unwrap();
        assert!(toast.contains("Bedroom") && toast.contains("was lost") && toast.contains(crate::ui::this_computer()), "{toast}");
        assert!(!s.airplay.active() && s.airplay.latency_ms.is_none());
        // The device is still remembered: a lost connection isn't the user's choice.
        assert!(s.airplay.prefs.last.is_some());
        s.airplay_pick(None);
        assert!(s.airplay.prefs.last.is_none(), "choosing this computer is");
    }

    #[test]
    fn questions_pairings_and_the_capture_follow_the_events() {
        let mut s = State::offline();
        s.airplay_event(Event::Auth { id: "dd".into(), name: "TV".into(), need: Need::Pin, wrong: false });
        assert_eq!(s.airplay.ask.as_ref().map(|a| a.need), Some(Need::Pin));
        s.airplay_event(Event::Paired { id: "dd".into(), credentials: "abcd".into() });
        assert_eq!(s.airplay_connect("dd").auth.as_deref(), Some("abcd"));
        assert_eq!(s.airplay_connect("other").auth, None);
        on(&mut s);
        assert!(s.airplay.ask.is_none(), "playing: the question is gone");
        s.airplay_event(Event::Forget { id: "dd".into() });
        assert_eq!(s.airplay_connect("dd").auth, None);
        assert!(!s.airplay_event(Event::Capture(true)), "nothing to draw");
        assert!(s.airplay.capture, "a bridge that starts later is told at Ready");
    }

    #[test]
    fn seek_pause_and_a_picked_song_flush_but_a_song_that_ended_does_not() {
        let mut s = State::offline();
        playing(&mut s, 50.0);
        s.seek_seconds(20.0);
        s.pause();
        assert_eq!(s.airplay.flushes.get(), 0, "no speaker: nothing to flush");
        on(&mut s);
        s.seek_seconds(30.0);
        assert_eq!(s.airplay.flushes.get(), 1);
        s.pause();
        assert_eq!(s.airplay.flushes.get(), 2);
        // The user picks another song mid-song.
        playing(&mut s, 50.0);
        s.airplay_song_change();
        assert_eq!(s.airplay.flushes.get(), 3);
        // The song reached its end: its last second is still on its way, so no flush.
        playing(&mut s, 199.5);
        s.airplay_song_change();
        s.playback.position = 10.0;
        s.playback.state = PlayState::Ended;
        s.airplay_song_change();
        assert_eq!(s.airplay.flushes.get(), 3);
    }
}
