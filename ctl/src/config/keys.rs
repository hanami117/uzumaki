//! `[keys]`: remapping the keys of the interactive screens.
//!
//! Each action has a name and default keys (`next = "n"`). In the file, `next = ["n", "ctrl+n"]`
//! gives it new keys. A list **replaces** the defaults of that action (so `next = []` turns the
//! action off, and `down = ["j", "down", "ctrl+n"]` adds a key). Actions you don't mention keep
//! theirs.
//!
//! How it works: the screens' key handling is written for the default keys, so the keymap sits
//! in front of it. A key you bound is turned into the action's first default key before the
//! screens see it; a default key of an action you remapped is dropped. Typing (a filter, the
//! search box) and the help and sort popups are not touched: they take every key as it comes.
//! Arrows (Left/Right, Tab, `h`) keep their meaning on the screens where they switch panes or
//! seek, and Ctrl+C always quits.

use std::collections::{HashMap, HashSet};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::reader::Entry;
use toml::Value;
use super::{Warnings, did_you_mean, list_of};

/// One remappable action.
pub struct Action {
    pub name: &'static str,
    /// The default keys, the first is the one the screens' code answers to.
    pub keys: &'static [&'static str],
    pub about: &'static str,
}

const fn a(name: &'static str, keys: &'static [&'static str], about: &'static str) -> Action {
    Action { name, keys, about }
}

/// Every action, in the order the default file lists them.
pub const ACTIONS: &[Action] = &[
    a("play_pause", &["space"], "play or pause"),
    a("next", &["n"], "next song"),
    a("prev", &["p"], "previous song"),
    a("seek_forward", &["f"], "seek forward 10 s"),
    a("seek_back", &["b"], "seek back 10 s"),
    a("volume_up", &["+", "="], "volume up 5%"),
    a("volume_down", &["-"], "volume down 5%"),
    a("shuffle", &["s"], "shuffle on or off"),
    a("repeat", &["r"], "repeat: off, all, one"),
    a("autoplay", &["A"], "AutoPlay (∞) on or off"),
    a("goto_queue", &["1"], "the Queue screen"),
    a("goto_playlists", &["2"], "the Playlists screen"),
    a("goto_search", &["3"], "the Search screen"),
    a("goto_albums", &["4"], "the Albums screen"),
    a("goto_lyrics", &["5"], "the Lyrics screen"),
    a("cover", &["i"], "show or hide the cover"),
    a("view", &["F"], "view: normal, showcase, solo"),
    a("lyrics", &["l"], "in solo: show or hide the lyrics (on screens 2 and 4 this key also switches panes)"),
    a("words", &["w"], "lyrics: word by word or line by line"),
    a("help", &["?"], "the help"),
    a("quit", &["q"], "quit"),
    a("up", &["k", "up"], "move up"),
    a("down", &["j", "down"], "move down"),
    a("page_up", &["pageup"], "a page up"),
    a("page_down", &["pagedown"], "a page down"),
    a("top", &["g", "home"], "first row"),
    a("bottom", &["G", "end"], "last row"),
    a("select", &["enter"], "play or open the selected row"),
    a("back", &["esc"], "clear the filter, step back, or quit from the queue"),
    a("filter", &["/"], "filter (on Search: type a new query)"),
    a("sort", &[","], "open the sort menu"),
    a("playing", &["o"], "select the playing song"),
    a("remove", &["d", "delete"], "remove the selected song from Up Next"),
    a("move_down", &["J"], "move the selected song down in Up Next"),
    a("move_up", &["K"], "move the selected song up in Up Next"),
    a("play_all", &["P"], "play the whole playlist or album"),
    a("reload", &["R"], "reload the list"),
    a("add", &["a"], "Search: add the result to the queue"),
    a("play_next", &["N"], "Search: play the result next"),
];

pub fn action_names() -> Vec<&'static str> {
    ACTIONS.iter().map(|a| a.name).collect()
}

/// Keys that keep meaning something on some screens, whatever is bound: binding one of them
/// to an action shadows it there.
const FIXED: &[(&str, &str)] = &[
    ("left", "seeks on the queue, search and lyrics screens and switches panes on 2 and 4"),
    ("right", "seeks on the queue, search and lyrics screens and switches panes on 2 and 4"),
    ("tab", "switches panes on screens 2 and 4"),
    ("shift+tab", "switches panes on screens 2 and 4"),
    ("h", "switches panes on screens 2 and 4"),
];

// ------------------------------------------------------------------------------------ chords

