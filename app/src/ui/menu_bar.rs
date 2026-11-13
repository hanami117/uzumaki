//! The macOS menu bar: Uzumaki, Edit, View, Controls, Window, Help, built from `keys::BINDINGS`
//! (the user's changes included), so menus, Settings → Keyboard and the keys can't disagree.
//!
//! Rune owns the menu bar (`rune::MenuItem`, `AppHandle::set_menu`): winit's app menu (Hide,
//! Hide Others, Show All, Quit) and Rune's standard Edit and Window items (`App::standard_menus`:
//! Undo … Select All, Minimize ⌘M, Close ⌘W, which hand their keys to Rune's text fields
//! first) stay as Rune installs them. This module adds the app's own items; `menu_bar_mac.rs`
//! tidies the app menu (one About, "Uzumaki" titles) with AppKit.
//!
//! **One path per key press.** Every item is `keys_first`: AppKit hands its key equivalent to the
//! window first, so the press goes through Rune exactly as without the menu (a text field, the
//! onboarding's tour, `ui::on_key` → `keys::handle`), and the item runs only if no handler took
//! the key. `keys::handle` takes every enabled item's chord (its `return false` cases change
//! nothing), so in practice keys never run items; a click on an item runs it through
//! `keys::perform`, the same function the key runs, with the same modal rules. A disabled item
//! gets no key equivalent: AppKit swallows the equivalent of a disabled item, and the key must
//! reach Rune as before (the tour, a modal's own keys).
//!
//! The model is plain data (`model`), tested without AppKit; `sync` (from the view) hands it to
//! Rune when it changes. Never built headless. Linux has no menu bar (nothing here runs there).

use rune::Tx;

use super::keys::{self, Action, BINDINGS};
use super::{narrow, onboarding};
use crate::settings::RightTab;
use crate::state::{Repeat, State};

/// A top-level menu. Edit is Rune's standard one, untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// The app menu (bold "Uzumaki").
    App,
    View,
    Controls,
    /// Rune's Window menu (Minimize, Close); Zoom is added after them.
    Window,
    Help,
}

impl Menu {
    /// The menu bar title; None for the app menu.
    pub fn title(self) -> Option<&'static str> {
        match self {
            Menu::App => None,
            Menu::View => Some("View"),
            Menu::Controls => Some("Controls"),
            Menu::Window => Some("Window"),
            Menu::Help => Some("Help"),
        }
    }
}

/// What an item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// The standard About panel.
    About,
    /// Check for updates now (`State::check_updates_now`).
    CheckUpdates,
    /// The same as the shortcut of this action (`keys::perform`).
    Key(Action),
    /// Zoom the window, as the green button's zoom.
    Zoom,
    /// Help → Show the Welcome Tour (Settings → General → Show again).
    Tour,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub menu: Menu,
    /// None for a separator.
    pub command: Option<Command>,
    /// Stable item id for Rune; empty for separators.
    pub id: &'static str,
    pub title: String,
    /// The key equivalent in Rune chord syntax ("mod+shift+b"), as the user has it now; None
    /// when the action has no shortcut, or the item is disabled (see the module docs).
    pub chord: Option<String>,
    pub enabled: bool,
    pub checked: bool,
}

fn sep(menu: Menu) -> Entry {
    Entry { menu, command: None, id: "", title: String::new(), chord: None, enabled: true, checked: false }
}

fn plain(menu: Menu, id: &'static str, title: &str, command: Command, enabled: bool) -> Entry {
    Entry { menu, command: Some(command), id, title: title.to_string(), chord: None, enabled, checked: false }
}

/// The current chord of `action` (`keys::chord_of`), in chord syntax.
fn chord(s: &State, action: Action) -> Option<String> {
    let i = BINDINGS.iter().position(|b| b.action == action)?;
    keys::chord_of(s, i).map(|c| c.to_string())
}

/// Whether the item for `action` can run now: during the onboarding only the tour card's own
/// shortcut (it plays in the picture); otherwise what the keys allow (modals).
fn enabled(s: &State, action: Action) -> bool {
    if onboarding::active(s) {
        onboarding::menu_takes(s, action)
    } else {
        keys::allowed_now(s, action)
    }
}

fn key(s: &State, menu: Menu, id: &'static str, title: &str, action: Action, checked: bool) -> Entry {
    let on = enabled(s, action);
    Entry {
        menu,
        command: Some(Command::Key(action)),
        id,
        title: title.to_string(),
        chord: if on { chord(s, action) } else { None },
        enabled: on,
        checked,
    }
}

