//! AirPlay: where the audio goes (this computer, or AirPlay speakers: one or several playing in
//! step), as the app sees it.
//!
//! `crate::airplay` holds the controller; its events arrive as `Msg::AirPlay` and land in
//! `State::airplay` here. The picker (`ui/airplay.rs`), `uzu airplay` and the reconnect at
//! launch all go through the methods below. In headless runs and tests there is no controller:
//! the methods only change state, and scenes fill in made-up speakers.

use std::time::{Duration, Instant};

use crate::clock;

use uzumaki_airplay::discover::Output;
use uzumaki_airplay::{Cast, Connect, Event, Need, Speaker};

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
    /// The group as a whole (the first speaker that plays, else the first connecting).
    pub cast: Cast,
    /// Every speaker the audio goes to (empty: this computer).
    pub speakers: Vec<Speaker>,
    /// The bridge must capture (a speaker is connected). Sent again to a bridge that starts later.
    pub capture: bool,
    pub ask: Option<Ask>,
    /// Somebody is looking for speakers (the picker is open, or `uzu airplay` asked).
    pub browsing: bool,
    /// `uzu airplay` is looking: the picker closing doesn't end the search.
    pub ctl_browsing: bool,
    /// Since when speakers are looked for (the picker's "none found" hint comes after a while).
    pub browsing_since: Option<Instant>,
    /// The helper program is installed. False: no AirPlay button, no speaker search, `uzu airplay` says so.
    pub available: bool,
    /// The bridge's last word on its capture (`Event::AirPlayCapture`): it taps MusicKit's player.
    /// Until it does, the controller keeps the speaker "Connecting".
    pub capture_ready: bool,
    pub prefs: Prefs,
    /// How late the speaker plays, from the last start (for a later lyrics and progress offset).
    pub latency_ms: Option<u32>,
    /// Back to this computer: where the listener was, for the bridge's player to go back to when
    /// its own output comes back (`Event::Capture(false)`).
    pub rewind: Option<Rewind>,
    /// Tests: how many flushes were asked for (there is no controller to count them).
    #[cfg(test)]
    pub flushes: std::cell::Cell<u32>,
}

/// Leaving the speakers: the spot in the song the listener had reached. The player is ahead of
/// it by the speakers' delay (the audio on its way and queued in the receiver).
#[derive(Clone, Debug, PartialEq)]
pub struct Rewind {
    /// Seconds into the song.
    pub to: f64,
    /// The song it is a spot in (None: no id).
    pub song: Option<String>,
    pub at: Instant,
}

/// A step back shorter than this isn't worth a seek (a seek has its own hiccup).
const MIN_REWIND: Duration = Duration::from_millis(40);
/// A rewind not used by then is stale (the bridge's output came back long ago, or never will).
const REWIND_TTL: Duration = Duration::from_secs(5);

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

    /// The speaker `id` is in the group (playing or connecting).
    pub fn has(&self, id: &str) -> bool {
        self.speakers.iter().any(|s| s.id == id) || self.cast.device().is_some_and(|(cur, _)| cur == id)
    }

    /// The speakers' ids, in the group's order (the summary's alone while no list came yet).
    pub fn ids(&self) -> Vec<String> {
        if self.speakers.is_empty() { self.cast.device().map(|(id, _)| vec![id.to_string()]).unwrap_or_default() } else { self.speakers.iter().map(|s| s.id.clone()).collect() }
    }

    /// The master volume of a group: the loudest speaker's.
    pub fn master(&self) -> u8 {
        self.ids().iter().map(|id| self.prefs.volume_of(id)).max().unwrap_or_else(|| self.volume())
    }

    /// "Bedroom", "Bedroom and Kitchen": where the audio goes.
    pub fn names(&self) -> String {
        if self.speakers.is_empty() {
            return self.cast.device().map(|(_, n)| n.to_string()).unwrap_or_default();
        }
        uzumaki_airplay::names_text(&self.speakers.iter().map(|s| s.name.clone()).collect::<Vec<_>>())
    }
}