/// A key as the keymap sees it: the code and whether Ctrl or Alt are down (Shift is already in
/// an uppercase letter, so it is not kept).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

impl Chord {
    #[cfg(test)]
    pub const fn plain(code: KeyCode) -> Chord {
        Chord { code, ctrl: false, alt: false }
    }

    pub fn of(ev: &KeyEvent) -> Chord {
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let code = match ev.code {
            // Ctrl+Shift+N and Ctrl+N are one key here.
            KeyCode::Char(c) if ctrl => KeyCode::Char(c.to_ascii_lowercase()),
            // Shift+Tab arrives as BackTab on most terminals, as Tab+SHIFT on some.
            KeyCode::Tab if ev.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
            other => other,
        };
        Chord { code, ctrl, alt }
    }

    /// The event the screens' code expects for this chord.
    pub fn event(self) -> KeyEvent {
        KeyEvent::new(self.code, KeyModifiers::NONE)
    }
}

const NAMES: &[(&str, KeyCode)] = &[
    ("space", KeyCode::Char(' ')),
    ("enter", KeyCode::Enter),
    ("esc", KeyCode::Esc),
    ("tab", KeyCode::Tab),
    ("backspace", KeyCode::Backspace),
    ("delete", KeyCode::Delete),
    ("insert", KeyCode::Insert),
    ("up", KeyCode::Up),
    ("down", KeyCode::Down),
    ("left", KeyCode::Left),
    ("right", KeyCode::Right),
    ("home", KeyCode::Home),
    ("end", KeyCode::End),
    ("pageup", KeyCode::PageUp),
    ("pagedown", KeyCode::PageDown),
    ("plus", KeyCode::Char('+')),
    ("minus", KeyCode::Char('-')),
    ("comma", KeyCode::Char(',')),
];

const ALIASES: &[(&str, &str)] = &[("return", "enter"), ("escape", "esc"), ("del", "delete"), ("ins", "insert"), ("bs", "backspace"), ("pgup", "pageup"), ("pgdn", "pagedown"), ("pgdown", "pagedown")];

/// A key as written in the file: one character (`n`, `N`, `?`), a name (`space`, `enter`, `esc`,
/// `tab`, `up`, `pageup`, `f5`, …), with `ctrl+`, `alt+` or `shift+` in front. Letters are
/// case-sensitive (`N` is Shift+n); names and modifiers are not.
pub fn parse_key(text: &str) -> Result<Chord, String> {
    let original = text.trim();
    let mut rest = original;
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    loop {
        let lower = rest.to_lowercase();
        let strip = |p: &str| (lower.starts_with(p) && rest.len() > p.len()).then(|| &rest[p.len()..]);
        if let Some(r) = strip("ctrl+").or_else(|| strip("control+")) {
            ctrl = true;
            rest = r;
        } else if let Some(r) = strip("alt+") {
            alt = true;
            rest = r;
        } else if let Some(r) = strip("shift+") {
            shift = true;
            rest = r;
        } else {
            break;
        }
    }
    if rest.is_empty() {
        return Err("a key can't be empty".into());
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => {
            if shift {
                if c.is_alphabetic() {
                    KeyCode::Char(c.to_uppercase().next().unwrap_or(c))
                } else {
                    return Err(format!("'{original}': shift only goes with a letter or tab (write the character itself, like '?' or '+')"));
                }
            } else {
                KeyCode::Char(c)
            }
        }
        _ => {
            let lower = rest.to_lowercase();
            let lower = ALIASES.iter().find(|(a, _)| *a == lower).map_or(lower.as_str(), |(_, n)| n);
            if let Some((_, code)) = NAMES.iter().find(|(n, _)| *n == lower) {
                if shift && *code == KeyCode::Tab {
                    KeyCode::BackTab
                } else if shift {
                    return Err(format!("'{original}': shift only goes with a letter or tab"));
                } else {
                    *code
                }
            } else if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)) {
                KeyCode::F(n)
            } else {
                let mut names: Vec<&str> = NAMES.iter().map(|(n, _)| *n).collect();
                names.extend(ALIASES.iter().map(|(a, _)| *a));
                let hint = match did_you_mean(lower, &names) {
                    Some(n) => format!("did you mean '{n}'?"),
                    None => format!("a key is one character, or a name like {}, with ctrl+ or alt+ in front", list_of(&["space", "enter", "esc", "tab", "up", "pageup"], "'", "'")),
                };
                return Err(format!("'{original}' is not a key; {hint}"));
            }
        }
    };
    let code = if ctrl { if let KeyCode::Char(c) = code { KeyCode::Char(c.to_ascii_lowercase()) } else { code } } else { code };
    Ok(Chord { code, ctrl, alt })
}