/// Every item the app adds, in menu order (Rune keeps the order within each menu).
pub fn model(s: &State) -> Vec<Entry> {
    use Menu::*;
    let wide = !narrow::narrow(s);
    let set = &s.settings;
    let right = set.right_panel && wide;
    let tour = onboarding::active(s);
    let mut v = vec![
        plain(App, "app.about", "About Uzumaki", Command::About, true),
        plain(App, "app.check_updates", "Check for Updates\u{2026}", Command::CheckUpdates, !tour && !s.update_check.running),
        sep(App),
        key(s, App, "app.settings", "Settings\u{2026}", Action::Settings, false),
        sep(App),
        key(s, View, "view.sidebar", "Sidebar", Action::ToggleSidebar, !set.sidebar_collapsed && wide),
        key(s, View, "view.right", "Right Sidebar", Action::ToggleRightPanel, right),
        sep(View),
    ];
    // The two tabs are ticked as the right sidebar shows them, or in Now Playing as its column
    // does (the keys drive that column there).
    let shown = |tab| {
        if super::now_playing::is_open(s) { super::now_playing::column(s) == Some(tab) } else { right && set.right_tab == tab }
    };
    // Lyrics are Apple Music's: local only mode has none (the tour leaves them out too).
    if !s.local_only() {
        v.push(key(s, View, "view.lyrics", "Lyrics", Action::ShowLyrics, shown(RightTab::Lyrics)));
    }
    v.extend([
        key(s, View, "view.queue", "Up Next", Action::ShowQueue, shown(RightTab::Queue)),
        sep(View),
        key(s, View, "view.now_playing", "Now Playing", Action::NowPlaying, s.ui.now_playing),
        sep(View),
        key(s, View, "view.find", "Find", Action::Find, false),
    ]);
    let playing = s.playback.state.is_active();
    let repeat = match s.playback.repeat {
        Repeat::Off => "Repeat: Off",
        Repeat::All => "Repeat: All",
        Repeat::One => "Repeat: One",
    };
    v.extend([
        key(s, Controls, "controls.play", if playing { "Pause" } else { "Play" }, Action::PlayPause, false),
        key(s, Controls, "controls.next", "Next", Action::Next, false),
        key(s, Controls, "controls.previous", "Previous", Action::Previous, false),
        sep(Controls),
        key(s, Controls, "controls.shuffle", "Shuffle", Action::Shuffle, s.playback.shuffle),
        key(s, Controls, "controls.repeat", repeat, Action::Repeat, s.playback.repeat != Repeat::Off),
        sep(Controls),
        key(s, Controls, "controls.volume_up", "Volume Up", Action::VolumeUp, false),
        key(s, Controls, "controls.volume_down", "Volume Down", Action::VolumeDown, false),
        sep(Window),
        plain(Window, "window.zoom", "Zoom", Command::Zoom, true),
        key(s, Help, "help.shortcuts", "Keyboard Shortcuts", Action::Shortcuts, false),
        plain(Help, "help.tour", "Show the Welcome Tour", Command::Tour, !tour),
    ]);
    v
}

/// Runs a picked item. The same `keys::perform` its shortcut runs; during the onboarding, the
/// tour card's effect (or nothing behind it).
pub fn run(s: &mut State, command: Command, tx: &mut Tx) {
    match command {
        Command::About => about(),
        Command::CheckUpdates => s.check_updates_now(),
        Command::Zoom => tx.toggle_maximized(),
        Command::Tour => {
            if !onboarding::active(s) {
                onboarding::replay(s);
            }
        }
        Command::Key(action) if onboarding::active(s) => onboarding::menu_press(s, action, tx),
        // Settings… opens Settings; picked again it stays (its shortcut still toggles).
        Command::Key(Action::Settings) if s.ui.settings.open.is_some() => {}
        Command::Key(action) => {
            keys::perform(s, action, tx);
        }
    }
}

