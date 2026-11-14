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
//! The reading of the file's text is in `reader` (the `toml` crate); each section has its own module.

pub mod format;
pub mod keys;
pub mod theme;
pub mod reader;

const DEFAULT_TEMPLATE: &str = include_str!("default.toml");

/// The commented file `uzu config --default` prints: every setting, commented out, at its
/// default. The `[keys]` lines come from the action table, so they can't drift from the code.
pub fn default_file() -> String {
    let width = keys::ACTIONS.iter().map(|a| a.name.len() + 4 + a.keys.iter().map(|k| k.len() + 4).sum::<usize>()).max().unwrap_or(0);
    let lines: Vec<String> = keys::ACTIONS
        .iter()
        .map(|a| {
            let list = a.keys.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ");
            let left = if a.keys.len() == 1 { format!("# {} = {list}", a.name) } else { format!("# {} = [{list}]", a.name) };
            format!("{left:<w$}  # {}", a.about, w = width.clamp(0, 28))
        })
        .collect();
    let name_w = theme::COLOURS.iter().chain(theme::STYLES).map(|r| r.name.len()).max().unwrap_or(0);
    let theme_lines: Vec<String> = theme::COLOURS.iter().chain(theme::STYLES).map(|r| format!("{:<w$}  # {}", format!("# {} = \"{}\"", r.name, r.default), r.about, w = name_w + 14)).collect();
    DEFAULT_TEMPLATE.replace("@KEYS@", &lines.join("\n")).replace("@THEME@", &theme_lines.join("\n"))
}

use std::fmt;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use keys::Keymap;
use crate::tui::sort::{Prefs, View};
use theme::Theme;
use reader::{Document, Entry};
use toml::Value;

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

/// Where solo puts the lyrics (`[layout] lyrics`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// Beside the cover when there is room, else under the song (what `uzu` always did).
    #[default]
    Auto,
    /// Beside the cover only; a terminal too narrow for that shows solo without lyrics.
    Beside,
    /// Always under the cover and the song.
    Under,
}

/// The song table's columns: how the width is shared (relative weights) and which show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Columns {
    pub artist: u16,
    pub title: u16,
    pub album: u16,
    pub show_artist: bool,
    pub show_album: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Columns { artist: 3, title: 5, album: 3, show_artist: true, show_album: true }
    }
}

/// `[layout]`.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutCfg {
    /// 0-based index into the screens (queue, playlists, search, albums, lyrics).
    pub start_screen: usize,
    /// `normal`, `showcase` or `solo` (the queue screen's view); `None` is normal.
    pub start_view: Option<String>,
    pub lyrics: Placement,
    pub columns: Columns,
}

impl Default for LayoutCfg {
    fn default() -> Self {
        LayoutCfg { start_screen: 0, start_view: None, lyrics: Placement::Auto, columns: Columns::default() }
    }
}