// ------------------------------------------------------------------------------------ keymap

/// The user's bindings, ready for use. `Keymap::default()` changes nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    /// A bound key → the first default key of its action.
    bound: HashMap<Chord, Chord>,
    /// Default keys of remapped actions that nothing else claimed: they do nothing.
    blocked: HashSet<Chord>,
    /// Default key → the user's keys for that action as written (empty: unbound), for the help.
    shown: HashMap<Chord, (usize, Vec<String>)>,
}

impl Keymap {
    /// No remapping at all.
    pub fn is_default(&self) -> bool {
        self.bound.is_empty() && self.blocked.is_empty()
    }

    /// What the screens should see for a key press: the key itself, another key standing for
    /// the action it is bound to, or nothing (the key was unbound).
    pub fn translate(&self, ev: KeyEvent) -> Option<KeyEvent> {
        if self.is_default() {
            return Some(ev);
        }
        let chord = Chord::of(&ev);
        if let Some(target) = self.bound.get(&chord) {
            return Some(target.event());
        }
        // With a keymap, Ctrl and Alt keys that were not bound do nothing (the screens would
        // otherwise read Ctrl+N as plain n).
        if self.blocked.contains(&chord) || chord.ctrl || chord.alt {
            return None;
        }
        Some(ev)
    }

    /// One key as the help shows it (`n`, `Space`, `PgUp`, …) → what the user bound instead.
    /// `None` for keys of actions not remapped. `taken` collects the actions already shown in
    /// this row, so `Up Down  j k` with `down` remapped shows the new key once.
    fn relabel_one(&self, token: &str, taken: &mut Vec<usize>) -> Option<String> {
        let (action, keys) = self.shown.get(&parse_key(token).ok()?)?;
        if taken.contains(action) {
            return Some(String::new());
        }
        taken.push(*action);
        Some(if keys.is_empty() { "–".to_string() } else { keys.join("/") })
    }

    /// A help row's key column with the user's keys in place of the defaults.
    pub fn relabel(&self, keys: &str) -> String {
        if self.is_default() {
            return keys.to_string();
        }
        let mut taken = Vec::new();
        let mut out = String::new();
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut String, taken: &mut Vec<usize>| {
            if !word.is_empty() {
                match self.relabel_one(word, taken) {
                    Some(s) => out.push_str(&s),
                    None => out.push_str(word),
                }
                word.clear();
            }
        };
        for c in keys.chars() {
            if c == ' ' {
                flush(&mut word, &mut out, &mut taken);
                out.push(c);
            } else {
                word.push(c);
            }
        }
        flush(&mut word, &mut out, &mut taken);
        out
    }

    /// A bottom-border hint (`n/p next/prev`, `Space play/pause`): the key part is relabelled.
    pub fn relabel_hint(&self, hint: &str) -> String {
        if self.is_default() {
            return hint.to_string();
        }
        let (keys, rest) = hint.split_once(' ').map_or((hint, ""), |(k, r)| (k, r));
        let mut taken = Vec::new();
        let keys: Vec<String> = keys
            .split('/')
            .map(|k| match self.relabel_one(k, &mut taken) {
                Some(s) => s,
                None => k.to_string(),
            })
            .filter(|k| !k.is_empty())
            .collect();
        if rest.is_empty() { keys.join("/") } else { format!("{} {rest}", keys.join("/")) }
    }

