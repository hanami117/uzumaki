//! The volume of wherever the sound goes, for the macOS menu bar slider (`tray_mac.rs`).
//!
//! It is the volume the window's own control sets for the output in use, through the same
//! actions: AirPlay speakers → the picker's master volume (`airplay_master_volume`; with one
//! speaker, that speaker's own), a Chromecast → the device's volume (`cast_volume`), this
//! computer → the app's volume (`set_volume`, the player bar's slider). No volume of its own.
//!
//! `Throttle` keeps a drag from sending every mouse move to a speaker: at most one value per
//! `THROTTLE`, and always the last one.

use std::time::{Duration, Instant};

use super::State;

/// Where the sound goes now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    ThisComputer,
    AirPlay,
    Cast,
}

/// What the slider shows.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputVolume {
    pub output: Output,
    /// "This Mac", "Bedroom", "Bedroom and Living Room", the Chromecast's name.
    pub label: String,
    /// 0..1; None while the output hasn't said its volume yet (a Cast session starting).
    pub volume: Option<f64>,
}

impl State {
    /// Where the sound goes: AirPlay and Cast are never on together.
    pub fn output(&self) -> Output {
        if self.airplay.active() {
            Output::AirPlay
        } else if self.cast.active() {
            Output::Cast
        } else {
            Output::ThisComputer
        }
    }

    /// The output in use and its volume, as the window's control for it shows it.
    pub fn output_volume(&self) -> OutputVolume {
        match self.output() {
            Output::AirPlay => OutputVolume { output: Output::AirPlay, label: self.airplay.names(), volume: Some(f64::from(self.airplay.master()) / 100.0) },
            Output::Cast => OutputVolume {
                output: Output::Cast,
                label: self.cast.state.name().unwrap_or("Chromecast").to_string(),
                volume: self.cast.volume.map(|v| f64::from(v) / 100.0),
            },
            Output::ThisComputer => OutputVolume { output: Output::ThisComputer, label: crate::ui::this_computer().to_string(), volume: Some(self.playback.volume) },
        }
    }

    /// Sets the volume of the output in use (0..1). `done`: the drag ended (or a single step),
    /// so AirPlay saves the speakers' volumes and the next drag starts from where they are.
    pub fn set_output_volume(&mut self, volume: f64, done: bool) {
        let v = volume.clamp(0.0, 1.0);
        let percent = (v * 100.0).round() as u8;
        match self.output() {
            Output::AirPlay => {
                if self.output_drag.is_none() {
                    self.output_drag = Some(self.airplay_volumes());
                }
                let from = self.output_drag.clone().unwrap_or_default();
                self.airplay_master_volume(percent, &from, done);
            }
            Output::Cast => {
                if self.cast.volume.is_some() {
                    self.cast_volume(percent);
                }
            }
            Output::ThisComputer => self.set_volume(v),
        }
        if done {
            self.output_drag = None;
        }
    }
}

/// At most one value sent per this while the slider is dragged.
pub const THROTTLE: Duration = Duration::from_millis(100);

/// Spaces out a slider's values: the first goes at once, later ones at most every `THROTTLE`,
/// and the last one is never lost (`flush`, or a `done` value).
#[derive(Debug, Default)]
pub struct Throttle {
    sent_at: Option<Instant>,
    pending: Option<f64>,
}

impl Throttle {
    /// A new slider value: what to send now, if anything.
    pub fn push(&mut self, v: f64, now: Instant, done: bool) -> Option<f64> {
        let due = done || self.sent_at.is_none_or(|t| now.saturating_duration_since(t) >= THROTTLE);
        if due {
            self.pending = None;
            self.sent_at = Some(now);
            Some(v)
        } else {
            self.pending = Some(v);
            None
        }
    }

    /// The value held back, once its time has come (`force`: now, e.g. the menu closed).
    pub fn flush(&mut self, now: Instant, force: bool) -> Option<f64> {
        let due = force || self.sent_at.is_none_or(|t| now.saturating_duration_since(t) >= THROTTLE);
        if !due {
            return None;
        }
        let v = self.pending.take()?;
        self.sent_at = Some(now);
        Some(v)
    }

    /// A value is waiting for `flush`.
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uzumaki_airplay::{Cast, Event, Speaker};
    use uzumaki_cast::State as CastState;

    fn group(s: &mut State, names: &[(&str, &str)]) {
        let list: Vec<Speaker> = names.iter().map(|(id, name)| Speaker { id: id.to_string(), name: name.to_string(), playing: true }).collect();
        s.airplay_event(Event::Speakers(list));
        let (id, name) = names[0];
        s.airplay_event(Event::State(Cast::On { id: id.into(), name: name.into(), latency_ms: Some(400) }));
    }

    #[test]
    fn on_this_computer_it_is_the_apps_volume() {
        let mut s = State::offline();
        s.playback.volume = 0.4;
        let o = s.output_volume();
        assert_eq!(o.output, Output::ThisComputer);
        assert_eq!(o.label, crate::ui::this_computer());
        assert_eq!(o.volume, Some(0.4));
        s.set_output_volume(0.75, false);
        assert_eq!(s.playback.volume, 0.75, "the player bar's volume");
        s.set_output_volume(1.7, true);
        assert_eq!(s.playback.volume, 1.0, "clamped");
        assert!(s.output_drag.is_none());
    }

