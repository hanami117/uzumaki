//! Resume where you left off, the state side (`crate::resume` keeps the file and writes it).
//!
//! At start the last run's song comes back paused at its position, with its queue and History
//! (`restore_playback`). Nothing is handed to a player then: a local file only opens the sound
//! card when Play is pressed; an Apple song is given to MusicKit paused (setQueue without play,
//! at the position) once the bridge is ready and signed in (`prime_resume`), so Play starts
//! at once. Until Play, the bridge's playback reports are ignored (MusicKit would say 0:00).
//! Play, Next, Previous or any new song end that state.

use std::hash::{Hash, Hasher};
use std::time::Instant;

use super::queue::History;
use super::{Msg, PlayQueue, State};
use crate::bridge::PlayState;
use crate::resume::{self, Snapshot};

/// The restored song hasn't been handed to a player yet.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Pending {
    /// MusicKit holds the song, paused at this position (`prime_resume`): Play only resumes it.
    pub primed: Option<f64>,
}

/// What `resume.rs` was last given.
#[derive(Debug, Default)]
pub struct Published {
    /// The restore ran (or had nothing to do): from then on the file follows the app.
    pub ready: bool,
    sig: u64,
    at: Option<Instant>,
}

impl State {
    /// The playback to save now, or None (the setting is off, or no song).
    pub fn resume_snapshot(&self) -> Option<Snapshot> {
        if !self.settings.resume_playback {
            return None;
        }
        let song = self.playback.song.clone()?;
        let q = &self.queue;
        let history = self.history.tracks.iter().map(|(_, s)| s.clone()).collect();
        let position = self.playback.position_now();
        let user = q.user.iter().cloned().collect();
        if q.station.is_some() || q.context.is_empty() {
            // A station can't be picked up mid-way: its song on its own, then what was queued.
            let one = vec![song.clone()];
            return Some(Snapshot::new(song, position, &one, &one, 0, user, false, q.source.clone(), false, history));
        }
        let pos = q.pos.min(q.context.len() - 1);
        let user_playing = q.user_playing.is_some() || !q.context[pos].same_track(&song);
        Some(Snapshot::new(song, position, &q.original, &q.context, pos, user, user_playing, q.source.clone(), q.shuffled, history))
    }

    /// Shows a saved playback (already `prepare`d), paused. Not when a song was started
    /// meanwhile or the setting is off. Returns whether it was taken.
    pub fn restore_playback(&mut self, snap: Snapshot) -> bool {
        if self.playback.song.is_some() || !self.settings.resume_playback {
            return false;
        }
        let song = snap.song;
        let mut queue = PlayQueue {
            original: snap.original,
            context: snap.context,
            pos: snap.pos,
            user: snap.user.into(),
            source: snap.source,
            shuffled: snap.shuffled,
            ..PlayQueue::default()
        };
        queue.pos = queue.pos.min(queue.context.len().saturating_sub(1));
        queue.base = queue.pos;
        if snap.user_playing || queue.context.is_empty() {
            queue.user_playing = Some(song.clone());
        }
        self.queue = queue;
        self.queue.plan_lap(self.playback.repeat);
        self.history = History::restore(snap.history);
        self.playback.duration = song.duration.max(0.0);
        self.playback.position = snap.position;
        self.playback.position_at = Some(crate::clock::now());
        self.playback.slew = 0.0;
        self.playback.seek_target = None;
        self.playback.state = PlayState::Paused;
        self.playback.local = song.is_local();
        self.playback.song = Some(song.clone());
        self.resume = Some(Pending::default());
        if let Some(t) = &mut self.tray {
            t.set_song(Some(&song));
            t.set_playing(false);
        }
        self.want_lyrics(&song);
        self.preload_art();
        self.prime_resume();
        true
    }

