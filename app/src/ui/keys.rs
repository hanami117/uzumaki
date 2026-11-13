//! Every keyboard shortcut, in one table: `on_key` dispatches through it and Settings →
//! Keyboard lists it, so the two can't disagree. Chords use Rune's syntax ("mod" is Ctrl on
//! Linux) and Rune prints them ("Ctrl+Shift+B").
//!
//! Shortcuts are customisable: `BINDINGS` holds the defaults, `Settings::keybinds` the user's
//! changes (by `Action::id`), and the number shortcuts 1–9 have their own modifier and target
//! (`Settings::number_mod`, `number_keys`). The map is rebuilt per key press (a few dozen
//! parses: nothing next to a frame).

use rune::{Chord, Key, KeyEvent, Tx};

use super::select::Move;
use crate::settings::{NumberKeys, NumberMod, RightTab};
use crate::state::{LibraryTab, Route, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PlayPause,
    Next,
    Previous,
    SeekForward,
    SeekBack,
    VolumeUp,
    VolumeDown,
    Shuffle,
    Repeat,
    Back,
    Find,
    /// The number shortcuts: page or pin `n` (1–9).
    Slot(u8),
    ToggleSidebar,
    ToggleRightPanel,
    ShowLyrics,
    ShowQueue,
    NowPlaying,
    NewPlaylist,
    RemoveFromPlaylist,
    SelectNext,
    SelectPrevious,
    ExtendNext,
    ExtendPrevious,
    SelectFirst,
    SelectLast,
    ExtendFirst,
    ExtendLast,
    SelectAll,
    PlaySelected,
    Settings,
    Shortcuts,
    Close,
    Quit,
}

impl Action {
    /// Stable name in `Settings::keybinds`.
    pub fn id(self) -> &'static str {
        match self {
            Action::PlayPause => "play_pause",
            Action::Next => "next",
            Action::Previous => "previous",
            Action::SeekForward => "seek_forward",
            Action::SeekBack => "seek_back",
            Action::VolumeUp => "volume_up",
            Action::VolumeDown => "volume_down",
            Action::Shuffle => "shuffle",
            Action::Repeat => "repeat",
            Action::Back => "back",
            Action::Find => "find",
            Action::Slot(_) => "slot",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::ToggleRightPanel => "toggle_right_panel",
            Action::ShowLyrics => "show_lyrics",
            Action::ShowQueue => "show_queue",
            Action::NowPlaying => "now_playing",
            Action::NewPlaylist => "new_playlist",
            Action::RemoveFromPlaylist => "remove_from_playlist",
            Action::SelectNext => "select_next",
            Action::SelectPrevious => "select_previous",
            Action::ExtendNext => "extend_next",
            Action::ExtendPrevious => "extend_previous",
            Action::SelectFirst => "select_first",
            Action::SelectLast => "select_last",
            Action::ExtendFirst => "extend_first",
            Action::ExtendLast => "extend_last",
            Action::SelectAll => "select_all",
            Action::PlaySelected => "play_selected",
            Action::Settings => "settings",
            Action::Shortcuts => "shortcuts",
            Action::Close => "close",
            Action::Quit => "quit",
        }
    }
}

/// What the number shortcuts open in `NumberKeys::Pages` mode, 1 to 9.
pub const PAGES: [(&str, fn() -> Route); 9] = [
    ("Home", || Route::Home),
    ("Albums", || Route::Library(LibraryTab::Albums)),
    ("Songs", || Route::Library(LibraryTab::Songs)),
    ("All Playlists", || Route::Library(LibraryTab::Playlists)),
    ("Local Files", || Route::LocalFiles),
    ("Recently Added", || Route::RecentlyAdded),
    ("Artists", || Route::Artists),
    ("Made for You", || Route::MadeForYou),
    ("Radio", || Route::Radio),
];

/// What the number shortcuts open in local only mode: its library tabs, in sidebar order but
/// Songs first (Ctrl+1 stays Local Files, as before the tabs).
pub const LOCAL_PAGES: [(&str, fn() -> Route); 4] = [
    ("Songs", || Route::LocalFiles),
    ("Artists", || Route::LocalArtists),
    ("Albums", || Route::LocalAlbums),
    ("Folders", || Route::Folders { path: None }),
];

pub struct Binding {
    /// Rune chord syntax.
    pub chord: &'static str,
    pub action: Action,
    /// What it does, as Settings → Keyboard shows it.
    pub label: &'static str,
    /// Heading it is listed under.
    pub group: &'static str,
}

const fn b(chord: &'static str, action: Action, label: &'static str, group: &'static str) -> Binding {
    Binding { chord, action, label, group }
}

pub const PLAYBACK: &str = "Playback";
pub const NAVIGATION: &str = "Go to";
pub const VIEW: &str = "View";
pub const LISTS: &str = "Song lists";
pub const LIBRARY: &str = "Library";
pub const APP: &str = "App";

/// Groups in the order Settings lists them.
pub const GROUPS: [&str; 6] = [PLAYBACK, NAVIGATION, VIEW, LISTS, LIBRARY, APP];

