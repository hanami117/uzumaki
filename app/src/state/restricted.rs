//! An account that doesn't allow explicit content (Apple's Screen Time / age restriction). MusicKit
//! refuses such songs with `[mk-007] CONTENT_RESTRICTED`. That is Apple's rule and we keep to it:
//! nothing here tries to play around it. The app says so in plain words once per session, skips on
//! to the next song that can play (a few in a row at most), and from the first refusal on treats
//! every explicit song as unplayable: dimmed, never queued for the bridge, passed over.

use super::*;

/// What the player says (any wrapping: `playback error: [mk-007] CONTENT_RESTRICTED; …`).
const REASON: &str = "CONTENT_RESTRICTED";

/// The one-time message. The toast also gets a "Open music.apple.com" button (`ui::view`).
pub const NOTICE: &str = "This song is explicit and your Apple account doesn't allow explicit content.";

/// The toast's second line, under `NOTICE`: where the setting is.
pub const NOTICE_HINT: &str = "Allow explicit content on music.apple.com, then restart Uzumaki.";

/// The Settings → Account paragraph.
pub const SETTINGS_TEXT: &str = "Uzumaki plays through Apple Music on the web, so the explicit content setting that counts is the one on music.apple.com (signed in with the same Apple Account: account menu \u{2192} Settings). A change made in Screen Time on an iPhone or Mac may not reach the web player. If songs are still refused, change it on music.apple.com itself, try another browser if it does not stick, then restart Uzumaki. An Apple Account for a child under 18 can't turn it on.";

/// Said when a run of songs ends playback: no `NOTICE` repeat, the reason it stopped.
const STOPPED: &str = "Stopped: the next songs are explicit too, and your Apple account doesn't allow explicit content.";

/// Where the setting lives: Uzumaki is the Apple Music web player, so its own account settings
/// decide, and the toast's button opens it in the browser.
pub const HELP_URL: &str = "https://music.apple.com";

/// The steps, for the button's tooltip and the Settings text. The web player first: a change made
/// in Screen Time on an iPhone or Mac does not always reach it.
pub const HELP_STEPS: &str = "On music.apple.com, signed in with the same Apple Account: account menu \u{2192} Settings \u{2192} allow explicit content. Also on iPhone or Mac: Screen Time \u{2192} Content & Privacy Restrictions \u{2192} Music: Explicit. A change made on the iPhone or Mac may not reach the web player, so change it on music.apple.com itself (try another browser if it does not stick), then restart Uzumaki. An Apple Account for a child under 18 can't turn it on.";

/// A second report of the same refusal (the play call fails, and the player reports it too) comes
/// within this many seconds; a real refusal of the next song takes a round trip to Apple.
const ECHO: Duration = Duration::from_millis(700);

/// Whether a player or bridge message is MusicKit refusing a song because of the account's
/// content restrictions.
pub fn is_restricted_error(message: &str) -> bool {
    message.contains(REASON)
}

impl State {
    /// A song the account can't play for sure: explicit while explicit content is refused.
    pub fn is_blocked(&self, song: &Song) -> bool {
        self.explicit_blocked && song.explicit
    }

    /// MusicKit's `restricted` flag (see `Event::Restricted`). Only ever switches the rule on:
    /// Apple's Screen Time restriction may not show in it, so `false` proves nothing.
    pub(super) fn restrictions_known(&mut self, restricted: bool) {
        if restricted {
            self.block_explicit();
        }
    }

    fn block_explicit(&mut self) {
        if self.explicit_blocked {
            return;
        }
        self.explicit_blocked = true;
        self.queue.block_explicit = true;
        self.sync_upcoming(); // MusicKit gets no explicit songs from now on
    }