pub const SCREEN_NAMES: &[&str] = &["queue", "playlists", "search", "albums", "lyrics"];
pub const VIEW_NAMES: &[&str] = &["normal", "showcase", "solo"];
pub const PLACEMENTS: &[&str] = &["auto", "beside", "under"];

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
    pub layout: LayoutCfg,
    /// `[defaults] sort_*`: the sort a view starts with when none was remembered.
    pub sorts: Prefs,
    /// `[defaults] notifications`: the short messages ("Paused", "Cover: on"…) show.
    pub notifications: bool,
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
        Config { path: None, found: false, warnings: Vec::new(), theme: Theme::default(), keys: Keymap::default(), layout: LayoutCfg::default(), sorts: Prefs::default(), notifications: true, status_format: None, status_idle: String::new(), respect_no_color: true }
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
            // No colour, but a look like bold or underline is not a colour: it stays.
            self.theme = Theme { selection_style: self.theme.selection_style, selection_inactive_style: self.theme.selection_inactive_style, ..Theme::mono() };
        }
        self
    }

    /// Reads the text of a file. Never fails: problems end up in `warnings`.
    pub fn parse(text: &str) -> Config {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let doc = reader::parse(text);
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
        for e in &doc.entries {
            if !SECTIONS.contains(&e.section.as_str()) {
                continue; // reported once for the header
            }
            if let Some(known) = known_keys(&e.section)
                && !known.contains(&e.key.as_str())
            {
                w.add(e.line, unknown_key(&e.section, &e.key, &known));
                continue;
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
        match e.section.as_str() {
            "layout" => self.take_layout(e, w),
            "defaults" => self.take_defaults(e, w),
            _ => {}
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

    fn take_layout(&mut self, e: &Entry, w: &mut Warnings) {
        let l = &mut self.layout;
        match e.key.as_str() {
            "start_screen" => {
                // A screen's number works too.
                if let Value::Integer(n) = &e.value
                    && (1..=5).contains(n)
                {
                    l.start_screen = *n as usize - 1;
                    return;
                }
                if let Some(i) = want_choice(e, w, SCREEN_NAMES) {
                    l.start_screen = i;
                }
            }
            "start_view" => {
                if let Some(i) = want_choice(e, w, VIEW_NAMES) {
                    l.start_view = (i > 0).then(|| VIEW_NAMES[i].to_string());
                }
            }
            "lyrics" => {
                if let Some(i) = want_choice(e, w, PLACEMENTS) {
                    l.lyrics = [Placement::Auto, Placement::Beside, Placement::Under][i];
                }
            }
            "artist_width" | "title_width" | "album_width" => {
                let Some(n) = want_int(e, w) else { return };
                if !(1..=20).contains(&n) {
                    w.add(e.line, format!("'{}' is a share of the width, a whole number from 1 to 20 (got {n})", e.key));
                    return;
                }
                let n = n as u16;
                match e.key.as_str() {
                    "artist_width" => l.columns.artist = n,
                    "title_width" => l.columns.title = n,
                    _ => l.columns.album = n,
                }
            }
            "show_artist" => {
                if let Some(b) = want_bool(e, w) {
                    l.columns.show_artist = b;
                }
            }
            "show_album" => {
                if let Some(b) = want_bool(e, w) {
                    l.columns.show_album = b;
                }
            }
            _ => {}
        }
    }

    fn take_defaults(&mut self, e: &Entry, w: &mut Warnings) {
        let view = match e.key.as_str() {
            "sort_playlists" => View::Playlists,
            "sort_albums" => View::Albums,
            "sort_songs" => View::Songs,
            "sort_search" => View::Search,
            "notifications" => {
                if let Some(b) = want_bool(e, w) {
                    self.notifications = b;
                }
                return;
            }
            _ => return,
        };
        let Some(text) = want_str(e, w) else { return };
        match crate::tui::sort::parse_default(view, text) {
            Ok(sort) => self.sorts.set(view, sort),
            Err(message) => w.add(e.line, message),
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
        "theme" => Some(["palette", "respect_no_color"].into_iter().chain(theme::COLOURS.iter().chain(theme::STYLES).map(|r| r.name)).collect()),
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

/// Edit distance in chars: insert, delete, change, or swap two neighbours (a typo like
/// `yaer`), each one step.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, v) in d[0].iter_mut().enumerate() {
        *v = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

// ---------------------------------------------------------------------------- value helpers

/// The entry's value as a string, or a warning saying what was expected.
pub fn want_str<'a>(e: &'a Entry, w: &mut Warnings) -> Option<&'a str> {
    match &e.value {
        Value::String(s) => Some(s),
        other => {
            w.add(e.line, format!("'{}' needs text in quotes (got {})", e.key, reader::kind(other)));
            None
        }
    }
}

/// One of `choices` (case-insensitive), or a warning that lists them with a hint.
pub fn want_choice(e: &Entry, w: &mut Warnings, choices: &[&str]) -> Option<usize> {
    let text = want_str(e, w)?;
    let lower = text.trim().to_lowercase();
    if let Some(i) = choices.iter().position(|c| *c == lower) {
        return Some(i);
    }
    let hint = match did_you_mean(&lower, choices) {
        Some(c) => format!("did you mean \"{c}\"?"),
        None => format!("use {}", list_of(choices, "\"", "\"")),
    };
    w.add(e.line, format!("'{text}' is not a choice for '{}'; {hint}", e.key));
    None
}

pub fn want_bool(e: &Entry, w: &mut Warnings) -> Option<bool> {
    match &e.value {
        Value::Boolean(b) => Some(*b),
        other => {
            w.add(e.line, format!("'{}' needs true or false (got {})", e.key, reader::kind(other)));
            None
        }
    }
}

pub fn want_int(e: &Entry, w: &mut Warnings) -> Option<i64> {
    match &e.value {
        Value::Integer(n) => Some(*n),
        other => {
            w.add(e.line, format!("'{}' needs a whole number (got {})", e.key, reader::kind(other)));
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
    styles: Vec<(&'static str, Option<ratatui::style::Modifier>)>,
}

impl ThemeSpec {
    fn take(&mut self, e: &Entry, w: &mut Warnings) {
        if e.key == "palette" {
            if let Some(name) = want_str(e, w) {
                self.palette = Some((e.line, name.to_string()));
            }
            return;
        }
        if let Some(role) = theme::STYLES.iter().find(|r| r.name == e.key) {
            match &e.value {
                Value::String(text) => match theme::parse_style(text) {
                    Ok(m) => self.styles.push((role.name, m)),
                    Err(message) => w.add(e.line, message),
                },
                other => w.add(e.line, format!("'{}' needs text like \"reversed|bold\" (got {})", e.key, reader::kind(other))),
            }
            return;
        }
        let Some(role) = theme::COLOURS.iter().find(|r| r.name == e.key) else { return };
        let colour = match &e.value {
            Value::String(text) => theme::parse_colour(text),
            Value::Integer(n) => u8::try_from(*n).map(|n| Some(Color::Indexed(n))).map_err(|_| format!("{n} is not a colour; numbers go from 0 to 255")),
            other => Err(format!("'{}' needs a colour in quotes (got {})", e.key, reader::kind(other))),
        };
        match colour {
            Ok(c) => self.roles.push((role.name, c)),
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
        let set: Vec<&str> = self.roles.iter().map(|(r, _)| *r).collect();
        for (role, colour) in &self.roles {
            theme.set(role, *colour);
        }
        // What follows another role, unless the file sets it too.
        if set.contains(&"accent") {
            for role in theme::FOLLOW_ACCENT.iter().filter(|r| !set.contains(r)) {
                theme.set(role, theme.accent);
            }
        }
        if set.contains(&"border") && !set.contains(&"border_focused") {
            theme.border_focused = theme.border;
        }
        for (key, style) in self.styles {
            theme.set_style(key, style);
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