pub const BINDINGS: &[Binding] = &[
    b("space", Action::PlayPause, "Play or pause", PLAYBACK),
    b("mod+right", Action::Next, "Next song", PLAYBACK),
    b("mod+left", Action::Previous, "Previous song (or back to the start)", PLAYBACK),
    b("shift+right", Action::SeekForward, "Skip forward 10 seconds", PLAYBACK),
    b("shift+left", Action::SeekBack, "Skip back 10 seconds", PLAYBACK),
    b("mod+up", Action::VolumeUp, "Volume up", PLAYBACK),
    b("mod+down", Action::VolumeDown, "Volume down", PLAYBACK),
    b("mod+s", Action::Shuffle, "Shuffle on or off", PLAYBACK),
    b("mod+r", Action::Repeat, "Repeat: off, all, one", PLAYBACK),
    b("alt+left", Action::Back, "Back", NAVIGATION),
    b("mod+f", Action::Find, "Filter this list (again: Search)", NAVIGATION),
    b("mod+b", Action::ToggleSidebar, "Show or hide the sidebar", VIEW),
    b("mod+shift+b", Action::ToggleRightPanel, "Show or hide the right sidebar", VIEW),
    b("mod+l", Action::ShowLyrics, "Lyrics (right sidebar, or in Now Playing)", VIEW),
    b("mod+u", Action::ShowQueue, "Up Next (right sidebar, or in Now Playing)", VIEW),
    b("mod+shift+f", Action::NowPlaying, "Full-screen Now Playing", VIEW),
    b("mod+n", Action::NewPlaylist, "New playlist (with the selected songs)", LIBRARY),
    b("down", Action::SelectNext, "Select the next song", LISTS),
    b("up", Action::SelectPrevious, "Select the previous song", LISTS),
    b("shift+down", Action::ExtendNext, "Add the next song to the selection", LISTS),
    b("shift+up", Action::ExtendPrevious, "Add the previous song to the selection", LISTS),
    b("home", Action::SelectFirst, "Select the first song", LISTS),
    b("end", Action::SelectLast, "Select the last song", LISTS),
    b("shift+home", Action::ExtendFirst, "Select up to the first song", LISTS),
    b("shift+end", Action::ExtendLast, "Select down to the last song", LISTS),
    b("delete", Action::RemoveFromPlaylist, "Remove the selected songs from this playlist", LIBRARY),
    b("mod+a", Action::SelectAll, "Select all songs in the list", LISTS),
    b("enter", Action::PlaySelected, "Play the selected songs", LISTS),
    b("mod+comma", Action::Settings, "Settings", APP),
    b("mod+slash", Action::Shortcuts, "Keyboard shortcuts", APP),
    b("escape", Action::Close, "Close (menu, filter, Now Playing, dialog)", APP),
    b("mod+q", Action::Quit, "Quit", APP),
];

/// The chord of `BINDINGS[i]` now: the user's change, else the default; None when removed.
pub fn chord_of(s: &State, i: usize) -> Option<Chord> {
    let b = &BINDINGS[i];
    match s.settings.keybinds.get(b.action.id()) {
        Some(c) if c.is_empty() => None,
        Some(c) => Chord::parse(c).ok().or_else(|| Chord::parse(b.chord).ok()),
        None => Chord::parse(b.chord).ok(),
    }
}

/// Whether `BINDINGS[i]` differs from its default.
pub fn is_custom(s: &State, i: usize) -> bool {
    s.settings.keybinds.contains_key(BINDINGS[i].action.id())
}

/// The pages slots 1 to 9 open now: local only mode skips the Apple ones (so Ctrl+1 is
/// Local Files there).
fn pages(s: &State) -> impl Iterator<Item = &'static (&'static str, fn() -> Route)> {
    let list: &'static [(&'static str, fn() -> Route)] = if s.local_only() { &LOCAL_PAGES } else { &PAGES };
    list.iter()
}

/// The number shortcut for slot `n` (1–9), if number shortcuts are on.
pub fn slot_chord(s: &State, n: u8) -> Option<Chord> {
    let m = match s.settings.number_mod {
        NumberMod::Ctrl => "mod",
        NumberMod::Alt => "alt",
        NumberMod::Off => return None,
    };
    Chord::parse(&format!("{m}+{n}")).ok()
}

/// How `BINDINGS[i]` reads now ("Ctrl+Shift+B"), or None when it has no shortcut.
pub fn label(s: &State, i: usize) -> Option<String> {
    chord_of(s, i).map(|c| c.label())
}

/// "Toggle sidebar (Ctrl+B)": a tooltip with the current shortcut of `action`, if any.
pub fn hint(s: &State, text: &str, action: Action) -> String {
    match BINDINGS.iter().position(|b| b.action == action).and_then(|i| label(s, i)) {
        Some(k) => format!("{text} ({k})"),
        None => text.to_string(),
    }
}

/// What slot `n` opens now, for Settings ("Home", or a pin's title).
pub fn slot_target(s: &State, n: u8) -> Option<String> {
    let i = (n as usize).checked_sub(1)?;
    match s.settings.number_keys {
        NumberKeys::Pages => pages(s).nth(i).map(|p| p.0.to_string()),
        // Pins are Apple Music's.
        NumberKeys::Pins if s.local_only() => None,
        NumberKeys::Pins => s.settings.pins.get(i).map(|p| p.title.clone()),
    }
}