    /// Gives the restored Apple song to MusicKit, paused at its position, when the bridge is
    /// ready and signed in. The answer comes back as `ResumePrimed` (or `QueueFailed`).
    pub(super) fn prime_resume(&mut self) {
        let Some(p) = &self.resume else { return };
        if p.primed.is_some() || !self.session.bridge_up || !self.session.authorized || self.local_only() {
            return;
        }
        let Some(song) = self.playback.song.clone().filter(|s| !s.is_local()) else { return };
        let at = self.playback.position;
        let first = song.play_id().to_string();
        #[cfg(test)]
        self.sent.borrow_mut().push(format!("prime {first} at {at}"));
        let Some(b) = &self.bridge else { return };
        let mut ids = vec![first.clone()];
        ids.extend(self.queue.upcoming_ids());
        b.prime_queue(&ids, self.bridge_cycle().as_deref(), at, move |r| {
            Some(match r {
                Ok(()) => Msg::ResumePrimed { first, at },
                Err(error) => Msg::QueueFailed { first, error },
            })
        });
    }

    /// MusicKit holds the restored song (`prime_resume`).
    pub(super) fn resume_primed(&mut self, first: &str, at: f64) {
        let ours = self.playback.song.as_ref().is_some_and(|s| s.play_id() == first);
        if let (Some(p), true) = (&mut self.resume, ours) {
            p.primed = Some(at);
        }
    }

    /// The bridge (re)started: whatever MusicKit held is gone.
    pub(super) fn resume_bridge_ready(&mut self) {
        if let Some(p) = &mut self.resume {
            p.primed = None;
        }
        self.prime_resume();
    }

    /// Play on the restored song: from where it was left.
    pub(super) fn play_resumed(&mut self) {
        let Some(p) = self.resume.take() else { return };
        let Some(song) = self.playback.song.clone() else { return };
        let at = self.playback.position;
        if p.primed.is_some() && !song.is_local() && (self.bridge.is_some() || cfg!(test)) {
            #[cfg(test)]
            self.sent.borrow_mut().push(format!("play at {at}"));
            if let Some(b) = &self.bridge {
                b.play_at(at);
            }
            self.starting = Some(crate::clock::now());
            self.playback.set_state(PlayState::Loading);
            self.playback.seek_target = Some((at, crate::clock::now()));
        } else {
            self.begin_current_at(song, at);
        }
    }

    /// A seek before Play: only the position changes (Play starts there).
    pub(super) fn seek_resumed(&mut self, seconds: f64) {
        self.playback.seek_preview = None;
        self.playback.position = seconds.clamp(0.0, self.playback.duration.max(0.0));
        self.playback.position_at = Some(crate::clock::now());
        self.resume_changed(true);
    }

    /// Settings → Playback → Resume where you left off. Off: the saved playback goes.
    pub fn set_resume_playback(&mut self, on: bool) {
        if self.settings.resume_playback == on {
            return;
        }
        self.settings.resume_playback = on;
        self.save_settings();
        self.resume_changed(true);
    }

    /// The real app at start: arms saving and reads the last run's playback off the UI thread.
    /// A test instance (`UZUMAKI_TEST_INSTANCE`) stays out of it unless it has its own
    /// `XDG_STATE_HOME`.
    pub fn start_resume(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        let var = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        if var("UZUMAKI_TEST_INSTANCE") && !var("XDG_STATE_HOME") {
            return;
        }
        let Some(path) = resume::path() else { return };
        let quit = handle.clone();
        resume::arm(path.clone(), move || quit.quit());
        if !self.settings.resume_playback {
            self.resume_out.ready = true;
            self.resume_changed(true); // drops a file left from when it was on
            return;
        }
        let local_only = self.local_only();
        std::thread::Builder::new()
            .name("resume-load".into())
            .spawn(move || {
                let snap = resume::load(&path).and_then(|s| s.prepare(local_only, |p| p.is_file()));
                handle.update(move |s: &mut State, tx| {
                    s.resume_out.ready = true;
                    if let Some(snap) = snap.filter(|_| s.local_only() == local_only) {
                        s.restore_playback(snap);
                    }
                    s.resume_changed(true);
                    s.landing(tx);
                });
            })
            .ok();
    }