impl State {
    /// One controller event. Returns whether the view changed.
    pub(super) fn airplay_event(&mut self, e: Event) -> bool {
        match e {
            Event::Devices(list) => self.airplay.devices = list,
            Event::State(cast) => {
                if let Cast::On { latency_ms, .. } = &cast {
                    self.airplay.latency_ms = *latency_ms;
                }
                if cast == Cast::Off {
                    self.airplay.latency_ms = None;
                    self.airplay.speakers.clear();
                    self.airplay.rewind = None; // the capture's end (just before) used it, or nothing will
                }
                let on = matches!(cast, Cast::On { .. });
                self.airplay.cast = cast;
                if on && self.airplay.speakers.iter().all(|s| s.playing) {
                    self.airplay_remember();
                }
            }
            Event::Speakers(list) => {
                let playing = !list.is_empty() && list.iter().all(|s| s.playing);
                self.airplay.speakers = list;
                if playing {
                    self.airplay_remember();
                }
            }
            Event::Capture(on) => {
                if !on {
                    // Before the bridge's output comes back: it then plays on from the heard spot.
                    self.airplay_rewind_bridge();
                }
                self.airplay.capture = on;
                self.airplay_capture();
                return false;
            }
            Event::Failed { name, why, still } => {
                if self.airplay.ask.as_ref().is_some_and(|a| a.name == name) || still.is_empty() {
                    self.airplay.ask = None;
                }
                self.toast = Some(failure_toast(&why.message(&name), &still));
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

    /// The speakers that all play now are the ones to reconnect to next time.
    fn airplay_remember(&mut self) {
        self.airplay.ask = None;
        let group: Vec<Last> = if self.airplay.speakers.is_empty() {
            self.airplay.cast.device().map(|(id, name)| Last { id: id.into(), name: name.into() }).into_iter().collect()
        } else {
            self.airplay.speakers.iter().map(|s| Last { id: s.id.clone(), name: s.name.clone() }).collect()
        };
        let last = group.first().cloned();
        let group = if group.len() > 1 { group } else { Vec::new() };
        if last.is_some() && (self.airplay.prefs.last != last || self.airplay.prefs.group != group) {
            self.airplay.prefs.last = last;
            self.airplay.prefs.group = group;
            self.airplay_save();
        }
    }

    fn airplay_save(&self) {
        if self.persist_settings {
            self.airplay.prefs.save();
        }
    }

    /// Tells the bridge whether to capture (when there is one; a later one is told at `Ready`).
    pub(super) fn airplay_capture(&mut self) {
        self.airplay.capture_ready = false; // the (new) capture says when it is ready
        self.airplay_bridge_changed();
        if let Some(b) = &self.bridge {
            b.airplay(self.airplay.capture);
        }
    }

    /// The bridge started or stopped: without one (local only mode) there is nothing to capture,
    /// so the controller doesn't wait for the bridge's word.
    pub(super) fn airplay_bridge_changed(&self) {
        if let Some(c) = crate::airplay::controller() {
            c.set_bridge(self.bridge.is_some());
        }
    }

    /// The bridge's capture is ready (or not): the speaker shows as playing only while it is.
    pub(super) fn airplay_capture_ready(&mut self, ready: bool, error: Option<&str>) {
        self.airplay.capture_ready = ready;
        if let Some(c) = crate::airplay::controller() {
            c.capture_ready(ready, error);
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
        self.airplay.browsing_since = on.then(clock::now);
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
        Connect { volume: self.airplay.prefs.volume_of(id), password: None, auth: self.airplay.prefs.pairings.get(id).cloned() }
    }

    /// The picker's choice: a speaker's id alone, or None for this computer.
    pub fn airplay_pick(&mut self, id: Option<&str>) {
        self.airplay.ask = None;
        let Some(id) = id else {
            // "This computer" is a choice too: nothing to reconnect to next time.
            if self.airplay.prefs.last.take().is_some() | !std::mem::take(&mut self.airplay.prefs.group).is_empty() {
                self.airplay_save();
            }
            let controller = crate::airplay::controller();
            let behind = controller.as_ref().and_then(|c| c.behind());
            if let Some(c) = controller {
                c.disconnect();
            }
            self.airplay_leave(behind);
            if crate::airplay::controller().is_none() {
                self.airplay.cast = Cast::Off;
                self.airplay.speakers.clear();
            }
            return;
        };
        self.cast_yield(); // one output at a time
        if self.airplay.ids() == [id] {
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
            None => {
                self.airplay.speakers = vec![Speaker { id: out.id.clone(), name: out.name.clone(), playing: false }];
                self.airplay.cast = Cast::Connecting { id: out.id, name: out.name };
            }
        }
    }

    /// Back to this computer from speakers `behind` the player: the music goes on from where the
    /// listener was, not from where the player got to. The local player steps back at once (its
    /// output comes back as soon as the speakers stop); the bridge's when its capture ends, which
    /// is when its own output comes back (`airplay_rewind_bridge`). Only while a song plays.
    pub(super) fn airplay_leave(&mut self, behind: Option<Duration>) {
        self.airplay.rewind = None;
        let Some(behind) = behind.filter(|b| *b >= MIN_REWIND) else { return };
        if !self.airplay.active() || self.resume.is_some() || self.playback.state != crate::bridge::PlayState::Playing {
            return;
        }
        let to = (self.playback.position_now() - behind.as_secs_f64()).max(0.0);
        eprintln!("airplay: back to this computer {} ms before the player, where the listener was ({to:.2} s)", behind.as_millis());
        if self.playback.local {
            self.seek_player(to);
        } else {
            self.airplay.rewind = Some(Rewind { to, song: self.playback.song.as_ref().map(|s| s.id.clone()), at: clock::now() });
        }
    }

    /// The bridge's capture ends: its player goes back to the spot `airplay_leave` kept, if the
    /// same song is still on.
    fn airplay_rewind_bridge(&mut self) {
        let Some(r) = self.airplay.rewind.take() else { return };
        let song = self.playback.song.as_ref().map(|s| s.id.clone());
        if self.playback.local || clock::since(r.at) > REWIND_TTL || r.song != song {
            return;
        }
        self.seek_player(r.to);
    }

    /// The picker's checkbox: the speaker joins the ones that play, or leaves them (the last one
    /// out: back to this computer).
    pub fn airplay_toggle(&mut self, id: &str) {
        if !self.airplay.has(id) {
            if !self.airplay.active() {
                return self.airplay_pick(Some(id));
            }
            let Some(out) = self.airplay.devices.iter().find(|o| o.id == id).cloned() else { return };
            match crate::airplay::controller() {
                Some(c) => {
                    if !c.add(id, self.airplay_connect(id)) {
                        self.toast = Some(format!("“{}” is no longer on this network.", out.name));
                    }
                }
                None => self.airplay.speakers.push(Speaker { id: out.id, name: out.name, playing: false }),
            }
            return;
        }
        if self.airplay.ids().len() <= 1 {
            return self.airplay_pick(None);
        }
        if self.airplay.ask.as_ref().is_some_and(|a| a.id == id) {
            self.airplay.ask = None;
        }
        match crate::airplay::controller() {
            Some(c) => c.remove(id),
            None => {
                self.airplay.speakers.retain(|s| s.id != id);
                if let Some(s) = self.airplay.speakers.first() {
                    self.airplay.cast = Cast::Connecting { id: s.id.clone(), name: s.name.clone() };
                }
            }
        }
    }

    /// `uzu airplay <name>` and the reconnect at launch: the speaker of that name.
    pub fn airplay_pick_named(&mut self, name: &str) -> Result<(), String> {
        self.airplay_pick_names(&[name.to_string()])
    }

    /// `uzu airplay <name> + <name>…` and the reconnect at launch: these speakers, together.
    pub fn airplay_pick_names(&mut self, names: &[String]) -> Result<(), String> {
        if !self.airplay.available {
            return Err(AIRPLAY_UNAVAILABLE.into());
        }
        let Some(c) = crate::airplay::controller() else { return Err("AirPlay isn't running".into()) };
        self.airplay.ask = None;
        let mut known: Vec<Last> = self.airplay.prefs.reconnect_to();
        known.extend(self.airplay.prefs.last.clone());
        for (i, name) in names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()).enumerate() {
            let id = self.airplay.devices.iter().find(|o| o.name.eq_ignore_ascii_case(name)).map(|o| o.id.clone());
            let id = id.or_else(|| known.iter().find(|l| l.name.eq_ignore_ascii_case(name)).map(|l| l.id.clone()));
            let opts = match &id {
                Some(id) => self.airplay_connect(id),
                None => Connect { volume: self.airplay.volume(), password: None, auth: None },
            };
            if i == 0 {
                c.connect_named(name, opts);
            } else {
                c.add_named(name, opts);
            }
        }
        Ok(())
    }

    /// The volume of one speaker, 0-100 (the app's volume slider stays the player's).
    pub fn airplay_speaker_volume(&mut self, id: &str, volume: u8, done: bool) {
        let v = volume.min(100);
        if self.airplay.prefs.volumes.get(id) != Some(&v) {
            self.airplay.prefs.volumes.insert(id.to_string(), v);
            // The next speaker without a volume of its own starts here.
            self.airplay.prefs.volume = Some(v);
            if let Some(c) = crate::airplay::controller() {
                c.set_speaker_volume(id, v);
            }
        }
        if done {
            self.airplay_save();
        }
    }

    /// The master volume: one speaker's own; with several, each moves in proportion (`from`: their
    /// volumes when the drag began, so a trip to 0 and back keeps the balance).
    pub fn airplay_master_volume(&mut self, volume: u8, from: &[(String, u8)], done: bool) {
        let v = volume.min(100);
        let top = from.iter().map(|(_, x)| *x).max().unwrap_or(0);
        for (id, was) in from {
            let to = if top == 0 { v } else { ((u32::from(*was) * u32::from(v) + u32::from(top) / 2) / u32::from(top)).min(100) as u8 };
            self.airplay_speaker_volume(id, to, false);
        }
        self.airplay.prefs.volume = Some(v);
        if done {
            self.airplay_save();
        }
    }

    /// The speakers' volumes now, for `airplay_master_volume`.
    pub fn airplay_volumes(&self) -> Vec<(String, u8)> {
        self.airplay.ids().into_iter().map(|id| {
            let v = self.airplay.prefs.volume_of(&id);
            (id, v)
        }).collect()
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
            if !self.airplay.speakers.iter().any(|s| s.id == a.id) {
                self.airplay.speakers.push(Speaker { id: a.id.clone(), name: a.name.clone(), playing: false });
            }
            if !self.airplay.active() {
                self.airplay.cast = Cast::Connecting { id: a.id, name: a.name };
            }
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
        if !self.song_ended() {
            self.airplay_flush();
        }
    }

    /// The playing song reached its end by itself (or is within its last 1.5 s).
    pub(super) fn song_ended(&self) -> bool {
        let pb = &self.playback;
        let dur = if pb.duration > 0.0 { pb.duration } else { pb.song.as_ref().map_or(0.0, |s| s.duration) };
        matches!(pb.state, crate::bridge::PlayState::Ended | crate::bridge::PlayState::Completed) || (dur > 0.0 && pb.position_now() >= dur - 1.5)
    }

    pub fn set_airplay_reconnect(&mut self, on: bool) {
        self.airplay.prefs.reconnect = on;
        self.airplay_save();
    }

    /// At launch: back to the speakers of the last run, if the setting asks for it.
    pub fn airplay_reconnect(&mut self) {
        if !self.airplay.prefs.reconnect || !self.airplay.available {
            return;
        }
        let names: Vec<String> = self.airplay.prefs.reconnect_to().into_iter().map(|l| l.name).collect();
        if !names.is_empty() {
            eprintln!("airplay: reconnecting to {} (Settings → Speakers)", uzumaki_airplay::names_text(&names));
            let _ = self.airplay_pick_names(&names);
        }
    }
}

/// A speaker stopped: why, and where the audio goes now.
fn failure_toast(message: &str, still: &[String]) -> String {
    if still.is_empty() {
        format!("{message} Playing on {}.", crate::ui::this_computer())
    } else {
        format!("{message} Still playing on {}.", uzumaki_airplay::names_text(still))
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
        assert!(s.airplay_event(Event::Failed { name: "Bedroom".into(), why: Why::Lost, still: vec![] }));
        assert!(s.airplay_event(Event::State(Cast::Off)));
        let toast = s.toast.clone().unwrap();
        assert!(toast.contains("Bedroom") && toast.contains("was lost") && toast.contains(crate::ui::this_computer()), "{toast}");
        assert!(!s.airplay.active() && s.airplay.latency_ms.is_none());
        // The device is still remembered: a lost connection isn't the user's choice.
        assert!(s.airplay.prefs.last.is_some());
        assert!(s.airplay.speakers.is_empty());
        s.airplay_pick(None);
        assert!(s.airplay.prefs.last.is_none(), "choosing this computer is");
    }

    fn group(s: &mut State, names: &[(&str, &str)], playing: bool) {
        let list: Vec<Speaker> = names.iter().map(|(id, name)| Speaker { id: id.to_string(), name: name.to_string(), playing }).collect();
        s.airplay_event(Event::Speakers(list));
        let (id, name) = names[0];
        let cast = if playing { Cast::On { id: id.into(), name: name.into(), latency_ms: Some(400) } } else { Cast::Connecting { id: id.into(), name: name.into() } };
        s.airplay_event(Event::State(cast));
    }

    #[test]
    fn one_speaker_failing_says_the_others_play_on() {
        let mut s = State::offline();
        group(&mut s, &[("aa", "Bedroom"), ("bb", "Kitchen"), ("cc", "Office")], true);
        assert_eq!(s.airplay.names(), "Bedroom, Kitchen and Office");
        s.airplay_event(Event::Failed { name: "Kitchen".into(), why: Why::Busy, still: vec!["Bedroom".into(), "Office".into()] });
        let toast = s.toast.clone().unwrap();
        assert_eq!(toast, "“Kitchen” is busy with another device. Still playing on Bedroom and Office.");
    }

    #[test]
    fn the_group_is_remembered_for_the_next_launch() {
        let mut s = State::offline();
        group(&mut s, &[("aa", "Bedroom"), ("bb", "Kitchen")], false);
        assert!(s.airplay.prefs.last.is_none(), "not while connecting");
        group(&mut s, &[("aa", "Bedroom"), ("bb", "Kitchen")], true);
        assert_eq!(s.airplay.prefs.last.as_ref().map(|l| l.name.as_str()), Some("Bedroom"));
        assert_eq!(s.airplay.prefs.reconnect_to().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Bedroom", "Kitchen"]);
        // Down to one: that one alone.
        group(&mut s, &[("bb", "Kitchen")], true);
        assert!(s.airplay.prefs.group.is_empty());
        assert_eq!(s.airplay.prefs.last.as_ref().map(|l| l.id.as_str()), Some("bb"));
        s.airplay_pick(None);
        assert!(s.airplay.prefs.last.is_none() && s.airplay.prefs.group.is_empty());
    }

    #[test]
    fn the_master_volume_moves_every_speaker_in_proportion() {
        let mut s = State::offline();
        group(&mut s, &[("aa", "Bedroom"), ("bb", "Kitchen")], true);
        s.airplay_speaker_volume("aa", 40, true);
        s.airplay_speaker_volume("bb", 80, true);
        assert_eq!(s.airplay.master(), 80, "the loudest");
        let from = s.airplay_volumes();
        s.airplay_master_volume(40, &from, false);
        assert_eq!((s.airplay.prefs.volume_of("aa"), s.airplay.prefs.volume_of("bb")), (20, 40));
        // Down to nothing and back up in the same drag: the balance is kept.
        s.airplay_master_volume(0, &from, false);
        assert_eq!((s.airplay.prefs.volume_of("aa"), s.airplay.prefs.volume_of("bb")), (0, 0));
        s.airplay_master_volume(80, &from, true);
        assert_eq!((s.airplay.prefs.volume_of("aa"), s.airplay.prefs.volume_of("bb")), (40, 80));
        // One speaker on its own.
        s.airplay_speaker_volume("bb", 10, true);
        assert_eq!((s.airplay.prefs.volume_of("aa"), s.airplay.prefs.volume_of("bb")), (40, 10));
        assert_eq!(s.airplay_connect("bb").volume, 10, "a speaker connects at its own volume");
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
    fn the_bridge_says_when_its_capture_is_ready_and_each_capture_starts_unready() {
        let mut s = State::offline();
        s.airplay_event(Event::Capture(true));
        assert!(!s.airplay.capture_ready);
        assert!(!s.apply_msg(crate::state::Msg::Bridge(crate::bridge::Event::AirPlayCapture { ready: true, error: None })), "nothing to draw");
        assert!(s.airplay.capture_ready);
        s.airplay_event(Event::Capture(false));
        s.airplay_event(Event::Capture(true));
        assert!(!s.airplay.capture_ready, "the next session waits for its own word");
    }

    /// A song plays at 42 s on a speaker 850 ms behind the player.
    fn casting_at_42(s: &mut State, local: bool) {
        on(s);
        playing(s, 42.0);
        s.playback.song = Some(crate::state::tests::song("x"));
        s.playback.local = local;
    }

    const BEHIND: Option<Duration> = Some(Duration::from_millis(850));

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn back_to_this_computer_a_local_file_goes_on_from_where_the_listener_was() {
        let mut s = State::offline();
        casting_at_42(&mut s, true);
        s.airplay_leave(BEHIND);
        // At once: the local player's output comes back as soon as the speakers stop.
        assert!(near(s.playback.position, 41.15), "{}", s.playback.position);
        assert_eq!(s.local_sent.borrow().last().map(String::as_str), Some("seek 41.15"));
        assert_eq!(s.airplay.flushes.get(), 0, "the speakers are leaving: nothing to flush");
        assert!(s.airplay.rewind.is_none());
    }

    #[test]
    fn back_to_this_computer_an_apple_song_steps_back_when_the_bridge_plays_here_again() {
        let mut s = State::offline();
        casting_at_42(&mut s, false);
        s.airplay_leave(BEHIND);
        assert!(near(s.playback.position, 42.0), "not yet: the bridge's output is still muted");
        assert!(s.airplay.rewind.as_ref().is_some_and(|r| near(r.to, 41.15)));
        // The capture ends (the bridge's output comes back): the player goes back first.
        s.airplay_event(Event::Capture(false));
        assert!(near(s.playback.position, 41.15), "{}", s.playback.position);
        assert!(s.playback.seek_target.is_some_and(|(t, _)| near(t, 41.15)));
        assert!(s.airplay.rewind.is_none());
        s.airplay_event(Event::State(Cast::Off));
        assert!(near(s.playback.position, 41.15));
    }

    #[test]
    fn no_step_back_when_paused_without_a_delay_or_after_the_song_changed() {
        // Paused: the player isn't ahead of anything.
        let mut s = State::offline();
        casting_at_42(&mut s, true);
        s.playback.state = PlayState::Paused;
        s.airplay_leave(BEHIND);
        assert!(near(s.playback.position, 42.0) && s.local_sent.borrow().is_empty());
        // No speaker had started, or a step too small to be worth a seek.
        for behind in [None, Some(Duration::from_millis(10))] {
            let mut s = State::offline();
            casting_at_42(&mut s, false);
            s.airplay_leave(behind);
            assert!(s.airplay.rewind.is_none(), "{behind:?}");
        }
        // Nothing was casting.
        let mut s = State::offline();
        playing(&mut s, 42.0);
        s.airplay_leave(BEHIND);
        assert!(near(s.playback.position, 42.0) && s.airplay.rewind.is_none());
        // Another song started before the bridge's output came back.
        let mut s = State::offline();
        casting_at_42(&mut s, false);
        s.airplay_leave(BEHIND);
        s.playback.song = Some(crate::state::tests::song("y"));
        s.playback.position = 0.5;
        s.airplay_event(Event::Capture(false));
        assert!(near(s.playback.position, 0.5), "the other song stays where it is");
    }

    #[test]
    fn a_step_back_kept_too_long_is_dropped() {
        clock::freeze();
        let mut s = State::offline();
        casting_at_42(&mut s, false);
        s.playback.position_at = Some(clock::now());
        s.airplay_leave(BEHIND);
        clock::advance(REWIND_TTL + Duration::from_millis(1));
        let before = s.playback.position_now();
        s.airplay_event(Event::Capture(false));
        assert!(near(s.playback.position_now(), before), "stale: no seek");
    }

    #[test]
    fn picking_this_computer_without_a_controller_changes_only_the_state() {
        let mut s = State::offline();
        casting_at_42(&mut s, false);
        s.airplay_pick(None);
        assert!(!s.airplay.active() && s.airplay.rewind.is_none());
        assert!(near(s.playback.position, 42.0), "no delay known: no step back");
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