#[cfg(target_os = "macos")]
fn about() {
    crate::menu_bar_mac::about();
}
#[cfg(not(target_os = "macos"))]
fn about() {}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::{Cell, RefCell};

    use rune::{KeyPlatform, MenuItem, Shortcut, Tx};

    use super::{Command, Entry, run};
    use crate::ui::keys::Action;
    use crate::state::State;

    thread_local! {
        /// The model last handed to Rune.
        static LAST: RefCell<Option<Vec<Entry>>> = const { RefCell::new(None) };
        /// `menu_bar_mac::tidy` has nothing left to do.
        static TIDY: Cell<bool> = const { Cell::new(false) };
    }

    /// From the view (UI thread = main thread on macOS): hands the menus to Rune when they
    /// changed, and tidies the app menu until it is done. Nothing headless.
    pub fn sync(s: &State) {
        if rune::is_headless() {
            return;
        }
        let Some(handle) = s.app_handle() else { return };
        let model = super::model(s);
        let changed = LAST.with(|l| {
            let mut l = l.borrow_mut();
            if l.as_ref() == Some(&model) {
                return false;
            }
            *l = Some(model.clone());
            true
        });
        if changed {
            if let Err(e) = handle.set_menu(model.iter().map(item)) {
                eprintln!("uzumaki: menu bar: {e}");
            }
            for e in handle.take_menu_errors() {
                eprintln!("uzumaki: menu bar: {e}");
            }
        }
        if !TIDY.with(Cell::get) {
            TIDY.with(|t| t.set(crate::menu_bar_mac::tidy()));
        }
    }

    fn item(e: &Entry) -> MenuItem<State> {
        let mut m = match e.command {
            None => MenuItem::separator(),
            Some(c) => MenuItem::new(e.title.clone(), action(c)).id(e.id).enabled(e.enabled).checked(e.checked).keys_first(),
        };
        if let Some(title) = e.menu.title() {
            m = m.menu(title);
        }
        if let Some(sc) = e.chord.as_deref().and_then(shortcut) {
            m = m.key_equivalent(sc);
        }
        m
    }

    /// A chord as Rune's menu shortcut ("mod" is ⌘).
    pub fn shortcut(chord: &str) -> Option<Shortcut> {
        Shortcut::parse_on(chord, KeyPlatform::Mac).ok()
    }

    /// Item actions are plain `fn`s (no captures): one per command.
    fn action(c: Command) -> fn(&mut State, &mut Tx) {
        macro_rules! key {
            ($($a:ident),* $(,)?) => {
                match c {
                    Command::About => |s, tx| run(s, Command::About, tx),
                    Command::CheckUpdates => |s, tx| run(s, Command::CheckUpdates, tx),
                    Command::Zoom => |s, tx| run(s, Command::Zoom, tx),
                    Command::Tour => |s, tx| run(s, Command::Tour, tx),
                    $(Command::Key(Action::$a) => |s, tx| run(s, Command::Key(Action::$a), tx),)*
                    Command::Key(_) => |_, _| {},
                }
            };
        }
        key!(
            Settings, ToggleSidebar, ToggleRightPanel, ShowLyrics, ShowQueue, NowPlaying, Find, PlayPause, Next,
            Previous, Shuffle, Repeat, VolumeUp, VolumeDown, Shortcuts,
        )
    }

    #[cfg(test)]
    pub fn action_for_test(c: Command) -> fn(&mut State, &mut Tx) {
        action(c)
    }
}
#[cfg(target_os = "macos")]
pub use mac::sync;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MusicSource;

    fn find<'a>(m: &'a [Entry], id: &str) -> &'a Entry {
        m.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("no item {id}"))
    }

    fn titles(m: &[Entry], menu: Menu) -> Vec<String> {
        m.iter().filter(|e| e.menu == menu).map(|e| if e.command.is_none() { "-".into() } else { e.title.clone() }).collect()
    }

    #[test]
    fn the_menus_hold_the_agreed_items_in_order() {
        let s = State::offline();
        let m = model(&s);
        assert_eq!(titles(&m, Menu::App), ["About Uzumaki", "Check for Updates\u{2026}", "-", "Settings\u{2026}", "-"]);
        assert_eq!(titles(&m, Menu::View), ["Sidebar", "Right Sidebar", "-", "Lyrics", "Up Next", "-", "Now Playing", "-", "Find"]);
        assert_eq!(
            titles(&m, Menu::Controls),
            ["Play", "Next", "Previous", "-", "Shuffle", "Repeat: Off", "-", "Volume Up", "Volume Down"]
        );
        assert_eq!(titles(&m, Menu::Window), ["-", "Zoom"]);
        assert_eq!(titles(&m, Menu::Help), ["Keyboard Shortcuts", "Show the Welcome Tour"]);
        // Ids are unique (Rune refuses the whole menu otherwise).
        let ids: Vec<_> = m.iter().filter(|e| e.command.is_some()).map(|e| e.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
        assert!(m.iter().filter(|e| e.command.is_some()).all(|e| e.enabled));
    }

    #[test]
    fn key_equivalents_come_from_the_bindings() {
        let s = State::offline();
        let m = model(&s);
        for (id, chord) in [
            ("app.settings", "mod+comma"),
            ("view.sidebar", "mod+b"),
            ("view.right", "mod+shift+b"),
            ("view.lyrics", "mod+l"),
            ("view.queue", "mod+u"),
            ("view.now_playing", "mod+shift+f"),
            ("view.find", "mod+f"),
            ("controls.play", "space"),
            ("controls.next", "mod+right"),
            ("controls.previous", "mod+left"),
            ("controls.shuffle", "mod+s"),
            ("controls.repeat", "mod+r"),
            ("controls.volume_up", "mod+up"),
            ("controls.volume_down", "mod+down"),
            ("help.shortcuts", "mod+slash"),
        ] {
            let want = rune::Chord::parse(chord).unwrap().to_string();
            assert_eq!(find(&m, id).chord.as_deref(), Some(want.as_str()), "{id}");
        }
        for id in ["app.about", "window.zoom", "help.tour"] {
            assert_eq!(find(&m, id).chord, None, "{id} has no shortcut");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_key_equivalent_is_a_menu_shortcut() {
        let s = State::offline();
        for e in model(&s).iter().filter(|e| e.chord.is_some()) {
            let sc = mac::shortcut(e.chord.as_deref().unwrap());
            assert!(sc.is_some(), "{} ({:?}) parses as a menu shortcut", e.id, e.chord);
        }
        let b = mac::shortcut(&find(&model(&s), "view.right").chord.clone().unwrap()).unwrap();
        assert_eq!(b, "cmd+shift+b".parse::<rune::Shortcut>().unwrap(), "mod is ⌘");
    }

    #[test]
    fn rebinds_and_removed_shortcuts_follow_the_users_settings() {
        let mut s = State::offline();
        s.settings.keybinds.insert("toggle_sidebar".into(), "alt+s".into());
        s.settings.keybinds.insert("play_pause".into(), String::new());
        let m = model(&s);
        assert_eq!(find(&m, "view.sidebar").chord.as_deref(), Some(rune::Chord::parse("alt+s").unwrap().to_string().as_str()));
        assert_eq!(find(&m, "controls.play").chord, None, "removed: no key equivalent, still in the menu");
        assert!(find(&m, "controls.play").enabled);
    }

    #[test]
    fn items_show_the_state() {
        let mut s = State::offline();
        let m = model(&s);
        assert!(find(&m, "view.sidebar").checked, "sidebar open by default");
        assert!(!find(&m, "view.right").checked);
        assert!(!find(&m, "controls.shuffle").checked);
        assert_eq!(find(&m, "controls.play").title, "Play");
        s.settings.sidebar_collapsed = true;
        s.settings.right_panel = true;
        s.settings.right_tab = RightTab::Queue;
        s.playback.shuffle = true;
        s.playback.repeat = Repeat::One;
        s.playback.state = crate::bridge::PlayState::Playing;
        s.ui.now_playing = true;
        let m = model(&s);
        assert!(!find(&m, "view.sidebar").checked);
        assert!(find(&m, "view.right").checked);
        assert!(find(&m, "view.queue").checked && !find(&m, "view.lyrics").checked);
        assert!(find(&m, "view.now_playing").checked);
        assert!(find(&m, "controls.shuffle").checked);
        assert_eq!(find(&m, "controls.repeat").title, "Repeat: One");
        assert!(find(&m, "controls.repeat").checked);
        assert_eq!(find(&m, "controls.play").title, "Pause");
        s.playback.repeat = Repeat::All;
        assert_eq!(find(&model(&s), "controls.repeat").title, "Repeat: All");
    }

    #[test]
    fn local_only_mode_has_no_lyrics_item() {
        let mut s = State::offline();
        s.settings.music_source = MusicSource::LocalOnly;
        let m = model(&s);
        assert!(m.iter().all(|e| e.id != "view.lyrics"));
        assert_eq!(titles(&m, Menu::View), ["Sidebar", "Right Sidebar", "-", "Up Next", "-", "Now Playing", "-", "Find"]);
    }

    #[test]
    fn a_modal_greys_what_its_keys_refuse_and_drops_their_equivalents() {
        let mut s = State::offline();
        super::super::settings::open(&mut s);
        let m = model(&s);
        for id in ["view.sidebar", "view.right", "view.now_playing", "controls.shuffle", "controls.repeat", "help.shortcuts"] {
            assert!(!find(&m, id).enabled, "{id} greyed while Settings is open");
            assert_eq!(find(&m, id).chord, None, "{id}: AppKit would swallow a disabled item's key");
        }
        for id in ["app.settings", "controls.play", "controls.next", "controls.volume_up", "view.find"] {
            assert!(find(&m, id).enabled, "{id} works in Settings, as its key does");
            assert!(find(&m, id).chord.is_some());
        }
    }

    #[test]
    fn during_the_onboarding_only_the_tour_cards_shortcut_is_on() {
        let mut s = State::offline();
        onboarding::scene(&mut s, onboarding::Mode::FirstRun, onboarding::Step::Welcome, None);
        let m = model(&s);
        assert!(m.iter().filter(|e| matches!(e.command, Some(Command::Key(_)))).all(|e| !e.enabled && e.chord.is_none()));
        assert!(!find(&m, "help.tour").enabled);
        assert!(find(&m, "app.about").enabled);
        // The tour's first card (play and pause): its item is on, with its key.
        onboarding::scene(&mut s, onboarding::Mode::FirstRun, onboarding::Step::Tour, None);
        let m = model(&s);
        let on: Vec<_> = m.iter().filter(|e| matches!(e.command, Some(Command::Key(_))) && e.enabled).map(|e| e.id).collect();
        assert_eq!(on, ["controls.play"]);
        assert!(find(&m, "controls.play").chord.is_some());
    }

    #[test]
    fn picking_an_item_runs_the_same_action_as_its_key() {
        let mut s = State::offline();
        let mut tx = Tx::default();
        run(&mut s, Command::Key(Action::ToggleSidebar), &mut tx);
        assert!(s.settings.sidebar_collapsed);
        run(&mut s, Command::Key(Action::Shuffle), &mut tx);
        assert!(s.playback.shuffle);
        run(&mut s, Command::Key(Action::Repeat), &mut tx);
        assert_eq!(s.playback.repeat, Repeat::All);
        run(&mut s, Command::Key(Action::ShowQueue), &mut tx);
        assert!(s.settings.right_panel && s.settings.right_tab == RightTab::Queue);
        run(&mut s, Command::Key(Action::Settings), &mut tx);
        assert!(s.ui.settings.open.is_some());
        // Settings… again keeps it open (its key toggles).
        run(&mut s, Command::Key(Action::Settings), &mut tx);
        assert!(s.ui.settings.open.is_some());
        // While Settings is open the refused actions do nothing, as their keys.
        run(&mut s, Command::Key(Action::ToggleSidebar), &mut tx);
        assert!(s.settings.sidebar_collapsed);
        super::super::settings::close(&mut s);
        run(&mut s, Command::Tour, &mut tx);
        assert!(onboarding::active(&s));
        // Behind the onboarding a pick changes nothing in the app.
        run(&mut s, Command::Key(Action::ToggleSidebar), &mut tx);
        assert!(s.settings.sidebar_collapsed);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn each_item_gets_its_own_action() {
        // What the menu commands change, to compare an item's fn with `run`.
        fn print(s: &State) -> String {
            format!(
                "{} {} {:?} {} {:?} {} {:?} {:?}",
                s.settings.sidebar_collapsed,
                s.settings.right_panel,
                s.settings.right_tab,
                s.playback.shuffle,
                s.playback.repeat,
                s.ui.now_playing,
                s.ui.settings.open.is_some(),
                s.route,
            )
        }
        for e in model(&State::offline()) {
            let Some(c @ Command::Key(_)) = e.command else { continue };
            let (mut a, mut b) = (State::offline(), State::offline());
            (mac::action_for_test(c))(&mut a, &mut Tx::default());
            run(&mut b, c, &mut Tx::default());
            assert_eq!(print(&a), print(&b), "{}", e.id);
        }
        let mut s = State::offline();
        (mac::action_for_test(Command::Key(Action::Shuffle)))(&mut s, &mut Tx::default());
        assert!(s.playback.shuffle, "Shuffle's item shuffles");
    }
}