    #[test]
    fn on_one_airplay_speaker_it_is_that_speakers_volume_and_the_apps_stays() {
        let mut s = State::offline();
        s.playback.volume = 0.5;
        group(&mut s, &[("aa", "Bedroom")]);
        s.airplay_speaker_volume("aa", 30, true);
        let o = s.output_volume();
        assert_eq!((o.output, o.label.as_str(), o.volume), (Output::AirPlay, "Bedroom", Some(0.3)));
        s.set_output_volume(0.62, false);
        s.set_output_volume(0.64, true);
        assert_eq!(s.airplay.prefs.volume_of("aa"), 64, "the picker's slider for Bedroom");
        assert_eq!(s.playback.volume, 0.5, "the app's volume is left alone");
        assert_eq!(s.output_volume().volume, Some(0.64));
    }

    #[test]
    fn on_several_speakers_it_is_the_master_volume_keeping_the_balance() {
        let mut s = State::offline();
        group(&mut s, &[("aa", "Bedroom"), ("bb", "Living Room")]);
        s.airplay_speaker_volume("aa", 40, true);
        s.airplay_speaker_volume("bb", 80, true);
        let o = s.output_volume();
        assert_eq!(o.label, "Bedroom and Living Room");
        assert_eq!(o.volume, Some(0.8), "the loudest, as the picker's master");
        // One drag down to nothing and back: each speaker comes back to its share.
        s.set_output_volume(0.4, false);
        assert_eq!((s.airplay.prefs.volume_of("aa"), s.airplay.prefs.volume_of("bb")), (20, 40));
        s.set_output_volume(0.0, false);
        s.set_output_volume(0.8, true);
        assert_eq!((s.airplay.prefs.volume_of("aa"), s.airplay.prefs.volume_of("bb")), (40, 80));
        assert!(s.output_drag.is_none(), "the next drag starts from where they are");
        // Living Room turned down in the picker: the next drag starts from the new balance.
        s.airplay_speaker_volume("bb", 20, true);
        s.set_output_volume(0.2, true);
        assert_eq!((s.airplay.prefs.volume_of("aa"), s.airplay.prefs.volume_of("bb")), (20, 10));
    }

    #[test]
    fn on_a_chromecast_it_is_the_devices_volume() {
        let mut s = State::offline();
        s.playback.volume = 0.5;
        s.cast.state = CastState::Connecting { name: "Smart TV Pro".into() };
        let o = s.output_volume();
        assert_eq!((o.output, o.label.as_str(), o.volume), (Output::Cast, "Smart TV Pro", None));
        s.set_output_volume(0.9, true);
        assert_eq!(s.cast.volume, None, "nothing to set before the device says its volume");
        s.cast.state = CastState::Playing { name: "Smart TV Pro".into(), latency_ms: None };
        s.cast.volume = Some(26);
        assert_eq!(s.output_volume().volume, Some(0.26));
        s.set_output_volume(0.55, true);
        assert_eq!(s.cast.volume, Some(55), "the picker's slider under the TV");
        assert_eq!(s.playback.volume, 0.5, "the app's volume is left alone");
    }

    #[test]
    fn the_output_follows_changes_made_elsewhere() {
        let mut s = State::offline();
        s.set_volume(0.3); // the window, `uzu vol`, a media key
        assert_eq!(s.output_volume().volume, Some(0.3));
        group(&mut s, &[("aa", "Bedroom")]);
        s.airplay_speaker_volume("aa", 70, true); // the picker
        assert_eq!(s.output_volume().volume, Some(0.7));
        s.airplay_pick(None); // back to this Mac
        let o = s.output_volume();
        assert_eq!((o.output, o.volume), (Output::ThisComputer, Some(0.3)));
    }

    #[test]
    fn a_drag_sends_at_most_one_value_per_interval_and_never_loses_the_last() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut t = Throttle::default();
        assert_eq!(t.push(0.10, ms(0), false), Some(0.10), "the first at once");
        assert_eq!(t.push(0.12, ms(16), false), None);
        assert_eq!(t.push(0.14, ms(32), false), None);
        assert!(t.pending());
        assert_eq!(t.flush(ms(50), false), None, "not yet");
        assert_eq!(t.flush(ms(100), false), Some(0.14), "the latest one");
        assert_eq!(t.flush(ms(300), false), None, "nothing left");
        assert_eq!(t.push(0.20, ms(150), false), None);
        assert_eq!(t.push(0.22, ms(160), true), Some(0.22), "the mouse up goes at once");
        assert!(!t.pending(), "and replaces what waited");
        assert_eq!(t.push(0.30, ms(170), false), None);
        assert_eq!(t.flush(ms(171), true), Some(0.30), "the menu closed");
        // 60 moves a second for a second: about ten sent.
        let mut t = Throttle::default();
        let sent = (0..60u32).filter(|i| t.push(f64::from(*i) / 60.0, ms(1000 + u64::from(*i) * 16), false).is_some()).count();
        assert!((9..=11).contains(&sent), "{sent}");
    }
}
