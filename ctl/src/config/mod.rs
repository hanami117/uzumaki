//! `uzu.toml`: the optional settings file of `uzu` (never of the app; those are in its
//! Settings, `settings.json`). The user writes it; `uzu` only reads it and never writes it. What
//! `uzu` remembers by itself (sort choices) stays in `uzu.json`, under the state directory.
//!
//! The rules:
//! - No file means today's behaviour, exactly.
//! - A mistake never stops `uzu`. Every problem becomes a [`Warning`] with its line (and a
//!   hint when one is likely), the bad line is skipped, and the rest of the file still counts.
//! - Warnings show in the interactive screens' status line, and `uzu config` lists them all.
//!
//! The reading of the file's text is in `toml`; each section has its own module.

pub mod format;
pub mod keys;
pub mod theme;
pub mod toml;

/// The commented file `uzu config --default` prints.
pub const DEFAULT_FILE: &str = include_str!("default.toml");

use std::fmt;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use keys::Keymap;
use theme::Theme;
use toml::{Document, Entry, Value};

/// The sections the file may have.
pub const SECTIONS: &[&str] = &["keys", "theme", "format", "layout", "defaults"];

/// One thing in the file `uzu` did not accept.
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    /// 1-based; 0 when it is about the file as a whole.
    pub line: usize,
    pub message: String,
}

impl Warning {
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Warning { line, message: message.into() }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 { write!(f, "line {}: {}", self.line, self.message) } else { f.write_str(&self.message) }
    }
}

/// What the file says. `Config::default()` is what no file means.
#[derive(Clone, Debug)]
pub struct Config {
    /// Where the file is (or would be).
    pub path: Option<PathBuf>,
    /// The file exists.
    pub found: bool,
    pub warnings: Vec<Warning>,
    pub theme: Theme,
    pub keys: Keymap,
    /// `[format] status_format` (`None`: [`format::DEFAULT`]) and `status_idle`.
    pub status_format: Option<String>,
    pub status_idle: String,
    /// `[theme] respect_no_color`: `NO_COLOR` turns the colours off.
    pub respect_no_color: bool,
}

/// `$XDG_CONFIG_HOME/uzumaki/uzu.toml`, else `~/.config/uzumaki/uzu.toml`: next to the app's
/// `settings.json`, but a file of its own.
pub fn default_path() -> Option<PathBuf> {
    path_in(std::env::var_os("XDG_CONFIG_HOME"), std::env::var_os("HOME"))
}

/// [`default_path`] for the given environment (a relative `XDG_CONFIG_HOME` is ignored, as the
/// XDG rules say).
pub fn path_in(xdg: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let base = xdg.map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home.map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("uzumaki").join("uzu.toml"))
}

impl Default for Config {
    fn default() -> Self {
        Config { path: None, found: false, warnings: Vec::new(), theme: Theme::default(), keys: Keymap::default(), status_format: None, status_idle: String::new(), respect_no_color: true }
    }
}

impl Config {
    /// The user's file. Missing is fine (the defaults); unreadable is a warning, not an error.
    pub fn load() -> Config {
        match default_path() {
            Some(path) => Config::load_from(&path),
            None => Config::default(),
        }
    }

    pub fn load_from(path: &Path) -> Config {
        let mut config = match std::fs::read(path) {
            Ok(bytes) => {
                let mut config = Config::parse(&String::from_utf8_lossy(&bytes));
                if std::str::from_utf8(&bytes).is_err() {
                    config.warnings.insert(0, Warning::new(0, "the file is not valid UTF-8; some characters were replaced"));
                }
                config.found = true;
                config
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => Config { warnings: vec![Warning::new(0, format!("can't read the file: {e}"))], ..Config::default() },
        };
        config.path = Some(path.to_path_buf());
        config.with_no_color(std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()))
    }

    /// `NO_COLOR` set: no colours, unless the file says `respect_no_color = false`.
    pub fn with_no_color(mut self, no_color: bool) -> Self {
        if no_color && self.respect_no_color {
            self.theme = Theme::mono();
        }
        self
    }

    /// Reads the text of a file. Never fails: problems end up in `warnings`.
    pub fn parse(text: &str) -> Config {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let doc = toml::parse(text);
        let mut w = Warnings::default();
        for p in &doc.problems {
            w.add(p.line, p.message.clone());
        }
        check_layout_of_file(&doc, &mut w);
        let mut config = Config::default();
        let mut theme = ThemeSpec::default();
        config.apply(&doc, &mut w, &mut theme);
        config.theme = theme.resolve(&mut w);
        let keys: Vec<&Entry> = doc.entries.iter().filter(|e| e.section == "keys").collect();
        config.keys = Keymap::build(&keys, &mut w);
        config.warnings = w.list;
        config.warnings.sort_by_key(|w| w.line);
        config
    }

