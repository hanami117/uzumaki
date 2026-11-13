//! The lyrics and Up Next shortcuts in and out of full-screen Now Playing: in it they press its
//! own column buttons; elsewhere they act on the right sidebar as before.

use rune::{Key, KeyEvent, Mods, Runtime};

use crate::settings::{MusicSource, RightTab};
use crate::state::State;

/// `mod+<c>`: Cmd on macOS, Ctrl elsewhere (where Ctrl sets both `ctrl` and `cmd`).
fn chord(c: char) -> KeyEvent {
    let mods = if cfg!(target_os = "macos") {
        Mods { cmd: true, ..Default::default() }
    } else {
        Mods { cmd: true, ctrl: true, ..Default::default() }
    };
    KeyEvent { key: Key::Command(c), mods }
}

fn window(scene: &str, size: (f32, f32), reduced: bool, edit: impl FnOnce(&mut State)) -> Runtime<State> {
    let mut s = State::offline();
    (crate::scenes::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
    edit(&mut s);
    let mut rt = Runtime::new(s, crate::ui::view, size, rune::Env { system_dark: true, scale: 1.0 });
    rt.set_system_reduced_motion(reduced);
    rt.on_key = Some(crate::ui::on_key);
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<State>) {
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

fn press(rt: &mut Runtime<State>, ev: KeyEvent) {
    rt.key(ev);
    settle(rt);
}

fn click(rt: &mut Runtime<State>, key: &str) {
    let r = rt.presented(key).unwrap_or_else(|| panic!("{key} on screen")).0;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// (Now Playing open, its column, right sidebar shown, its tab).
fn read(rt: &mut Runtime<State>) -> (bool, Option<RightTab>, bool, RightTab) {
    let mut out = None;
    rt.update(|s, _| {
        out = Some((s.ui.now_playing, super::now_playing::column(s), s.settings.right_panel, s.settings.right_tab))
    });
    out.unwrap()
}

fn shown(rt: &Runtime<State>, key: &str) -> bool {
    rt.presented(key).is_some()
}

/// Lyrics on → Ctrl+L hides them, Ctrl+U shows Up Next, Ctrl+L switches back to the lyrics,
/// Ctrl+U twice goes to Up Next and back. The right sidebar is never touched.
fn cycle(rt: &mut Runtime<State>) {
    let (_, _, right0, tab0) = read(rt);
    assert_eq!(read(rt).1, Some(RightTab::Lyrics));
    press(rt, chord('l'));
    assert_eq!(read(rt).1, None, "Ctrl+L hides the lyrics");
    assert!(!shown(rt, "np-lyrics") && !shown(rt, "np-queue"));
    press(rt, chord('u'));
    assert_eq!(read(rt).1, Some(RightTab::Queue), "Ctrl+U shows Up Next");
    assert!(shown(rt, "np-queue"));
    press(rt, chord('l'));
    assert_eq!(read(rt).1, Some(RightTab::Lyrics), "Ctrl+L switches from Up Next to the lyrics");
    assert!(!shown(rt, "np-queue"));
    press(rt, chord('u'));
    press(rt, chord('u'));
    assert_eq!(read(rt).1, Some(RightTab::Lyrics), "Ctrl+U again goes back to what was there");
    let (np, _, right, tab) = read(rt);
    assert!(np, "Now Playing stays open");
    assert_eq!((right, tab), (right0, tab0), "the right sidebar is untouched");
}

#[test]
fn in_now_playing_the_keys_drive_its_own_column() {
    let mut rt = window("now-playing", (1360.0, 860.0), false, |_| {});
    cycle(&mut rt);
}

#[test]
fn in_a_narrow_now_playing_too() {
    // Narrow (the stacked layout): Ctrl+L used to only show the lyrics there, never hide them.
    let mut rt = window("now-playing", (640.0, 860.0), false, |_| {});
    cycle(&mut rt);
}

#[test]
fn with_reduced_motion_too() {
    let mut rt = window("now-playing", (1360.0, 860.0), true, |_| {});
    cycle(&mut rt);
}

#[test]
fn the_keys_follow_the_users_own_chords() {
    let mut rt = window("now-playing", (1360.0, 860.0), false, |s| {
        s.set_keybind("show_lyrics", "alt+y", "mod+l");
    });
    press(&mut rt, chord('l'));
    assert_eq!(read(&mut rt).1, Some(RightTab::Lyrics), "the old chord does nothing now");
    press(&mut rt, KeyEvent { key: Key::Text("y".into()), mods: Mods { alt: true, ..Default::default() } });
    assert_eq!(read(&mut rt).1, None, "the new chord hides the lyrics");
}

/// The key does what the button does, step for step, also for a song with no lyrics (the
/// button stays enabled there and the "No lyrics" hint shows).
#[test]
fn key_and_button_agree_for_a_song_without_lyrics() {
    let mut by_key = window("now-playing-nolyrics", (1360.0, 860.0), false, |_| {});
    let mut by_button = window("now-playing-nolyrics", (1360.0, 860.0), false, |_| {});
    assert!(shown(&by_key, "np-nolyrics"));
    for (k, b) in [('l', "np-lyrics-toggle"), ('l', "np-lyrics-toggle"), ('u', "np-queue-toggle"), ('l', "np-lyrics-toggle"), ('u', "np-queue-toggle")] {
        press(&mut by_key, chord(k));
        click(&mut by_button, b);
        assert_eq!(read(&mut by_key), read(&mut by_button));
        assert_eq!(shown(&by_key, "np-nolyrics"), shown(&by_button, "np-nolyrics"));
        assert_eq!(shown(&by_key, "np-queue"), shown(&by_button, "np-queue"));
    }
}

/// Local only mode: no lyrics anywhere, but Now Playing's lyrics button is there (it shows the
/// hint), so the key does the same; Up Next works as everywhere.
#[test]
fn local_only_mode() {
    let mut rt = window("now-playing-untagged", (1360.0, 860.0), false, |s| {
        s.settings.music_source = MusicSource::LocalOnly;
        if let Some(song) = s.playback.song.as_mut() {
            song.catalog_id = None;
        }
    });
    assert!(shown(&rt, "np-nolyrics"));
    press(&mut rt, chord('l'));
    assert_eq!(read(&mut rt).1, None);
    assert!(!shown(&rt, "np-nolyrics"), "lyrics off: no hint");
    press(&mut rt, chord('u'));
    assert!(shown(&rt, "np-queue"));
}

#[test]
fn outside_now_playing_nothing_changes() {
    let mut rt = window("album", (1360.0, 860.0), false, |s| s.settings.right_panel = false);
    let mut before = None;
    rt.update(|s, _| before = Some((s.settings.now_playing_lyrics, s.settings.now_playing_queue)));
    press(&mut rt, chord('l'));
    let (np, _, right, tab) = read(&mut rt);
    assert!(!np && right && tab == RightTab::Lyrics, "the right sidebar on the lyrics");
    press(&mut rt, chord('u'));
    let (np, _, right, tab) = read(&mut rt);
    assert!(!np && right && tab == RightTab::Queue, "switched to Up Next");
    press(&mut rt, chord('u'));
    assert!(!read(&mut rt).2, "again: the sidebar hides");
    let mut after = None;
    rt.update(|s, _| after = Some((s.settings.now_playing_lyrics, s.settings.now_playing_queue)));
    assert_eq!(before, after, "Now Playing's column untouched");
}

/// A narrow window with Now Playing closed: the keys still open it on that column, as before.
#[test]
fn narrow_and_closed_still_opens_now_playing() {
    let mut rt = window("album", (640.0, 860.0), false, |s| s.settings.right_panel = false);
    press(&mut rt, chord('u'));
    let (np, col, right, _) = read(&mut rt);
    assert!(np && col == Some(RightTab::Queue) && !right);
}
