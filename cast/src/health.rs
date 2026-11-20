//! Reads the device's statuses for trouble: when the audio is really heard (the position moves,
//! not just `PLAYING`), underruns (`BUFFERING` again after it played) and stalls (`PLAYING` but
//! the position stands still). Pure: the session feeds it the statuses with their times.

use std::time::{Duration, Instant};

/// `PLAYING` without the position moving for this long is a stall.
const STALL_AFTER: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// The position moved for the first time: the audio is heard.
    Started { after_load_ms: u64 },
    Underrun { served: f64, device: f64 },
    Recovered { after_ms: u64 },
    Stall { secs: f64 },
    StallOver,
}

impl Note {
    pub fn line(&self) -> String {
        match self {
            Note::Started { after_load_ms } => format!("audio heard: the position moves, {after_load_ms} ms after LOAD"),
            Note::Underrun { served, device } => format!("UNDERRUN: the device is buffering again (served {served:.2} s, device at {device:.2} s)"),
            Note::Recovered { after_ms } => format!("underrun over after {after_ms} ms"),
            Note::Stall { secs } => format!("STALL: PLAYING but the position hasn't moved for {secs:.1} s"),
            Note::StallOver => "stall over: the position moves again".into(),
        }
    }
}

pub struct Health {
    load_at: Instant,
    played: bool,
    started: bool,
    under_since: Option<Instant>,
    /// The last `PLAYING` status with a moving position: when, and where.
    moving: Option<(Instant, f64)>,
    stalled: bool,
    pub underruns: u32,
    pub stalls: u32,
    pub load_to_audio_ms: Option<u64>,
}

impl Health {
    pub fn new(load_at: Instant) -> Self {
        Health { load_at, played: false, started: false, under_since: None, moving: None, stalled: false, underruns: 0, stalls: 0, load_to_audio_ms: None }
    }

    /// The position has moved since the LOAD: buffer measurements mean something.
    pub fn audible(&self) -> bool {
        self.started
    }

    pub fn observe(&mut self, now: Instant, state: &str, device: f64, served: f64) -> Vec<Note> {
        let mut notes = Vec::new();
        match state {
            "PLAYING" => {
                self.played = true;
                if let Some(t) = self.under_since.take() {
                    notes.push(Note::Recovered { after_ms: now.duration_since(t).as_millis() as u64 });
                }
                if !self.started && device > 0.05 {
                    self.started = true;
                    let ms = now.duration_since(self.load_at).as_millis() as u64;
                    self.load_to_audio_ms = Some(ms);
                    notes.push(Note::Started { after_load_ms: ms });
                }
                match self.moving {
                    Some((at, pos)) if device <= pos + 0.05 => {
                        let still = now.duration_since(at);
                        if self.started && still >= STALL_AFTER && !self.stalled {
                            self.stalled = true;
                            self.stalls += 1;
                            notes.push(Note::Stall { secs: still.as_secs_f64() });
                        }
                    }
                    _ => {
                        if self.stalled {
                            self.stalled = false;
                            notes.push(Note::StallOver);
                        }
                        self.moving = Some((now, device));
                    }
                }
            }
            "BUFFERING" if self.played && self.under_since.is_none() => {
                self.under_since = Some(now);
                self.underruns += 1;
                self.moving = None;
                notes.push(Note::Underrun { served, device });
            }
            "PAUSED" | "IDLE" => self.moving = None,
            _ => {}
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the TV did in the second live test: PLAYING at 0, then buffering ~6 s, then playing.
    #[test]
    fn the_live_tests_start_reads_as_an_underrun() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut h = Health::new(t0);
        assert!(h.observe(at(1885), "PLAYING", 0.0, 1.9).is_empty(), "PLAYING at 0 is not yet audio");
        assert!(!h.audible());
        assert_eq!(h.observe(at(2400), "BUFFERING", 0.0, 1.95), vec![Note::Underrun { served: 1.95, device: 0.0 }]);
        assert!(h.observe(at(4400), "BUFFERING", 0.0, 3.7).is_empty(), "one underrun, not one per status");
        let back = h.observe(at(8400), "PLAYING", 0.4, 9.9);
        assert_eq!(back, vec![Note::Recovered { after_ms: 6000 }, Note::Started { after_load_ms: 8400 }]);
        assert!(h.audible());
        assert_eq!((h.underruns, h.stalls, h.load_to_audio_ms), (1, 0, Some(8400)));
        assert!(back[0].line().contains("6000 ms") && Note::Underrun { served: 1.0, device: 0.0 }.line().starts_with("UNDERRUN"));
    }

    #[test]
    fn a_position_that_stands_still_is_a_stall() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut h = Health::new(t0);
        h.observe(at(1000), "PLAYING", 0.5, 2.5);
        assert!(h.observe(at(2000), "PLAYING", 1.5, 3.5).is_empty());
        assert!(h.observe(at(3000), "PLAYING", 1.5, 4.5).is_empty(), "1 s still: not yet");
        assert_eq!(h.observe(at(4000), "PLAYING", 1.5, 5.5), vec![Note::Stall { secs: 2.0 }]);
        assert!(h.observe(at(5000), "PLAYING", 1.5, 6.5).is_empty(), "reported once");
        assert_eq!(h.observe(at(6000), "PLAYING", 2.5, 7.5), vec![Note::StallOver]);
        assert_eq!(h.stalls, 1);
        // A pause is not a stall.
        h.observe(at(7000), "PAUSED", 2.5, 8.5);
        assert!(h.observe(at(9000), "PLAYING", 2.5, 10.5).is_empty());
        assert!(h.observe(at(9500), "PLAYING", 3.0, 11.0).is_empty());
    }

    #[test]
    fn a_clean_start_has_no_notes_but_the_start() {
        let t0 = Instant::now();
        let mut h = Health::new(t0);
        assert!(h.observe(t0 + Duration::from_millis(300), "BUFFERING", 0.0, 2.3).is_empty(), "buffering before playing is the start");
        assert_eq!(h.observe(t0 + Duration::from_millis(900), "PLAYING", 0.2, 2.9), vec![Note::Started { after_load_ms: 900 }]);
        for i in 1..10u64 {
            assert!(h.observe(t0 + Duration::from_millis(900 + i * 2000), "PLAYING", 0.2 + i as f64 * 2.0, 2.9 + i as f64 * 2.0).is_empty());
        }
        assert_eq!((h.underruns, h.stalls), (0, 0));
    }
}