    /// Hands the playback to `resume.rs` when it changed (song, play state, queue), every
    /// `SAVE_EVERY` while playing, or when `force`d; otherwise only the position. Only in the
    /// real app, and only after the restore ran.
    pub(super) fn resume_changed(&mut self, force: bool) {
        if self.handle.is_none() || !self.resume_out.ready {
            return;
        }
        let p = &self.playback;
        let q = &self.queue;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        p.song.as_ref().map(|s| &s.id).hash(&mut h);
        p.state.is_active().hash(&mut h);
        (q.pos, q.context.len(), q.user.len(), q.user_playing.is_some(), q.shuffled).hash(&mut h);
        q.up_next().take(32).for_each(|s| s.id.hash(&mut h));
        self.settings.resume_playback.hash(&mut h);
        let sig = h.finish();
        let due = p.state == PlayState::Playing && self.resume_out.at.is_none_or(|t| crate::clock::since(t) >= resume::SAVE_EVERY);
        if force || due || sig != self.resume_out.sig {
            self.resume_out.sig = sig;
            self.resume_out.at = Some(crate::clock::now());
            resume::publish(self.resume_snapshot());
        } else if p.song.is_some() {
            resume::note_position(p.position_now());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::bridge::Event;
    use crate::models::Song;
    use crate::settings::MusicSource;

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: format!("Song {id}"),
            artist: "A".into(),
            album: "B".into(),
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

    /// A state playing `ids[at]` 61.5 s in, with Up Next and a History, paused there.
    fn playing(ids: &[&str], at: usize, user: &[&str]) -> State {
        let mut s = State::offline();
        let list: Vec<Song> = ids.iter().map(|i| song(i)).collect();
        s.play_list(&list, at, "Mix");
        for u in user {
            s.add_to_queue(&[song(u)]);
        }
        s.history.heard(&song("h"));
        s.history.heard(&list[at]);
        s.playback.state = PlayState::Paused;
        s.playback.position = 61.5;
        s
    }

    /// Saves `from` through the file and restores it into a fresh state.
    fn round_trip(from: &State, local_only: bool, exists: impl Fn(&Path) -> bool) -> State {
        let snap = from.resume_snapshot().expect("something to save");
        let dir = std::env::temp_dir().join(format!("uzumaki-resume-state-{}-{:?}", std::process::id(), std::thread::current().id()));
        let path = dir.join("resume.json");
        crate::resume::save(&path, &snap).unwrap();
        let loaded = crate::resume::load(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = State::offline();
        if local_only {
            s.settings.music_source = MusicSource::LocalOnly;
        }
        if let Some(snap) = loaded.prepare(local_only, exists) {
            assert!(s.restore_playback(snap));
        }
        s
    }

    fn up_next(s: &State) -> Vec<&str> {
        s.queue.up_next().map(|x| x.id.as_str()).collect()
    }

    #[test]
    fn save_and_restore_brings_back_song_position_queue_and_history() {
        let from = playing(&["a", "b", "c", "d"], 1, &["u1", "u2"]);
        let s = round_trip(&from, false, |_| true);
        assert_eq!(s.playback.song.as_ref().unwrap().id, "b");
        assert_eq!(s.playback.position, 61.5);
        assert_eq!(s.playback.duration, 200.0);
        assert_eq!(s.playback.state, PlayState::Paused);
        assert_eq!(up_next(&s), ["u1", "u2", "c", "d"]);
        assert_eq!(s.queue.source, "Mix");
        assert_eq!(s.queue.pos, 1, "Previous still goes to a");
        assert_eq!(s.history.tracks.iter().map(|(_, x)| x.id.as_str()).collect::<Vec<_>>(), ["h"]);
        assert!(s.resume.is_some());
    }

    #[test]
    fn a_restored_song_stays_paused_and_opens_no_player() {
        let from = playing(&["file:///m/x.flac", "file:///m/y.flac"], 0, &[]);
        let mut s = round_trip(&from, false, |_| true);
        assert!(s.local_sent.borrow().is_empty(), "no sound card until Play");
        assert!(s.sent.borrow().is_empty());
        // MusicKit's reports (0:00, stopped) don't move it.
        s.apply_event(Event::Time { current: 0.0, duration: 0.0 });
        s.apply_event(Event::PlayState(PlayState::Stopped));
        assert_eq!((s.playback.state, s.playback.position), (PlayState::Paused, 61.5));
        // Pause does nothing; a seek only moves the place Play starts from.
        s.pause();
        s.seek_seconds(42.0);
        assert!(s.local_sent.borrow().is_empty());
        assert_eq!(s.playback.position, 42.0);
        s.play();
        assert_eq!(s.local_sent.borrow().as_slice(), ["play Song file:///m/x.flac at 42"]);
        assert!(s.resume.is_none());
        assert_eq!(s.playback.position, 42.0);
    }

    #[test]
    fn an_apple_song_is_handed_to_musickit_paused_once_signed_in() {
        let from = playing(&["a", "b"], 0, &[]);
        let mut s = round_trip(&from, false, |_| true);
        assert!(s.sent.borrow().is_empty(), "the bridge isn't up yet");
        s.session.bridge_up = true;
        s.session.authorized = true;
        s.resume_bridge_ready();
        assert_eq!(s.sent.borrow().as_slice(), ["prime a at 61.5"]);
        s.resume_primed("a", 61.5);
        assert_eq!(s.resume.as_ref().unwrap().primed, Some(61.5));
        s.play();
        assert_eq!(s.sent.borrow().last().unwrap(), "play at 61.5");
        assert!(s.resume.is_none());
        assert_eq!(s.playback.seek_target.map(|(t, _)| t), Some(61.5), "early 0:00 reports are ignored");
    }

    /// The bridge's sequence on a live resume: the restored song is primed paused, MusicKit
    /// reports 0:00 meanwhile; Play, then it announces the same song, plays, reports 0:00 again,
    /// then the target. The shown position (seek bar, time labels, lyrics) never drops to 0.
    #[test]
    fn the_shown_position_never_drops_to_zero_while_a_resume_is_handed_over() {
        let mut s = round_trip(&playing(&["a", "b"], 0, &[]), false, |_| true);
        let shown = |s: &State| s.playback.position_now();
        s.session.bridge_up = true;
        s.session.authorized = true;
        s.resume_bridge_ready();
        s.apply_event(Event::NowPlaying(Some(song("a"))));
        s.apply_event(Event::Time { current: 0.0, duration: 200.0 });
        s.resume_primed("a", 61.5);
        s.apply_event(Event::PlayState(PlayState::Paused));
        s.apply_event(Event::Time { current: 0.0, duration: 200.0 });
        assert_eq!((s.playback.state, shown(&s)), (PlayState::Paused, 61.5), "paused at the saved second");
        s.play();
        assert_eq!(s.sent.borrow().last().unwrap(), "play at 61.5");
        assert!(shown(&s) >= 61.5);
        s.apply_event(Event::NowPlaying(Some(song("a"))));
        assert!(shown(&s) >= 61.5, "the same song announced again keeps the position");
        s.apply_event(Event::Time { current: 0.0, duration: 200.0 });
        assert!(shown(&s) >= 61.5);
        s.apply_event(Event::PlayState(PlayState::Playing));
        for t in [0.0, 0.25, 1.0] {
            s.apply_event(Event::Time { current: t, duration: 200.0 });
            assert!(shown(&s) >= 61.5, "a {t} s report before the seek took: {}", shown(&s));
        }
        s.apply_event(Event::Time { current: 61.7, duration: 200.0 });
        assert!(s.playback.seek_target.is_none(), "a report near the target ends the hand-over");
        assert!((shown(&s) - 61.7).abs() < 1.0);
        // From then on reports are believed again: a real seek, then a new song from 0.
        s.seek_seconds(10.0);
        s.apply_event(Event::Time { current: 10.0, duration: 200.0 });
        assert!((shown(&s) - 10.0).abs() < 0.5);
        s.apply_event(Event::NowPlaying(Some(song("b"))));
        assert_eq!(s.playback.song.as_ref().unwrap().id, "b");
        s.apply_event(Event::Time { current: 0.0, duration: 200.0 });
        assert!(shown(&s) < 0.5, "a new song starts at 0");
    }

    /// Without the paused hand-over (it failed), Play starts the song afresh at the position:
    /// the same guard holds there.
    #[test]
    fn a_fresh_start_at_the_position_also_never_shows_zero() {
        let mut s = round_trip(&playing(&["a", "b"], 0, &[]), false, |_| true);
        s.play();
        assert_eq!(s.playback.position_now(), 61.5);
        s.apply_event(Event::NowPlaying(Some(song("a"))));
        s.apply_event(Event::Time { current: 0.0, duration: 200.0 });
        s.apply_event(Event::PlayState(PlayState::Playing));
        s.apply_event(Event::Time { current: 0.0, duration: 200.0 });
        assert!(s.playback.position_now() >= 61.5);
        s.apply_event(Event::Time { current: 61.5, duration: 200.0 });
        assert!(s.playback.seek_target.is_none());
    }

    #[test]
    fn a_failed_hand_over_starts_afresh_at_the_position() {
        let mut s = round_trip(&playing(&["a", "b"], 0, &[]), false, |_| true);
        s.session.bridge_up = true;
        s.session.authorized = true;
        s.resume_bridge_ready();
        s.queue_failed("a", "[mk-007] NOT_FOUND");
        assert!(s.toast.is_none(), "quietly");
        assert_eq!(s.resume.as_ref().unwrap().primed, None);
        s.play();
        assert!(!s.sent.borrow().iter().any(|x| x.starts_with("play at")));
        assert_eq!((s.playback.state, s.playback.position), (PlayState::Loading, 61.5));
        assert!(s.resume.is_none());
    }

    #[test]
    fn next_and_previous_leave_the_restored_song() {
        let mut s = round_trip(&playing(&["a", "b", "c"], 1, &[]), false, |_| true);
        s.next();
        assert_eq!(s.playback.song.as_ref().unwrap().id, "c");
        assert!(s.resume.is_none());
        let mut s = round_trip(&playing(&["a", "b", "c"], 1, &[]), false, |_| true);
        s.seek_seconds(1.0);
        s.previous();
        assert_eq!(s.playback.song.as_ref().unwrap().id, "a");
    }

    #[test]
    fn local_only_restores_only_local_files() {
        let from = playing(&["a", "file:///m/x.flac", "b", "file:///m/y.flac"], 1, &["u", "file:///m/z.flac"]);
        let s = round_trip(&from, true, |_| true);
        assert_eq!(s.playback.song.as_ref().unwrap().id, "file:///m/x.flac");
        assert_eq!(up_next(&s), ["file:///m/z.flac", "file:///m/y.flac"]);
        // An Apple song playing: the next local file instead, from the start.
        let s = round_trip(&playing(&["a", "file:///m/y.flac"], 0, &[]), true, |_| true);
        assert_eq!(s.playback.song.as_ref().unwrap().id, "file:///m/y.flac");
        assert_eq!(s.playback.position, 0.0);
        // Nothing local at all: nothing comes back.
        let s = round_trip(&playing(&["a", "b"], 0, &[]), true, |_| true);
        assert!(s.playback.song.is_none() && s.resume.is_none());
    }

    #[test]
    fn missing_files_are_skipped() {
        let from = playing(&["file:///m/gone.flac", "file:///m/y.flac", "file:///m/gone2.flac"], 0, &[]);
        let s = round_trip(&from, false, |p| !p.to_string_lossy().contains("gone"));
        assert_eq!(s.playback.song.as_ref().unwrap().id, "file:///m/y.flac");
        assert_eq!(s.playback.position, 0.0);
        assert_eq!(up_next(&s), Vec::<&str>::new());
    }

    #[test]
    fn a_station_comes_back_as_its_song() {
        let mut s = State::offline();
        s.queue = PlayQueue::for_station("ra.1", "Radio", None);
        s.playback.song = Some(song("st"));
        let snap = s.resume_snapshot().unwrap();
        assert_eq!((snap.context.len(), snap.song.id.as_str(), snap.source.as_str()), (1, "st", "Radio"));
    }

    #[test]
    fn switched_off_nothing_is_saved_or_restored() {
        let mut from = playing(&["a"], 0, &[]);
        let snap = from.resume_snapshot().unwrap();
        from.settings.resume_playback = false;
        assert!(from.resume_snapshot().is_none());
        let mut s = State::offline();
        s.settings.resume_playback = false;
        assert!(!s.restore_playback(snap.clone()));
        assert!(s.playback.song.is_none());
        // A song started before the file was read wins too.
        let mut s = State::offline();
        s.play_list(&[song("x")], 0, "");
        assert!(!s.restore_playback(snap));
        assert_eq!(s.playback.song.as_ref().unwrap().id, "x");
    }

    #[test]
    fn on_by_default_and_old_settings_files_get_it_on() {
        assert!(crate::settings::Settings::default().resume_playback);
        assert!(crate::settings::Settings::from_json(br#"{"onboarded": 3}"#).resume_playback);
    }
}
