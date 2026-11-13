//! Mouse wheel and trackpad over the volume controls: small steps, clamped, never scrolling the
//! page under them.

use rune::{Runtime, WheelEvent};

use super::player_bar::{stepped_volume, wheel_steps, wheel_volume_by};
use crate::state::State;

#[test]
fn a_wheel_notch_is_five_percent_and_a_trackpad_adds_up() {
    let mut acc = 0.0;
    assert_eq!(wheel_steps(&mut acc, 40.0), 5, "one notch (40 points) up");
    assert_eq!(wheel_steps(&mut acc, -40.0), -5, "one notch down");
    // A trackpad: 3 points at a time; 1% every 8 points, the rest kept.
    let steps: i32 = (0..8).map(|_| wheel_steps(&mut acc, 3.0)).sum();
    assert_eq!(steps, 3, "24 points make 3%");
    assert_eq!(acc, 0.0);
    wheel_steps(&mut acc, 5.0);
    assert_eq!(wheel_steps(&mut acc, -2.0), 0, "turning back starts again from zero");
    assert_eq!(acc, -2.0, "the 5 points saved upwards are dropped");
}

#[test]
fn steps_stay_on_whole_percents_within_0_and_100() {
    assert_eq!(stepped_volume(0.5, 5), 0.55);
    assert_eq!(stepped_volume(0.503, 1), 0.51, "a dragged 50.3% steps onto the grid");
    assert_eq!(stepped_volume(0.98, 5), 1.0);
    assert_eq!(stepped_volume(0.02, -5), 0.0);
    assert_eq!(stepped_volume(1.0, 3), 1.0);
}

#[test]
fn the_ends_send_nothing() {
    let mut s = State::offline();
    s.playback.volume = 1.0;
    s.volume_set_at = None;
    wheel_volume_by(&mut s, 400.0);
    assert!(s.volume_set_at.is_none(), "already at 100%: no set_volume, so nothing to the bridge");
    assert_eq!(s.ui.volume_wheel, 0.0, "nothing saved up past the end");
    wheel_volume_by(&mut s, -40.0);
    assert!((s.playback.volume - 0.95).abs() < 1e-9, "the first notch down answers at once");
    s.playback.volume = 0.0;
    wheel_volume_by(&mut s, -40.0);
    assert_eq!(s.playback.volume, 0.0);
}

/// Trackpad deltas of a point each send one volume change per whole percent, not per event.
#[test]
fn a_slow_trackpad_sends_one_change_per_percent() {
    let mut s = State::offline();
    s.playback.volume = 0.5;
    let mut sent = 0;
    for _ in 0..80 {
        let before = s.playback.volume;
        wheel_volume_by(&mut s, 1.0);
        sent += (s.playback.volume != before) as i32;
    }
    assert_eq!(sent, 10);
    assert!((s.playback.volume - 0.6).abs() < 1e-9);
}

fn window(scene: &str, now_playing: bool) -> Runtime<State> {
    let mut s = State::offline();
    (crate::scenes::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
    s.ui.now_playing = now_playing;
    s.playback.volume = 0.5;
    let mut rt = Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<State>) {
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

fn volume(rt: &mut Runtime<State>) -> f64 {
    let mut v = 0.0;
    rt.update(|s, _| v = s.playback.volume);
    v
}

/// Where every element outside the volume controls is.
fn layout(rt: &Runtime<State>) -> Vec<(String, [f32; 4])> {
    rt.elements().into_iter().filter(|e| !e.exiting && !e.path.contains("volume")).map(|e| (e.path, e.rect)).collect()
}

/// Wheel over `key`: up a notch, then a trackpad's worth down. The volume moves; nothing
/// else on screen does.
fn wheel_over(rt: &mut Runtime<State>, key: &str) {
    let r = rt.presented(key).unwrap_or_else(|| panic!("{key} on screen")).0;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, y);
    settle(rt);
    let before = layout(rt);
    rt.wheel_event(WheelEvent::new(0.0, 40.0).at(x, y));
    settle(rt);
    assert!((volume(rt) - 0.55).abs() < 1e-9, "{key}: a notch up is 5% louder");
    for _ in 0..4 {
        rt.wheel_event(WheelEvent::new(0.0, -4.0).at(x, y).precise(true));
    }
    settle(rt);
    assert!((volume(rt) - 0.53).abs() < 1e-9, "{key}: 16 trackpad points down are 2%");
    let after = layout(rt);
    let moved: Vec<_> = before.iter().filter(|b| !after.contains(b)).take(5).collect();
    assert!(moved.is_empty(), "{key}: nothing scrolled, but {moved:?} moved");
}

#[test]
fn wheel_over_the_player_bar_volume() {
    let mut rt = window("album", false);
    wheel_over(&mut rt, "volume");
}

#[test]
fn wheel_over_now_playing_volume() {
    let mut rt = window("now-playing-queue", true);
    wheel_over(&mut rt, "np-volume");
}