/// Every active shortcut: the bindings (as changed by the user), then the number slots.
fn keymap(s: &State) -> Vec<(Chord, Action)> {
    let mut map: Vec<(Chord, Action)> = (0..BINDINGS.len()).filter_map(|i| chord_of(s, i).map(|c| (c, BINDINGS[i].action))).collect();
    map.extend((1..=9).filter_map(|n| slot_chord(s, n).map(|c| (c, Action::Slot(n)))));
    map
}

/// One label per key of `BINDINGS[i]` now ("Ctrl", "Shift", "B"; "⇧", "⌘", "B" on macOS), for
/// drawing keycaps; None when it has no shortcut.
pub fn caps(s: &State, i: usize) -> Option<Vec<String>> {
    chord_of(s, i).map(|c| c.keycaps_on(rune::KeyPlatform::current()))
}

/// `caps` of the first binding for `action`.
pub fn action_caps(s: &State, action: Action) -> Option<Vec<String>> {
    BINDINGS.iter().position(|b| b.action == action).and_then(|i| caps(s, i))
}

/// The keycaps of number slot `n`, if number shortcuts are on.
pub fn slot_caps(s: &State, n: u8) -> Option<Vec<String>> {
    slot_chord(s, n).map(|c| c.keycaps_on(rune::KeyPlatform::current()))
}

/// The action `ev` triggers now, user changes included (the onboarding's tour asks too).
pub fn action_for(s: &State, ev: &KeyEvent, tx: &Tx) -> Option<Action> {
    find_tx(s, ev, tx)
}

/// The action `ev` triggers now.
fn find_tx(s: &State, ev: &KeyEvent, tx: &Tx) -> Option<Action> {
    keymap(s).into_iter().find(|(c, _)| c.matches_tx(ev, tx)).map(|(_, a)| a)
}

// Only the Linux key tests use it (macOS sets `mod` from Cmd, not Ctrl).
#[cfg(all(test, target_os = "linux"))]
fn find(s: &State, ev: &KeyEvent) -> Option<Action> {
    keymap(s).into_iter().find(|(c, _)| c.matches(ev)).map(|(_, a)| a)
}

/// Keys that still work while a modal (Settings, the New Playlist dialog) is open.
fn allowed_in_modal(a: Action) -> bool {
    matches!(a, Action::Close | Action::Settings | Action::Quit | Action::PlayPause | Action::Next | Action::Previous | Action::VolumeUp | Action::VolumeDown)
}

/// Where the keyboard focus came from, so Enter presses only what Tab reached (as
/// `:focus-visible`). Rune knows (it draws its focus ring only for keyboard focus) but doesn't
/// tell handlers, and has no focus-change callback, so the app keeps its own record: Tab and
/// Shift+Tab send `MARKER` after themselves, which arrives once the focus has moved and names
/// the element Tab reached; a mouse click (`pointer`) forgets it. The view never reads it.
#[derive(Default)]
pub struct KbdFocus {
    /// A Tab went by; the marker that follows it records where it landed.
    pending: bool,
    /// `Tx::focused` of the element Tab or Shift+Tab focused, until a click or Escape.
    key: Option<String>,
    /// Enter sent a Space to press the focused control: let it through to Rune.
    press: bool,
    /// The pointer's last position, to tell a button release from a move (`pointer`).
    ptr: Option<(f32, f32)>,
}

/// Sent after Tab to learn where the focus went. No keyboard sends F24 with every modifier,
/// and nothing binds it.
fn marker() -> KeyEvent {
    KeyEvent { key: Key::Function(24), mods: rune::Mods { shift: true, cmd: true, alt: true, ctrl: true, logo: true } }
}

fn plain(ev: &KeyEvent) -> bool {
    !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo)
}

/// Every key handler calls this first: notes a Tab (it goes on, so Rune moves the focus) and
/// takes the marker that follows it, recording the element Tab reached. True for the marker.
pub fn track_focus(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if *ev == marker() {
        if std::mem::take(&mut s.ui.kbd.pending) {
            s.ui.kbd.key = tx.focused().map(str::to_string);
        }
        return true;
    }
    if ev.key == Key::Tab && plain(ev) && !s.ui.kbd.pending {
        s.ui.kbd.pending = true;
        tx.send_key(marker());
    }
    false
}

/// The focused element got its focus from Tab or Shift+Tab, not from a click.
pub fn keyboard_focused(s: &State, tx: &Tx) -> bool {
    s.ui.kbd.key.is_some() && s.ui.kbd.key.as_deref() == tx.focused()
}

/// Forget the keyboard focus (a click, Escape).
pub fn forget_focus(s: &mut State) {
    s.ui.kbd.key = None;
    s.ui.kbd.press = false;
}

/// The window root's `on_pointer_move`. Rune reports moves, and every button release as a
/// move to where the pointer already is: that release is a click, after which the focus
/// (if any) is the mouse's. Changes nothing the view reads, so it never rebuilds the view.
pub fn pointer(s: &mut State, x: f32, y: f32) {
    if s.ui.kbd.ptr == Some((x, y)) {
        forget_focus(s);
    }
    s.ui.kbd.ptr = Some((x, y));
}

