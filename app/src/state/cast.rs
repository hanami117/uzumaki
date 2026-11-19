//! Chromecast spike in the app state: `crate::cast` holds the controller (only with
//! `UZUMAKI_CAST_SPIKE=1`); its events arrive as `Msg::Cast`, and `uzu cast` drives it.
//! One output at a time: a Cast device ends the AirPlay speakers and the other way round.

use uzumaki_cast::{Event, Meta, State as CastState};

use super::State;

#[derive(Default)]
pub struct Cast {
    pub state: CastState,
    /// The bridge must capture for the Cast session's socket.
    pub capture: bool,
}

impl State {
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
                let changed = (delay - self.playback.output_delay).abs() > 0.01;
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
                self.toast = Some(why.message(&name));
                true
            }
            Event::Devices(_) | Event::Volume { .. } => false,
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

    /// AirPlay was chosen: the Cast session ends.
    pub(super) fn cast_yield(&self) {
        if self.cast.state != CastState::Off
            && let Some(c) = crate::cast::controller()
        {
            c.stop();
        }
    }

    /// A seek or another song picked: with `UZUMAKI_CAST_RELOAD_ON_SEEK=1`, a new LOAD, so the
    /// device's buffer starts at the new place (else the old buffer plays out first).
    pub(super) fn cast_seeked(&self) {
        if matches!(self.cast.state, CastState::Playing { .. } | CastState::Buffering { .. })
            && std::env::var("UZUMAKI_CAST_RELOAD_ON_SEEK").is_ok_and(|v| v == "1")
            && let Some(c) = crate::cast::controller()
        {
            c.reload(Some(self.cast_meta()));
        }
    }

    fn cast_meta(&self) -> Meta {
        match &self.playback.song {
            Some(s) => Meta { title: s.title.clone(), artist: s.artist.clone(), album: s.album.clone(), image: s.artwork.as_ref().map(|a| a.url(600)).filter(|u| u.starts_with("https://")) },
            None => Meta { title: "Uzumaki".into(), ..Meta::default() },
        }
    }

    /// `uzu cast …` (on the UI thread; `List` is answered by `ctl.rs` itself, it waits).
    pub fn ctl_cast(&mut self, to: uzumaki_ctl::CastTo) -> Result<Vec<String>, String> {
        use uzumaki_ctl::CastTo;
        let c = crate::cast::controller().ok_or(crate::cast::OFF)?;
        let sent = |ok: bool| if ok { Ok(vec!["sent".to_string()]) } else { Err("no Cast session (uzu cast <name> first)".to_string()) };
        match to {
            CastTo::List | CastTo::Status => Ok(c.status_lines()),
            CastTo::Off => {
                c.stop();
                Ok(vec!["back to this computer".into()])
            }
            CastTo::Pause => sent(c.pause()),
            CastTo::Resume => sent(c.resume()),
            CastTo::Reload => sent(c.reload(Some(self.cast_meta()))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_switch_there_is_no_chromecast() {
        let mut s = State::offline();
        assert_eq!(s.ctl_cast(uzumaki_ctl::CastTo::Status), Err(crate::cast::OFF.to_string()));
        assert!(crate::cast::controller().is_none());
        // A failure from a session shows a toast; the state follows the controller.
        assert!(s.cast_event(Event::Failed { name: "TV".into(), why: uzumaki_cast::Why::NoFetch, detail: String::new() }));
        assert!(s.toast.as_deref().is_some_and(|t| t.contains("firewall")));
        assert!(!s.cast_event(Event::State(CastState::Connecting { name: "TV".into() })));
        assert_eq!(s.cast.state.name(), Some("TV"));
        // The measured delay shifts what lyrics and the progress bar show; back to 0 when off.
        s.playback.position = 10.0;
        assert!(s.cast_event(Event::State(CastState::Playing { name: "TV".into(), latency_ms: Some(2100) })));
        assert!((s.playback.heard_now() - 7.9).abs() < 1e-9);
        assert!(!s.cast_event(Event::State(CastState::Buffering { name: "TV".into() })), "kept while it rebuffers");
        assert!(s.cast_event(Event::State(CastState::Off)));
        assert_eq!(s.playback.heard_now(), 10.0);
    }
}
