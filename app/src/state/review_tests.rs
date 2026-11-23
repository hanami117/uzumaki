//! Review 2026-11-23 (shared playback state): failing tests that pin down drifts between two
//! views of the same playback.

use super::*;
use super::tests::song;

/// Previous on a Play Next song goes back to the song heard just before it. MusicKit's own queue
/// does (the bridge's Previous), but when the queued song follows the clicked song (`pos == base`)
/// the app steps back in the list instead, to the song *before* the clicked one.
#[test]
fn review_previous_on_a_play_next_song_returns_to_the_song_before_it() {
    let mut s = State::offline();
    let list = vec![song("a"), song("b"), song("c")];
    s.play_songs(&list, 1); // click "b"
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("b")))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    s.play_next(&[song("x")]);
    // "b" ends, MusicKit moves on to the queued "x".
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("x")))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    s.apply_msg(Msg::Bridge(Event::Time { current: 1.0, duration: 200.0 }));
    s.previous();
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("b"), "Previous should go back to b, the song before x");
}

/// The same from the first song of a list: Previous on the queued song restarts it instead of
/// going back to the clicked song.
#[test]
fn review_previous_on_a_play_next_song_after_the_first_song_returns_to_it() {
    let mut s = State::offline();
    let list = vec![song("a"), song("b")];
    s.play_songs(&list, 0);
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("a")))));
    s.play_next(&[song("x")]);
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("x")))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    s.previous();
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("a"), "Previous should go back to a");
}

/// On a Chromecast (measured delay) the window's bar and lyrics follow `heard_now`; `uzu`'s
/// status (its bar and synced lyrics) gets the player's position, seconds ahead.
#[test]
fn review_uzu_status_position_is_what_the_window_shows() {
    let mut s = State::offline();
    s.playback.song = Some(song("a"));
    s.playback.state = PlayState::Paused;
    s.playback.duration = 200.0;
    s.playback.position = 20.0;
    s.playback.output_delay = 7.7;
    let window = s.playback.heard_now();
    let uzu = s.ctl_status().position;
    assert!((uzu - window).abs() < 0.01, "uzu says {uzu:.1} s, the window {window:.1} s");
}