/// A key while a control has focus (a button, a song row, a menu item, a switch: anything but
/// a text field, whose keys Rune keeps for it). Rune offers it here, on the window's root,
/// before it presses the focused control (a song row's own `on_key` comes first, so Enter on a
/// row still plays it); a clicked control keeps the focus, so without this Space after a click
/// on Shuffle would shuffle again instead of pausing. The play/pause shortcut runs instead of
/// the press. Enter (and keypad Enter) presses the control only when Tab brought the focus
/// there: Rune presses a focused button on Space alone, so Enter sends one, marked to get
/// through. After a click Enter does nothing here, as before. Every other key goes on as
/// before (Tab moves focus, Enter picks a song row or the highlighted menu item). While a
/// shortcut is being recorded, Space is recorded rather than pressing the focused Record button.
pub fn control_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if track_focus(s, ev, tx) {
        return true;
    }
    let space = matches!(&ev.key, Key::Text(t) if t == " ") && plain(ev);
    if std::mem::take(&mut s.ui.kbd.press) && space && ev.mods == rune::Mods::default() {
        return false; // Enter's Space: Rune presses the focused control
    }
    if ev.key == Key::Enter && ev.mods == rune::Mods::default() && s.ui.recording.is_none() && keyboard_focused(s, tx) {
        if !tx.key_repeat() {
            s.ui.kbd.press = true;
            tx.send_key(KeyEvent { key: Key::Text(" ".into()), mods: rune::Mods::default() });
        }
        return true;
    }
    let takes = if s.ui.recording.is_some() { space } else { find_tx(s, ev, tx) == Some(Action::PlayPause) };
    takes && handle(s, ev, tx)
}

/// Runs the shortcut `ev` matches. Returns whether it was used.
pub fn handle(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if let Some(i) = s.ui.recording {
        return record(s, i, ev, tx);
    }
    let Some(action) = find_tx(s, ev, tx) else { return false };
    perform(s, action, tx)
}

/// A modal (Settings, the New Playlist dialog) is open: only `allowed_in_modal` keys work.
pub fn modal_open(s: &State) -> bool {
    s.ui.settings.open.is_some() || super::new_playlist::is_open(s)
}

/// Whether `action` does anything now as far as modals go (the menu bar greys the rest).
#[cfg(any(target_os = "macos", test))]
pub fn allowed_now(s: &State, action: Action) -> bool {
    !modal_open(s) || allowed_in_modal(action) || (action == Action::Find && s.ui.settings.open.is_some())
}

/// Runs `action`, from its shortcut or from the menu bar (macOS): one path for both, so the
/// same modal rules apply. Returns whether it was used; false means nothing changed.
pub fn perform(s: &mut State, action: Action, tx: &mut Tx) -> bool {
    // Settings has its own search field.
    if action == Action::Find && s.ui.settings.open.is_some() && !super::new_playlist::is_open(s) {
        super::settings::focus_search(s);
        return true;
    }
    if modal_open(s) && !allowed_in_modal(action) {
        return false;
    }
    match action {
        // A held key plays and pauses once, not on every repeat.
        Action::PlayPause if tx.key_repeat() => {}
        Action::PlayPause => s.toggle_play(),
        Action::Next => s.next(),
        Action::Previous => s.previous(),
        Action::SeekForward => s.seek_by(10.0),
        Action::SeekBack => s.seek_by(-10.0),
        Action::VolumeUp => s.volume_by(0.05),
        Action::VolumeDown => s.volume_by(-0.05),
        Action::Shuffle => s.toggle_shuffle(),
        Action::Repeat => s.cycle_repeat(),
        Action::Back => s.back(),
        Action::Find => {
            // Filter the page's list; again while the filter has focus (or on any page without
            // one), go to the Search page and focus its field.
            let in_filter = tx.focused() == Some("filter");
            if in_filter || super::lists::filter_list_id(s).is_none() {
                s.navigate(Route::Search);
                super::search::focus(s);
            } else {
                super::filter::open(s);
            }
        }
        Action::Slot(n) => {
            let i = n as usize - 1;
            match s.settings.number_keys {
                NumberKeys::Pages => {
                    let Some(page) = pages(s).nth(i).map(|p| p.1) else { return false };
                    s.navigate(page());
                }
                NumberKeys::Pins if s.local_only() => return false,
                NumberKeys::Pins => match s.settings.pins.get(i).cloned() {
                    Some(pin) => s.open_pin(&pin),
                    None => return false,
                },
            }
        }
        Action::ToggleSidebar => super::narrow::toggle_sidebar(s),
        Action::ToggleRightPanel => super::narrow::toggle_right_panel(s),
        // In Now Playing they drive its own column (as its buttons do); elsewhere the right sidebar.
        Action::ShowLyrics if super::now_playing::key_column(s, RightTab::Lyrics) => {}
        Action::ShowQueue if super::now_playing::key_column(s, RightTab::Queue) => {}
        Action::ShowLyrics => super::narrow::show_tab(s, RightTab::Lyrics),
        Action::ShowQueue => super::narrow::show_tab(s, RightTab::Queue),
        Action::NowPlaying => {
            if s.ui.now_playing {
                super::now_playing::close(s);
            } else {
                super::open_now_playing(s);
            }
        }
        Action::NewPlaylist => {
            let songs = super::select::selected(s);
            super::new_playlist::open(s, songs);
        }
        Action::RemoveFromPlaylist => {
            let Some(list) = super::lists::list_id(s) else { return false };
            let Some(c) = super::menu::editable_playlist_here(s, &list) else { return false };
            let songs = super::select::selected_rows(s);
            if songs.is_empty() {
                return false;
            }
            s.remove_from_playlist(&c, &songs);
        }
        Action::SelectNext => return arrow(s, tx, Move::By(1), false),
        Action::SelectPrevious => return arrow(s, tx, Move::By(-1), false),
        Action::ExtendNext => return arrow(s, tx, Move::By(1), true),
        Action::ExtendPrevious => return arrow(s, tx, Move::By(-1), true),
        Action::SelectFirst => return arrow(s, tx, Move::First, false),
        Action::SelectLast => return arrow(s, tx, Move::Last, false),
        Action::ExtendFirst => return arrow(s, tx, Move::First, true),
        Action::ExtendLast => return arrow(s, tx, Move::Last, true),
        Action::SelectAll => {
            if super::lists::list_id(s).is_none() {
                return false;
            }
            super::select::select_all(s);
        }
        Action::PlaySelected => {
            if tx.focused().is_some() {
                return false; // Enter belongs to the focused field or button
            }
            super::select::play_selected(s);
        }
        Action::Settings => super::settings::toggle(s),
        Action::Shortcuts => super::settings::open_tab(s, super::settings::Tab::Keyboard),
        Action::Close => {
            if super::escape(s) {
                return true;
            }
            // Nothing to close: drop the focus (and its ring), so Enter and the arrows act on the
            // page again. A text field keeps its own Esc.
            if tx.focused().is_none() {
                return false;
            }
            forget_focus(s);
            tx.blur();
        }
        Action::Quit => tx.quit(),
    }
    true
}