    fn apply(&mut self, doc: &Document, w: &mut Warnings, theme: &mut ThemeSpec) {
        let mut seen: Vec<(&str, &str, usize)> = Vec::new();
        for e in &doc.entries {
            if !SECTIONS.contains(&e.section.as_str()) {
                continue; // reported once for the header
            }
            if let Some(&(_, _, first)) = seen.iter().find(|(s, k, _)| *s == e.section && *k == e.key) {
                w.add(e.line, format!("'{}' is already set on line {first}; this line wins", e.key));
            }
            seen.push((&e.section, &e.key, e.line));
            let known = known_keys(&e.section);
            if let Some(known) = known {
                if !known.contains(&e.key.as_str()) {
                    w.add(e.line, unknown_key(&e.section, &e.key, &known));
                    continue;
                }
            }
            self.take(e, w, theme);
        }
    }

    /// One accepted key of a known section.
    fn take(&mut self, e: &Entry, w: &mut Warnings, theme: &mut ThemeSpec) {
        if e.section == "format" {
            match e.key.as_str() {
                "status_format" => {
                    if let Some(text) = want_str(e, w) {
                        for problem in format::Format::parse(text).1 {
                            w.add(e.line, problem);
                        }
                        self.status_format = Some(text.to_string());
                    }
                }
                "status_idle" => {
                    if let Some(text) = want_str(e, w) {
                        self.status_idle = text.to_string();
                    }
                }
                _ => {}
            }
        }
        if e.section == "theme" {
            if e.key == "respect_no_color" {
                if let Some(b) = want_bool(e, w) {
                    self.respect_no_color = b;
                }
            } else {
                theme.take(e, w);
            }
        }
    }

    /// The line the interactive screens show: the first warning, and how many more there are.
    pub fn status_line(&self) -> Option<String> {
        let first = self.warnings.first()?;
        let more = self.warnings.len() - 1;
        let name = self.path.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or("uzu.toml");
        let tail = if more > 0 { format!(" (+{more} more; 'uzu config' lists them)") } else { String::new() };
        Some(format!("{name} {first}{tail}"))
    }
}

/// The keys each section knows, `None` for a section this build has no schema for yet.
fn known_keys(section: &str) -> Option<Vec<&'static str>> {
    match section {
        // Any key: the actions are checked (with hints) when the keymap is built.
        "keys" => None,
        "theme" => Some(vec!["palette", "accent", "number", "album", "time", "paused", "respect_no_color"]),
        "format" => Some(vec!["status_format", "status_idle"]),
        "layout" => Some(vec!["start_screen", "start_view", "lyrics", "artist_width", "title_width", "album_width", "show_artist", "show_album"]),
        "defaults" => Some(vec!["sort_playlists", "sort_albums", "sort_songs", "sort_search", "notifications"]),
        _ => None,
    }
}

/// Warnings being collected (the same message for the same line is kept once).
#[derive(Default)]
pub struct Warnings {
    pub list: Vec<Warning>,
}

impl Warnings {
    pub fn add(&mut self, line: usize, message: impl Into<String>) {
        let warning = Warning::new(line, message);
        if !self.list.contains(&warning) {
            self.list.push(warning);
        }
    }
}

/// Unknown sections, once each.
fn check_layout_of_file(doc: &Document, w: &mut Warnings) {
    for (name, line) in &doc.sections {
        if !SECTIONS.contains(&name.as_str()) {
            let hint = did_you_mean(name, SECTIONS).map(|s| format!("; did you mean [{s}]?")).unwrap_or_else(|| format!("; the sections are {}", list_of(SECTIONS, "[", "]")));
            w.add(*line, format!("unknown section [{name}]{hint} Its lines are ignored."));
        }
    }
    for e in &doc.entries {
        if e.section.is_empty() {
            let home = home_of(&e.key, "");
            let hint = match home {
                Some(s) => format!("put it under [{s}]"),
                None => "keys go under a [section]".to_string(),
            };
            w.add(e.line, format!("'{}' is outside any section; {hint}", e.key));
        }
    }
}

/// The section a key belongs to (other than `not`).
fn home_of(key: &str, not: &str) -> Option<&'static str> {
    if not != "keys" && keys::ACTIONS.iter().any(|a| a.name == key) {
        return Some("keys");
    }
    SECTIONS.iter().find(|s| **s != not && known_keys(s).is_some_and(|k| k.contains(&key))).copied()
}