    /// The player refused the current song (`first`: the id the refused call was for, when the
    /// refusal answers one). Says why once, then plays on from the next song that can play.
    pub(super) fn content_restricted(&mut self, first: Option<&str>) {
        if self.playback.local || self.resume.is_some() {
            return;
        }
        let Some(song) = self.playback.song.clone() else { return };
        match first {
            Some(id) if song.play_id() != id && song.id != id => return, // a newer song took over
            // The same refusal told twice, or one for the song we just left.
            None if self.restricted_at.is_some_and(|t| crate::clock::since(t) < ECHO) => return,
            _ => {}
        }
        self.restricted_at = Some(crate::clock::now());
        self.starting = None;
        self.mark_explicit(&song);
        self.block_explicit();
        self.skipped_in_row += 1;
        let next = if self.skipped_in_row < MAX_SKIPS { self.next_allowed() } else { None };
        match next {
            Some(next) => {
                if !std::mem::replace(&mut self.restricted_told, true) {
                    self.toast = Some(NOTICE.into());
                }
                self.begin_current(next);
            }
            None => {
                let told = std::mem::replace(&mut self.restricted_told, true);
                self.toast = Some(if told { STOPPED } else { NOTICE }.into());
                self.playback.set_state(PlayState::Stopped);
                self.skipped_in_row = 0;
                if let Some(b) = &self.bridge {
                    b.announce(); // MusicKit may still be playing the song from before
                }
            }
        }
    }

    /// The next song of the queue that isn't blocked, moving the queue to it. Blocked ones are
    /// passed over without being tried (they are not played, so they stay out of the history).
    /// Bounded by the size of everything queued, so repeat all on a list of blocked songs ends.
    pub(super) fn next_allowed(&mut self) -> Option<Song> {
        let q = &self.queue;
        let mut left = q.user.len() + q.context.len() + q.autoplay.len() + 1;
        while left > 0 {
            left -= 1;
            let next = self.queue.advance(self.playback.repeat)?;
            if !self.is_blocked(&next) {
                return Some(next);
            }
        }
        None
    }