/// Arrow keys in the page's song list: move (or extend) the selection and keep the row in
/// view. Home/End also scroll the page to its top or bottom (those rows may not be built yet).
fn arrow(s: &mut State, tx: &mut Tx, mv: Move, extend: bool) -> bool {
    let Some(id) = super::select::move_lead(s, mv, extend) else { return false };
    if let Some(list) = super::lists::list_id(s) {
        tx.scroll_into_view(super::select::mark_key(&list, &id));
    }
    if let Some(scroll) = super::lists::scroll_key(s) {
        match mv {
            Move::First => tx.scroll_to(scroll, 0.0, 0.0),
            Move::Last => tx.scroll_to_bottom(scroll),
            Move::By(_) => {}
        }
    }
    true
}

// ---------------------------------------------------------------- recording a new shortcut

/// Settings → Keyboard: the next key press becomes the shortcut of `BINDINGS[i]`.
pub fn start_recording(s: &mut State, i: usize) {
    s.ui.recording = Some(i);
}

/// Handles the key press while recording: Esc cancels, Backspace/Delete removes the shortcut,
/// anything else becomes it. A chord another action had is taken from it (with a toast); one
/// the number shortcuts use is refused.
fn record(s: &mut State, i: usize, ev: &KeyEvent, tx: &Tx) -> bool {
    let b = &BINDINGS[i];
    match ev.key {
        Key::Escape => {
            s.ui.recording = None;
            return true;
        }
        Key::Backspace | Key::Delete => {
            s.ui.recording = None;
            s.set_keybind(b.action.id(), "", b.chord);
            return true;
        }
        _ => {}
    }
    let Some(chord) = chord_from_event(ev, tx) else { return true }; // keep waiting
    let platform = rune::KeyPlatform::current();
    if (1..=9).any(|n| slot_chord(s, n).is_some_and(|c| c.same_keys(&chord, platform))) {
        s.toast = Some(format!("{} opens the number shortcuts. Change those above, or pick other keys.", chord.label()));
        return true;
    }
    s.ui.recording = None;
    let text = chord.to_string();
    for j in (0..BINDINGS.len()).filter(|&j| j != i) {
        if chord_of(s, j).is_some_and(|c| c.same_keys(&chord, platform)) {
            let other = &BINDINGS[j];
            s.set_keybind(other.action.id(), "", other.chord);
            s.toast = Some(format!("{} moved here from “{}”, which has no shortcut now.", chord.label(), other.label));
        }
    }
    let default = Chord::parse(b.chord).map(|c| c.to_string()).unwrap_or_default();
    s.set_keybind(b.action.id(), &text, &default);
    true
}