fn unknown_key(section: &str, key: &str, known: &[&str]) -> String {
    if let Some(s) = did_you_mean(key, known) {
        return format!("unknown key '{key}' in [{section}]; did you mean '{s}'?");
    }
    if let Some(other) = home_of(key, section) {
        return format!("'{key}' belongs under [{other}], not [{section}]");
    }
    format!("unknown key '{key}' in [{section}]; the keys there are {}", list_of(known, "", ""))
}

pub fn list_of(items: &[&str], open: &str, close: &str) -> String {
    items.iter().map(|i| format!("{open}{i}{close}")).collect::<Vec<_>>().join(", ")
}

// ------------------------------------------------------------------------------------- hints

/// The candidate closest to `word`, when it is close enough to be a typo (not just a different
/// word). Case, `-` and `_` don't count as differences.
pub fn did_you_mean<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let norm = |s: &str| s.to_lowercase().replace('-', "_");
    let word = norm(word);
    if word.is_empty() {
        return None;
    }
    let mut best: Option<(&str, usize)> = None;
    for &c in candidates {
        let cn = norm(c);
        if cn == word {
            return Some(c);
        }
        let mut d = edit_distance(&word, &cn);
        // A prefix ("seek" for "seek_forward") is a good guess even when far in letters.
        if word.len() >= 3 && (cn.starts_with(&word) || word.starts_with(&cn)) {
            d = d.min(1);
        }
        let limit = if word.chars().count() <= 4 { 1 } else { 2 };
        if d <= limit && best.is_none_or(|(_, b)| d < b) {
            best = Some((c, d));
        }
    }
    best.map(|(c, _)| c)
}

/// Levenshtein distance, in chars.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

// ---------------------------------------------------------------------------- value helpers

/// The entry's value as a string, or a warning saying what was expected.
pub fn want_str<'a>(e: &'a Entry, w: &mut Warnings) -> Option<&'a str> {
    match &e.value {
        Value::Str(s) => Some(s),
        other => {
            w.add(e.line, format!("'{}' needs text in quotes (got {})", e.key, other.kind()));
            None
        }
    }
}

pub fn want_bool(e: &Entry, w: &mut Warnings) -> Option<bool> {
    match &e.value {
        Value::Bool(b) => Some(*b),
        other => {
            w.add(e.line, format!("'{}' needs true or false (got {})", e.key, other.kind()));
            None
        }
    }
}

pub fn want_int(e: &Entry, w: &mut Warnings) -> Option<i64> {
    match &e.value {
        Value::Int(n) => Some(*n),
        other => {
            w.add(e.line, format!("'{}' needs a whole number (got {})", e.key, other.kind()));
            None
        }
    }
}

// ---------------------------------------------------------------------------------- [theme]

/// What `[theme]` said, resolved once the whole section is read (the palette first, then the
/// roles, whatever order the lines are in).
#[derive(Default)]
struct ThemeSpec {
    palette: Option<(usize, String)>,
    roles: Vec<(&'static str, Option<Color>)>,
}

impl ThemeSpec {
    fn take(&mut self, e: &Entry, w: &mut Warnings) {
        if e.key == "palette" {
            if let Some(name) = want_str(e, w) {
                self.palette = Some((e.line, name.to_string()));
            }
            return;
        }
        let Some(role) = theme::ROLES.iter().find(|r| **r == e.key) else { return };
        let colour = match &e.value {
            Value::Str(text) => theme::parse_colour(text),
            Value::Int(n) => u8::try_from(*n).map(|n| Some(Color::Indexed(n))).map_err(|_| format!("{n} is not a colour; numbers go from 0 to 255")),
            other => Err(format!("'{}' needs a colour in quotes (got {})", e.key, other.kind())),
        };
        match colour {
            Ok(c) => self.roles.push((role, c)),
            Err(message) => w.add(e.line, message),
        }
    }

    fn resolve(self, w: &mut Warnings) -> Theme {
        let mut theme = Theme::default();
        if let Some((line, name)) = &self.palette {
            match Theme::palette(&name.to_lowercase()) {
                Some(t) => theme = t,
                None => {
                    let hint = did_you_mean(name, theme::PALETTES).map(|p| format!("did you mean '{p}'?")).unwrap_or_else(|| format!("the palettes are {}", list_of(theme::PALETTES, "'", "'")));
                    w.add(*line, format!("unknown palette '{name}'; {hint}"));
                }
            }
        }
        for (role, colour) in self.roles {
            theme.set(role, colour);
        }
        theme
    }
}

impl Config {
    /// The format `uzu status --line` uses.
    pub fn line_format(&self) -> &str {
        self.status_format.as_deref().unwrap_or(format::DEFAULT)
    }
}