    /// Records that `song` is explicit wherever this session lists it (a library song Apple
    /// didn't rate, say), so it is badged, dimmed and passed over from now on.
    fn mark_explicit(&mut self, song: &Song) {
        self.for_each_song(|s| {
            if s.same_track(song) {
                s.explicit = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::{playing_id, song};

    fn explicit(id: &str) -> Song {
        Song { explicit: true, ..song(id) }
    }
    fn refused(first: &str) -> Msg {
        Msg::QueueFailed { first: first.into(), error: "[mk-007] CONTENT_RESTRICTED; Content restricted".into() }
    }
    fn event() -> Msg {
        Msg::Bridge(Event::Error("playback error: [mk-007] CONTENT_RESTRICTED; Content restricted".into()))
    }

    #[test]
    fn the_players_wording_is_recognised() {
        assert!(is_restricted_error("playback error: [mk-007] CONTENT_RESTRICTED; Content restricted"));
        assert!(is_restricted_error("[mk-007] CONTENT_RESTRICTED"));
        assert!(!is_restricted_error("[mk-007] NOT_FOUND; One or more items could not be resolved"));
        assert!(!is_restricted_error("playback error: [mk-007] SUBSCRIPTION_ERROR"));
    }

    #[test]
    fn a_refusal_says_why_once_and_skips_to_the_next_song() {
        let mut s = State::offline();
        s.play_list(&[explicit("1"), song("2"), song("3")], 0, "");
        s.apply_msg(refused("1"));
        assert_eq!(playing_id(&s), Some("2"));
        assert_eq!(s.toast.as_deref(), Some(NOTICE));
        assert!(s.explicit_blocked);
        s.toast = None;
        s.apply_msg(refused("2")); // an unrated song the account refuses too
        assert_eq!(playing_id(&s), Some("3"));
        assert_eq!(s.toast, None, "said once per session");
    }

    #[test]
    fn explicit_songs_are_passed_over_once_the_account_is_known_to_refuse_them() {
        let mut s = State::offline();
        s.play_list(&[explicit("1"), explicit("2"), explicit("3"), song("4")], 0, "");
        s.apply_msg(refused("1"));
        assert_eq!(playing_id(&s), Some("4"), "2 and 3 were not even tried");
        assert_eq!(s.skipped_in_row, 1);
        assert_eq!(s.queue.upcoming_ids(), Vec::<String>::new());
    }

    #[test]
    fn the_bridge_is_never_handed_blocked_songs() {
        let mut s = State::offline();
        s.play_list(&[song("1"), explicit("2"), song("3")], 0, "");
        assert_eq!(s.queue.upcoming_ids(), ["2", "3"]);
        s.apply_msg(Msg::Bridge(Event::Restricted(true)));
        assert_eq!(s.queue.upcoming_ids(), ["3"]);
        assert_eq!(s.queue.cycle_ids(), Some(vec!["1".to_string(), "3".to_string()]));
        s.apply_msg(Msg::Bridge(Event::Restricted(false)));
        assert!(s.explicit_blocked, "a false flag proves nothing");
    }

    #[test]
    fn a_player_report_after_the_play_call_failed_does_not_skip_twice() {
        crate::clock::freeze();
        let mut s = State::offline();
        s.play_list(&[song("1"), song("2"), song("3")], 0, "");
        s.apply_msg(refused("1"));
        assert_eq!(playing_id(&s), Some("2"));
        s.apply_msg(event());
        assert_eq!(playing_id(&s), Some("2"), "the same refusal, told again");
        crate::clock::advance(Duration::from_secs(2));
        s.apply_msg(event());
        assert_eq!(playing_id(&s), Some("3"), "a later refusal is the next song's");
    }

    #[test]
    fn a_list_of_refused_songs_stops_with_one_message() {
        crate::clock::freeze();
        let mut s = State::offline();
        let songs: Vec<Song> = (1..=9).map(|i| song(&i.to_string())).collect();
        s.play_list(&songs, 0, "");
        for i in 1..=MAX_SKIPS {
            crate::clock::advance(Duration::from_secs(2));
            s.apply_msg(refused(&i.to_string()));
        }
        assert_eq!(playing_id(&s), Some("5"));
        assert_eq!(s.playback.state, PlayState::Stopped);
        assert_eq!(s.toast.as_deref(), Some(STOPPED));
        assert_eq!(s.skipped_in_row, 0);
    }

    #[test]
    fn every_song_explicit_and_repeat_all_still_ends() {
        let mut s = State::offline();
        s.play_list(&[explicit("1"), explicit("2"), explicit("3")], 0, "");
        s.set_repeat(Repeat::All);
        s.apply_msg(refused("1"));
        assert_eq!(s.playback.state, PlayState::Stopped);
        assert_eq!(s.toast.as_deref(), Some(NOTICE), "the reason, once");
    }

    #[test]
    fn the_end_of_the_queue_says_why_it_stopped() {
        let mut s = State::offline();
        s.play_list(&[explicit("1")], 0, "");
        s.apply_msg(refused("1"));
        assert_eq!(s.playback.state, PlayState::Stopped);
        assert_eq!(s.toast.as_deref(), Some(NOTICE), "a single song: the plain reason");
    }

    #[test]
    fn songs_you_queued_are_passed_over_and_the_history_stays_true() {
        let mut s = State::offline();
        s.play_list(&[song("1"), song("2")], 0, "");
        s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
        s.queue.add_to_queue(&[explicit("u1"), song("u2")]);
        s.apply_msg(Msg::Bridge(Event::Restricted(true)));
        let next = s.next_allowed();
        assert_eq!(next.as_ref().map(|n| n.id.as_str()), Some("u2"), "u1 never plays");
        assert!(s.history.tracks.iter().all(|(_, h)| h.id != "u1"));
    }

    #[test]
    fn a_refused_song_not_marked_explicit_becomes_so() {
        let mut s = State::offline();
        s.play_list(&[song("1"), song("2")], 0, "");
        s.apply_msg(refused("1"));
        assert!(s.queue.original[0].explicit);
        assert!(s.is_blocked(&s.queue.original[0]));
    }
}