/// The chord a key press makes, in canonical form. None for presses that can't be a shortcut.
fn chord_from_event(ev: &KeyEvent, tx: &Tx) -> Option<Chord> {
    let key: String = match &ev.key {
        Key::Command(c) => key_name(tx.key_base().filter(char::is_ascii).unwrap_or(*c))?,
        Key::Text(t) => {
            let mut cs = t.chars();
            let (c, None) = (cs.next()?, cs.next()) else { return None };
            // Shift changes the typed character ("!" for 1): name the key by its own.
            let base = tx.key_base().filter(char::is_ascii).unwrap_or(c);
            key_name(if ev.mods.shift { base } else { c })?
        }
        Key::Enter => "enter".into(),
        Key::Tab => "tab".into(),
        Key::Left => "left".into(),
        Key::Right => "right".into(),
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::Home => "home".into(),
        Key::End => "end".into(),
        Key::PageUp => "pageup".into(),
        Key::PageDown => "pagedown".into(),
        Key::Insert => "insert".into(),
        Key::Function(n) => format!("f{n}"),
        _ => return None,
    };
    let mut parts: Vec<&str> = Vec::new();
    if ev.mods.cmd || ev.mods.ctrl {
        parts.push("mod");
    }
    if ev.mods.alt {
        parts.push("alt");
    }
    if ev.mods.shift {
        parts.push("shift");
    }
    if ev.mods.logo && !ev.mods.cmd {
        parts.push("super");
    }
    let spec = parts.into_iter().chain([key.as_str()]).collect::<Vec<_>>().join("+");
    Chord::parse(&spec).ok()
}