    /// Builds the keymap from the `[keys]` entries of the file.
    pub fn build(entries: &[&Entry], w: &mut Warnings) -> Keymap {
        struct Binding {
            action: usize,
            line: usize,
            chord: Chord,
        }
        let mut bindings: Vec<Binding> = Vec::new();
        // action index → the keys as written (None: untouched).
        let mut written: Vec<Option<Vec<String>>> = vec![None; ACTIONS.len()];
        let names = action_names();
        for e in entries {
            let Some(index) = ACTIONS.iter().position(|a| a.name == e.key.to_lowercase().replace('-', "_")) else {
                w.add(e.line, unknown_action(e, &names));
                continue;
            };
            let Some(specs) = specs_of(e, w) else { continue };
            if written[index].is_some() {
                // `Warnings` already said the key is repeated; the later line replaces the earlier.
                bindings.retain(|b| b.action != index);
            }
            let mut good: Vec<String> = Vec::new();
            for spec in &specs {
                let chord = match parse_key(spec) {
                    Ok(c) => c,
                    Err(message) => {
                        w.add(e.line, message);
                        continue;
                    }
                };
                if chord == (Chord { code: KeyCode::Char('c'), ctrl: true, alt: false }) {
                    w.add(e.line, "Ctrl+C always quits; it can't be bound to anything else");
                    continue;
                }
                if let Some(first) = bindings.iter().find(|b| b.chord == chord && b.action != index) {
                    w.add(e.line, format!("'{spec}' is already used for '{}' (line {}); it stays there", ACTIONS[first.action].name, first.line));
                    continue;
                }
                if good.iter().any(|g| parse_key(g).ok() == Some(chord)) {
                    w.add(e.line, format!("'{spec}' is listed twice for '{}'", e.key));
                    continue;
                }
                bindings.push(Binding { action: index, line: e.line, chord });
                good.push(spec.clone());
            }
            // A list whose every key was bad leaves the action as it was, not unbound by accident.
            if good.is_empty() && !specs.is_empty() {
                continue;
            }
            written[index] = Some(good);
        }

        let mut map = Keymap::default();
        for (index, keys) in written.iter().enumerate() {
            let Some(keys) = keys else { continue };
            let action = &ACTIONS[index];
            let defaults: Vec<Chord> = action.keys.iter().filter_map(|k| parse_key(k).ok()).collect();
            let target = defaults[0];
            for b in bindings.iter().filter(|b| b.action == index) {
                map.bound.insert(b.chord, target);
            }
            map.blocked.extend(defaults.iter().copied());
            for d in &defaults {
                map.shown.insert(*d, (index, keys.clone()));
            }
            let _ = target;
        }
        // A bound key stops being "blocked" (it is bound).
        let bound: Vec<Chord> = map.bound.keys().copied().collect();
        for c in bound {
            map.blocked.remove(&c);
        }
        // Warn where a new key takes over a default key of an action the user did not touch, or
        // a key that keeps a meaning on some screens.
        for b in &bindings {
            let spec_of = |c: &Chord| written[b.action].as_ref().and_then(|ks| ks.iter().find(|k| parse_key(k).ok().as_ref() == Some(c)).cloned()).unwrap_or_default();
            let spec = spec_of(&b.chord);
            for (other, action) in ACTIONS.iter().enumerate() {
                if other == b.action || written[other].is_some() {
                    continue;
                }
                if action.keys.iter().filter_map(|k| parse_key(k).ok()).any(|c| c == b.chord) {
                    w.add(b.line, format!("'{spec}' used to be '{}'; it now does '{}', and '{}' loses that key", action.name, ACTIONS[b.action].name, action.name));
                }
            }
            if let Some((name, what)) = FIXED.iter().find(|(n, _)| parse_key(n).ok() == Some(b.chord)) {
                w.add(b.line, format!("'{name}' also {what}; the new binding wins"));
            }
        }
        map
    }
}

/// The key names a `[keys]` value gives: one string, a list of strings, or a digit.
fn specs_of(e: &Entry, w: &mut Warnings) -> Option<Vec<String>> {
    let one = |v: &Value, w: &mut Warnings| match v {
        Value::String(s) => Some(s.clone()),
        Value::Integer(n) if (0..=9).contains(n) => Some(n.to_string()),
        other => {
            w.add(e.line, format!("'{}' needs keys in quotes, like \"n\" or [\"n\", \"ctrl+n\"] (got {})", e.key, super::reader::kind(other)));
            None
        }
    };
    match &e.value {
        Value::Array(items) => {
            let specs: Vec<Option<String>> = items.iter().map(|v| one(v, w)).collect();
            // A list with a bad item is still used for its good ones.
            Some(specs.into_iter().flatten().collect())
        }
        v => one(v, w).map(|s| vec![s]),
    }
}

fn unknown_action(e: &Entry, names: &[&str]) -> String {
    // `n = "next"`: the way round people write it in other programs.
    if let Value::String(v) = &e.value
        && names.contains(&v.as_str())
    {
        return format!("write it as {v} = \"{}\" (the action first, then its keys)", e.key);
    }
    match did_you_mean(&e.key, names) {
        Some(n) => format!("unknown action '{}' in [keys]; did you mean '{n}'?", e.key),
        None => format!("unknown action '{}' in [keys]; 'uzu config --default' lists them all", e.key),
    }
}
