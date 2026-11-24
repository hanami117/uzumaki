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

// ------------------------------------------------------------ media controls (state L1–L4)

/// What the media controls (MPRIS, macOS Now Playing, SMTC) were told, folded the way they fold
/// it: the last song, the last play state, the last position.
#[derive(Debug, Default, PartialEq)]
struct Shown {
    song: Option<String>,
    playing: Option<bool>,
    position: Option<f64>,
}

type Told = std::sync::Arc<std::sync::Mutex<Vec<Event>>>;

fn listen(s: &mut State) -> Told {
    let told: Told = Default::default();
    let t = told.clone();
    s.media_out = Some(Box::new(move |e| t.lock().unwrap().push(e.clone())));
    told
}

fn shown(told: &Told) -> Shown {
    let mut out = Shown::default();
    for e in told.lock().unwrap().iter() {
        match e {
            Event::NowPlaying(Some(song)) => {
                out.song = Some(song.id.clone());
                out.position = Some(0.0);
            }
            Event::PlayState(p) => out.playing = Some(p.is_active()),
            Event::Stopped | Event::Exited => out.playing = Some(false),
            Event::Time { current, .. } => out.position = Some(*current),
            _ => {}
        }
    }
    out
}

fn bridge(s: &mut State, e: Event) {
    s.apply(Msg::Bridge(e), &mut Tx::default());
}

/// L1: a new song was asked for; MusicKit's old queue winds down (Paused) within the start-up
/// grace. The window shows the new song loading; the media controls must too.
#[test]
fn review_media_controls_show_the_new_song_while_the_old_queue_winds_down() {
    let mut s = State::offline();
    let told = listen(&mut s);
    s.play_songs(&[song("a")], 0);
    bridge(&mut s, Event::NowPlaying(Some(song("a"))));
    bridge(&mut s, Event::PlayState(PlayState::Playing));
    bridge(&mut s, Event::Time { current: 30.0, duration: 200.0 });
    s.play_songs(&[song("b"), song("c")], 0);
    bridge(&mut s, Event::PlayState(PlayState::Paused)); // the old queue, ignored by the window
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("b"));
    assert!(s.playback.state.is_active());
    let m = shown(&told);
    assert_eq!((m.song.as_deref(), m.playing), (Some("b"), Some(true)), "media controls: {m:?}");
}

/// L1: on a Cast device the window's bar follows `heard_now` (player minus the measured delay).
#[test]
fn review_media_controls_position_is_what_the_window_shows() {
    let mut s = State::offline();
    let told = listen(&mut s);
    s.play_songs(&[song("a")], 0);
    bridge(&mut s, Event::NowPlaying(Some(song("a"))));
    bridge(&mut s, Event::PlayState(PlayState::Playing));
    s.playback.output_delay = 7.7;
    bridge(&mut s, Event::Time { current: 20.0, duration: 200.0 });
    let window = s.playback.heard_now();
    let m = shown(&told).position.unwrap();
    assert!((m - window).abs() < 0.5, "media controls at {m:.1} s, the window at {window:.1} s");
}

/// L1 + L3: the last song of the list isn't on Apple Music: the window stops (no event from the
/// bridge says so). The media controls (and the tray, which follows the same state) must stop too.
#[test]
fn review_media_controls_stop_after_the_last_unavailable_song() {
    let mut s = State::offline();
    let told = listen(&mut s);
    s.play_songs(&[song("a"), song("b")], 0);
    bridge(&mut s, Event::NowPlaying(Some(song("a"))));
    bridge(&mut s, Event::PlayState(PlayState::Playing));
    s.play_songs(&[song("a"), song("b")], 1); // b, the list's last song
    s.apply(Msg::QueueFailed { first: "b".into(), error: "NOT_FOUND".into() }, &mut Tx::default());
    assert!(!s.playback.state.is_active(), "the window stops");
    let m = shown(&told);
    assert_eq!(m.playing, Some(false), "media controls: {m:?}");
}

/// L1: a local file plays and is paused from the window: the controls follow the local player.
/// The (idle) bridge's own reports must not reach them.
#[test]
fn review_media_controls_follow_the_local_player_not_the_bridge() {
    let mut s = State::offline();
    let told = listen(&mut s);
    s.play_songs(&[song("a")], 0);
    bridge(&mut s, Event::NowPlaying(Some(song("a"))));
    bridge(&mut s, Event::PlayState(PlayState::Playing));
    s.play_songs(&[super::tests::local("x")], 0);
    let generation = s.local_gen;
    s.apply(Msg::Local { generation, event: Event::PlayState(PlayState::Playing) }, &mut Tx::default());
    bridge(&mut s, Event::PlayState(PlayState::Paused)); // the bridge stopping for the local file
    let m = shown(&told);
    assert_eq!(m.song.as_deref(), s.playback.song.as_ref().map(|x| x.id.as_str()), "media controls: {m:?}");
    assert_eq!(m.playing, Some(true), "media controls: {m:?}");
}

/// L2: Pause pressed while the new song is still loading: MusicKit pauses, and the window must
/// show it (not keep the spinner).
#[test]
fn review_pause_while_a_song_loads_is_shown() {
    let mut s = State::offline();
    s.play_songs(&[song("a")], 0);
    s.pause();
    bridge(&mut s, Event::PlayState(PlayState::Paused));
    assert_eq!(s.playback.state, PlayState::Paused);
}

/// L4: Previous's 3-second rule counts what is heard: 1 s into the song on a Cast device (8.7 s
/// in the player) goes back a song instead of restarting this one.
#[test]
fn review_previous_counts_the_heard_position() {
    let mut s = State::offline();
    s.play_songs(&[song("a"), song("b")], 1);
    bridge(&mut s, Event::NowPlaying(Some(song("b"))));
    bridge(&mut s, Event::PlayState(PlayState::Playing));
    s.playback.output_delay = 7.7;
    s.playback.position = 8.7;
    s.playback.position_at = None;
    s.previous();
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("a"), "Previous at 1 s heard goes back");
}