/// The chord-syntax name of a character key.
fn key_name(c: char) -> Option<String> {
    Some(match c {
        ' ' => "space".into(),
        '+' => "plus".into(),
        c if c.is_control() => return None,
        c => c.to_lowercase().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use rune::Mods;

    #[cfg(target_os = "linux")]
    fn ev(key: Key, ctrl: bool, shift: bool, alt: bool) -> KeyEvent {
        // Off macOS, Ctrl sets both `ctrl` and `cmd` (Rune's "mod").
        KeyEvent { key, mods: Mods { shift, cmd: ctrl, alt, ctrl, ..Default::default() } }
    }

    #[test]
    fn every_default_parses_and_none_is_bound_twice() {
        let s = State::offline();
        let platform = rune::KeyPlatform::current();
        let map = keymap(&s);
        assert_eq!(map.len(), BINDINGS.len() + 9, "all bindings and nine number slots");
        for (i, (a, _)) in map.iter().enumerate() {
            for (b, _) in map.iter().skip(i + 1) {
                assert!(!a.same_keys(b, platform), "{a} is bound twice");
            }
        }
        assert!(BINDINGS.iter().all(|b| GROUPS.contains(&b.group)));
        let ids: std::collections::HashSet<_> = BINDINGS.iter().map(|b| b.action.id()).collect();
        assert_eq!(ids.len(), BINDINGS.len(), "ids are unique");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn key_presses_reach_the_right_action() {
        let s = State::offline();
        let f = |key, c, sh, a| find(&s, &ev(key, c, sh, a));
        assert_eq!(f(Key::Text(" ".into()), false, false, false), Some(Action::PlayPause));
        assert_eq!(f(Key::Command('b'), true, false, false), Some(Action::ToggleSidebar));
        assert_eq!(f(Key::Command('b'), true, true, false), Some(Action::ToggleRightPanel));
        assert_eq!(f(Key::Command('n'), true, false, false), Some(Action::NewPlaylist));
        assert_eq!(f(Key::Command(','), true, false, false), Some(Action::Settings));
        assert_eq!(f(Key::Command('3'), true, false, false), Some(Action::Slot(3)));
        assert_eq!(f(Key::Right, true, false, false), Some(Action::Next));
        assert_eq!(f(Key::Right, false, true, false), Some(Action::SeekForward));
        assert_eq!(f(Key::Left, false, false, true), Some(Action::Back));
        assert_eq!(f(Key::Enter, false, false, false), Some(Action::PlaySelected));
        assert_eq!(f(Key::Escape, false, false, false), Some(Action::Close));
        assert_eq!(f(Key::Delete, false, false, false), Some(Action::RemoveFromPlaylist));
        assert_eq!(f(Key::Down, false, false, false), Some(Action::SelectNext));
        assert_eq!(f(Key::Up, false, true, false), Some(Action::ExtendPrevious));
        assert_eq!(f(Key::End, false, true, false), Some(Action::ExtendLast));
        assert_eq!(f(Key::Up, true, false, false), Some(Action::VolumeUp), "Ctrl+Up stays volume");
        assert_eq!(f(Key::Right, false, false, false), None, "plain arrows stay with lists and fields");
        assert_eq!(f(Key::Text(" ".into()), true, false, false), None, "Ctrl+Space is not play");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn alt_numbers_and_pins_when_chosen() {
        let mut s = State::offline();
        s.set_number_shortcuts(NumberMod::Alt, NumberKeys::Pins);
        assert_eq!(find(&s, &ev(Key::Text("2".into()), false, false, true)), Some(Action::Slot(2)));
        assert_eq!(find(&s, &ev(Key::Command('2'), true, false, false)), None, "Ctrl+2 is free now");
        assert_eq!(slot_target(&s, 1), None, "no pins yet");
        s.settings.pins.push(crate::settings::Pin { kind: crate::settings::PinKind::Artist, id: "a1".into(), title: "Telu".into(), artwork: None });
        assert_eq!(slot_target(&s, 1).as_deref(), Some("Telu"));
        s.set_number_shortcuts(NumberMod::Off, NumberKeys::Pages);
        assert_eq!(keymap(&s).len(), BINDINGS.len());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn changed_and_removed_shortcuts() {
        let mut s = State::offline();
        let sidebar = BINDINGS.iter().position(|b| b.action == Action::ToggleSidebar).unwrap();
        assert_eq!(label(&s, sidebar).as_deref(), Some("Ctrl+B"));
        s.set_keybind("toggle_sidebar", "alt+s", "mod+b");
        assert!(is_custom(&s, sidebar));
        assert_eq!(label(&s, sidebar).as_deref(), Some("Alt+S"));
        assert_eq!(find(&s, &ev(Key::Text("s".into()), false, false, true)), Some(Action::ToggleSidebar));
        assert_eq!(find(&s, &ev(Key::Command('b'), true, false, false)), None);
        assert_eq!(hint(&s, "Toggle sidebar", Action::ToggleSidebar), "Toggle sidebar (Alt+S)");
        s.set_keybind("toggle_sidebar", "", "mod+b");
        assert_eq!(label(&s, sidebar), None);
        assert_eq!(hint(&s, "Toggle sidebar", Action::ToggleSidebar), "Toggle sidebar");
        s.set_keybind("toggle_sidebar", "mod+b", "mod+b");
        assert!(!is_custom(&s, sidebar), "back to the default forgets the change");
        s.settings.keybinds.insert("next".into(), "nonsense+++".into());
        let next = BINDINGS.iter().position(|b| b.action == Action::Next).unwrap();
        assert_eq!(label(&s, next).as_deref(), Some("Ctrl+→"), "a broken saved chord falls back to the default");
    }

    /// The album scene in a headless window, settled, with the app's `on_key`.
    fn album_window() -> rune::Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "album").unwrap().1)(&mut s);
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(crate::ui::on_key);
        settle(&mut rt);
        rt
    }

    fn settle(rt: &mut rune::Runtime<State>) {
        rt.frame(0.0);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn click(rt: &mut rune::Runtime<State>, key: &str) {
        let r = rt.presented(key).unwrap_or_else(|| panic!("{key} on screen")).0;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    fn press(rt: &mut rune::Runtime<State>, key: Key) {
        rt.key(KeyEvent { key, mods: Default::default() });
        settle(rt);
    }

    /// After a click on Shuffle the button has focus, and Rune presses a focused button on
    /// Space: Space must still pause, not shuffle again.
    #[test]
    fn space_after_clicking_shuffle_pauses_instead_of_shuffling_again() {
        let mut rt = album_window();
        click(&mut rt, "shuffle");
        assert!(rt.has_focus(), "the clicked button keeps the focus (the case this guards)");
        let mut started = None;
        rt.update(|s, _| {
            assert!(s.playback.shuffle, "Shuffle turned shuffle on");
            started = s.playback.song.clone();
            // Make it look like a local file playing, so pause and resume are recorded.
            s.playback.local = true;
            s.playback.state = crate::bridge::PlayState::Playing;
        });
        assert!(started.is_some(), "Shuffle started a song");
        press(&mut rt, Key::Text(" ".into()));
        rt.update(|s, _| {
            assert_eq!(*s.local_sent.borrow(), ["pause"], "Space paused");
            assert_eq!(s.playback.state, crate::bridge::PlayState::Playing, "no new shuffle restarted playback");
            assert_eq!(s.playback.song, started, "same song");
            // The pause lands; Space again resumes.
            s.playback.state = crate::bridge::PlayState::Paused;
        });
        press(&mut rt, Key::Text(" ".into()));
        rt.update(|s, _| assert_eq!(*s.local_sent.borrow(), ["pause", "resume"]));
    }

    /// The same for the player bar's buttons.
    #[test]
    fn space_never_presses_a_focused_player_button() {
        let mut rt = album_window();
        click(&mut rt, "shuffle");
        rt.update(|s, _| {
            s.playback.local = true;
            s.playback.state = crate::bridge::PlayState::Playing;
        });
        settle(&mut rt);
        let mut repeat = None;
        rt.update(|s, _| repeat = Some(s.playback.repeat));
        click(&mut rt, "repeat");
        let mut after = None;
        rt.update(|s, _| after = Some(s.playback.repeat));
        assert_ne!(after, repeat, "the click cycled repeat");
        press(&mut rt, Key::Text(" ".into()));
        rt.update(|s, _| {
            assert_eq!(Some(s.playback.repeat), after, "Space did not press Repeat again");
            assert_eq!(*s.local_sent.borrow(), ["pause"]);
        });
    }

    /// The focused element's key and whether Rune draws its focus ring.
    fn focus(rt: &rune::Runtime<State>) -> Option<(String, bool)> {
        rt.elements().into_iter().find(|e| e.focused).map(|e| (e.key.unwrap_or(e.path), e.focus_visible))
    }

    /// Tab (or Shift+Tab) until `key` has the focus.
    fn tab_to(rt: &mut rune::Runtime<State>, key: &str, shift: bool) {
        for _ in 0..300 {
            if focus(rt).is_some_and(|(k, _)| k == key) {
                return;
            }
            rt.key(KeyEvent { key: Key::Tab, mods: rune::Mods { shift, ..Default::default() } });
            settle(rt);
        }
        panic!("Tab never reached {key} (at {:?})", focus(rt));
    }

    fn shuffle_on(rt: &mut rune::Runtime<State>) -> bool {
        let mut on = false;
        rt.update(|s, _| on = s.playback.shuffle);
        on
    }

    /// Tab to Shuffle, Enter: it shuffles, as a click would. Keypad Enter is the same key.
    #[test]
    fn enter_presses_a_button_tab_reached() {
        let mut rt = album_window();
        assert!(!shuffle_on(&mut rt));
        tab_to(&mut rt, "shuffle", false);
        assert_eq!(focus(&rt), Some(("shuffle".into(), true)), "Tab focus draws the ring");
        press(&mut rt, Key::Enter);
        assert!(shuffle_on(&mut rt), "Enter pressed Shuffle");
        assert!(focus(&rt).is_some_and(|(k, ring)| k == "shuffle" && ring), "focus and ring stay");
        // Again, from the other side.
        rt.update(|s, _| s.playback.shuffle = false);
        tab_to(&mut rt, "back", true);
        tab_to(&mut rt, "shuffle", true);
        press(&mut rt, Key::Enter);
        assert!(shuffle_on(&mut rt), "Enter pressed Shuffle after Shift+Tab");
    }

    /// A click focuses Shuffle without a ring, and Enter then does not shuffle again; nor after
    /// a click on the button Tab had reached.
    #[test]
    fn enter_after_a_click_presses_nothing() {
        let mut rt = album_window();
        click(&mut rt, "shuffle");
        assert!(shuffle_on(&mut rt), "the click shuffled");
        assert_eq!(focus(&rt), Some(("shuffle".into(), false)), "a click focuses without a ring");
        rt.update(|s, _| s.playback.shuffle = false);
        press(&mut rt, Key::Enter);
        assert!(!shuffle_on(&mut rt), "Enter after the click did not press Shuffle");

        tab_to(&mut rt, "back", true);
        tab_to(&mut rt, "shuffle", false);
        click(&mut rt, "shuffle");
        assert_eq!(focus(&rt), Some(("shuffle".into(), false)), "the click hid the ring");
        rt.update(|s, _| s.playback.shuffle = false);
        press(&mut rt, Key::Enter);
        assert!(!shuffle_on(&mut rt), "a click on the Tab-focused button makes it the mouse's");
    }

    /// Space on a button Tab reached plays or pauses; it does not press the button.
    #[test]
    fn space_on_a_tab_focused_button_plays_or_pauses() {
        let mut rt = album_window();
        tab_to(&mut rt, "shuffle", false);
        rt.update(|s, _| {
            s.playback.local = true;
            s.playback.state = crate::bridge::PlayState::Playing;
        });
        press(&mut rt, Key::Text(" ".into()));
        assert!(!shuffle_on(&mut rt), "Space did not press Shuffle");
        rt.update(|s, _| assert_eq!(*s.local_sent.borrow(), ["pause"], "Space paused"));
        // Enter right after still presses it (Enter's own Space is not taken for play/pause).
        press(&mut rt, Key::Enter);
        assert!(shuffle_on(&mut rt));
        // (The shuffle's new song stops the local player: "stop", not another play/pause.)
        rt.update(|s, _| assert_eq!(*s.local_sent.borrow(), ["pause", "stop"], "Enter did not play or pause"));
    }

    /// Esc with nothing to close drops the focus and its ring; Enter then presses nothing.
    #[test]
    fn escape_drops_keyboard_focus() {
        let mut rt = album_window();
        tab_to(&mut rt, "shuffle", false);
        press(&mut rt, Key::Escape);
        assert_eq!(focus(&rt), None);
        press(&mut rt, Key::Enter);
        assert!(!shuffle_on(&mut rt));
    }

    /// Tab out of the search field also counts as keyboard focus.
    #[test]
    fn tab_from_a_text_field_counts() {
        let mut s = State::offline();
        s.navigate(Route::Search);
        super::super::search::focus(&mut s);
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(crate::ui::on_key);
        settle(&mut rt);
        assert_eq!(focus(&rt).map(|f| f.0).as_deref(), Some("search"));
        press(&mut rt, Key::Tab);
        let (key, ring) = focus(&rt).expect("Tab moved the focus");
        assert!(ring, "ring on {key}");
        let mut known = None;
        rt.update(|s, _| known = s.ui.kbd.key.clone());
        assert_eq!(known, Some(key), "the app knows where Tab landed");
    }

    /// In a text field Space is a space.
    #[test]
    fn space_in_the_search_field_types_a_space() {
        let mut s = State::offline();
        s.navigate(Route::Search);
        super::super::search::focus(&mut s);
        s.playback.local = true;
        s.playback.state = crate::bridge::PlayState::Playing;
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(crate::ui::on_key);
        settle(&mut rt);
        assert!(rt.has_focus(), "the search field has focus");
        for k in ["a", " ", "b"] {
            press(&mut rt, Key::Text(k.into()));
        }
        rt.update(|s, _| {
            assert_eq!(s.ui.search.text, "a b");
            assert!(s.local_sent.borrow().is_empty(), "Space did not pause");
        });
    }

    #[test]
    fn labels_read_as_linux_users_expect() {
        let s = State::offline();
        let i = BINDINGS.iter().position(|b| b.action == Action::ToggleRightPanel).unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!(label(&s, i).as_deref(), Some("Ctrl+Shift+B"));
        }
    }
}
