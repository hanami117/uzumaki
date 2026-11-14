//! `[theme]`: the colours of the interactive screens. Every colour has a role (`COLOURS`); a role
//! with no colour (`None`) is drawn in the terminal's own foreground.
//!
//! - `palette`: `"default"` (the coral accent and the ANSI colours), `"terminal"` (only the 16
//!   colours of the terminal's own theme, no fixed RGB) or `"mono"` (no colour at all).
//! - The roles, one key each, go on top of the palette. `playing`, `progress`, `lyrics_current`
//!   and `warning` follow `accent` unless set (`FOLLOW_ACCENT`); `border_focused` follows `border`.
//! - `selection_style` and `selection_inactive_style` are modifiers (`reversed|bold|underline`).
//! - `NO_COLOR` (set, not empty) turns every colour off, unless `respect_no_color = false`.
//!
//! A colour is `"#ff7a8a"` or `"#f78"`, a name (`red`, `light-blue`, `dark-gray`, …), a number
//! 0-255 (the terminal's 256-colour table), or `"none"` for the terminal's foreground.

use ratatui::style::{Color, Modifier};

/// The accent of the screens, as `uzu` has always drawn it.
pub const CORAL: Color = Color::Rgb(255, 122, 138);

/// What each colour paints. `None` is the terminal's own foreground (or background), except
/// where the field says otherwise (`muted`, the selection).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// The current tab, key names in the help, the search prompt.
    pub accent: Option<Color>,
    /// The `#` column.
    pub number: Option<Color>,
    /// The Album column.
    pub album: Option<Color>,
    /// The Time column.
    pub time: Option<Color>,
    /// The "Paused" state.
    pub paused: Option<Color>,
    /// The playing song: its row, its title in the player, the "Playing" word.
    pub playing: Option<Color>,
    /// The song title in the player and the panel titles ("Now Playing", "Up Next"), on top of bold.
    pub title: Option<Color>,
    /// The column headers of the song tables and the list panes, on top of bold.
    pub header: Option<Color>,
    /// Secondary text. `None` is the terminal's "dim" look; a colour is used instead of it.
    pub muted: Option<Color>,
    /// Borders, rules, scrollbars and the divider.
    pub border: Option<Color>,
    /// The border of the box that has the keys: the help and the sort menu.
    pub border_focused: Option<Color>,
    /// The cursor row in the pane that has the focus: its text and its background.
    pub selection_fg: Option<Color>,
    pub selection_bg: Option<Color>,
    /// The cursor row in a pane that does not have the focus.
    pub selection_inactive_fg: Option<Color>,
    pub selection_inactive_bg: Option<Color>,
    /// The finished part of the progress bar (while playing).
    pub progress: Option<Color>,
    /// The rest of the progress bar, its track.
    pub progress_bg: Option<Color>,
    /// The words being sung, in the lyrics.
    pub lyrics_current: Option<Color>,
    /// The other lyrics lines (the ones sung are also dimmed).
    pub lyrics_other: Option<Color>,
    /// The line under the tabs: what you type, the messages, the screen's own line.
    pub status_fg: Option<Color>,
    pub status_bg: Option<Color>,
    /// "Uzumaki went away", and the problems found in this file.
    pub warning: Option<Color>,
    /// An error from Uzumaki.
    pub error: Option<Color>,
    /// How the cursor row looks besides its colours. `None` (auto) is reversed in the focused
    /// pane and underlined in the other one, and nothing when a selection colour is set.
    pub selection_style: Option<Modifier>,
    pub selection_inactive_style: Option<Modifier>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::coloured(CORAL)
    }
}

/// One colour setting: its key, the text `uzu config --default` shows for it, and what it paints.
pub struct Role {
    pub name: &'static str,
    pub default: &'static str,
    pub about: &'static str,
}

const fn role(name: &'static str, default: &'static str, about: &'static str) -> Role {
    Role { name, default, about }
}

/// Every colour key, in the order the default file and the docs list them.
pub const COLOURS: &[Role] = &[
    role("accent", "#ff7a8a", "the current tab, key names in the help, the search prompt"),
    role("playing", "#ff7a8a", "the playing song: its row, its title, the word Playing"),
    role("number", "green", "the # column"),
    role("album", "cyan", "the Album column"),
    role("time", "magenta", "the Time column"),
    role("paused", "yellow", "the word Paused"),
    role("title", "none", "the song title in the player, the panel titles (on top of bold)"),
    role("header", "none", "the column headers (on top of bold)"),
    role("muted", "none", "secondary text; none = the terminal's dim look"),
    role("border", "none", "borders, rules, scrollbars, the divider"),
    role("border_focused", "none", "the border of the help and the sort menu"),
    role("selection_fg", "none", "the text of the cursor row; none = as before"),
    role("selection_bg", "none", "the background of the cursor row; none = reversed"),
    role("selection_inactive_fg", "none", "the text of the cursor row in the pane without the focus"),
    role("selection_inactive_bg", "none", "the background of that row; none = underlined"),
    role("progress", "#ff7a8a", "the finished part of the progress bar"),
    role("progress_bg", "none", "the rest of the progress bar"),
    role("lyrics_current", "#ff7a8a", "the words being sung"),
    role("lyrics_other", "none", "the other lyrics lines"),
    role("status_fg", "none", "the text of the line under the tabs"),
    role("status_bg", "none", "the background of that line"),
    role("warning", "#ff7a8a", "\"Uzumaki went away\", problems in this file"),
    role("error", "none", "an error from Uzumaki"),
];

/// The two keys that say how the cursor row looks: reversed, bold, underline...
pub const STYLES: &[Role] = &[
    role("selection_style", "auto", "the cursor row: reversed, bold, underline, italic, dim, none"),
    role("selection_inactive_style", "auto", "the same in the pane without the focus"),
];

pub const PALETTES: &[&str] = &["default", "terminal", "mono"];

/// Roles that were the accent before they had a key of their own: they follow `accent` unless
/// the file sets them, so an old file keeps its look.
pub const FOLLOW_ACCENT: &[&str] = &["playing", "progress", "lyrics_current", "warning"];

impl Theme {
    /// A palette with colour: the accent (and what follows it) and the ANSI colours.
    const fn coloured(accent: Color) -> Theme {
        Theme {
            accent: Some(accent),
            number: Some(Color::Green),
            album: Some(Color::Cyan),
            time: Some(Color::Magenta),
            paused: Some(Color::Yellow),
            playing: Some(accent),
            title: None,
            header: None,
            muted: None,
            border: None,
            border_focused: None,
            selection_fg: None,
            selection_bg: None,
            selection_inactive_fg: None,
            selection_inactive_bg: None,
            progress: Some(accent),
            progress_bg: None,
            lyrics_current: Some(accent),
            lyrics_other: None,
            status_fg: None,
            status_bg: None,
            warning: Some(accent),
            error: None,
            selection_style: None,
            selection_inactive_style: None,
        }
    }

    pub fn palette(name: &str) -> Option<Theme> {
        match name {
            "default" => Some(Theme::default()),
            "terminal" => Some(Theme::terminal()),
            "mono" => Some(Theme::mono()),
            _ => None,
        }
    }

    /// Only the 16 colours of the terminal's own theme.
    pub const fn terminal() -> Theme {
        Theme::coloured(Color::Red)
    }

    pub const fn mono() -> Theme {
        Theme {
            accent: None,
            number: None,
            album: None,
            time: None,
            paused: None,
            playing: None,
            title: None,
            header: None,
            muted: None,
            border: None,
            border_focused: None,
            selection_fg: None,
            selection_bg: None,
            selection_inactive_fg: None,
            selection_inactive_bg: None,
            progress: None,
            progress_bg: None,
            lyrics_current: None,
            lyrics_other: None,
            status_fg: None,
            status_bg: None,
            warning: None,
            error: None,
            selection_style: None,
            selection_inactive_style: None,
        }
    }

    /// The colour a key sets (`None` for a key that is not a colour).
    fn slot(&mut self, role: &str) -> Option<&mut Option<Color>> {
        Some(match role {
            "accent" => &mut self.accent,
            "number" => &mut self.number,
            "album" => &mut self.album,
            "time" => &mut self.time,
            "paused" => &mut self.paused,
            "playing" => &mut self.playing,
            "title" => &mut self.title,
            "header" => &mut self.header,
            "muted" => &mut self.muted,
            "border" => &mut self.border,
            "border_focused" => &mut self.border_focused,
            "selection_fg" => &mut self.selection_fg,
            "selection_bg" => &mut self.selection_bg,
            "selection_inactive_fg" => &mut self.selection_inactive_fg,
            "selection_inactive_bg" => &mut self.selection_inactive_bg,
            "progress" => &mut self.progress,
            "progress_bg" => &mut self.progress_bg,
            "lyrics_current" => &mut self.lyrics_current,
            "lyrics_other" => &mut self.lyrics_other,
            "status_fg" => &mut self.status_fg,
            "status_bg" => &mut self.status_bg,
            "warning" => &mut self.warning,
            "error" => &mut self.error,
            _ => return None,
        })
    }

    pub fn set(&mut self, role: &str, colour: Option<Color>) {
        if let Some(slot) = self.slot(role) {
            *slot = colour;
        }
    }

    #[cfg(test)]
    pub fn get(&self, role: &str) -> Option<Color> {
        let mut copy = *self;
        copy.slot(role).and_then(|s| *s)
    }

    pub fn set_style(&mut self, key: &str, style: Option<Modifier>) {
        match key {
            "selection_style" => self.selection_style = style,
            "selection_inactive_style" => self.selection_inactive_style = style,
            _ => {}
        }
    }

    #[cfg(test)]
    pub fn get_style(&self, key: &str) -> Option<Modifier> {
        match key {
            "selection_style" => self.selection_style,
            "selection_inactive_style" => self.selection_inactive_style,
            _ => None,
        }
    }
}

const NAMED: &[(&str, Color)] = &[
    ("black", Color::Black),
    ("red", Color::Red),
    ("green", Color::Green),
    ("yellow", Color::Yellow),
    ("blue", Color::Blue),
    ("magenta", Color::Magenta),
    ("cyan", Color::Cyan),
    ("gray", Color::Gray),
    ("white", Color::White),
    ("dark-gray", Color::DarkGray),
    ("light-red", Color::LightRed),
    ("light-green", Color::LightGreen),
    ("light-yellow", Color::LightYellow),
    ("light-blue", Color::LightBlue),
    ("light-magenta", Color::LightMagenta),
    ("light-cyan", Color::LightCyan),
    ("coral", CORAL),
];

/// A colour from the file: `Ok(None)` is "the terminal's foreground".
pub fn parse_colour(text: &str) -> Result<Option<Color>, String> {
    let t = text.trim();
    let lower = t.to_lowercase().replace(['_', ' '], "-");
    if let Some(hex) = lower.strip_prefix('#') {
        return hex_colour(hex).map(Some).ok_or_else(|| format!("'{t}' is not a colour; hex colours are #rrggbb or #rgb"));
    }
    if matches!(lower.as_str(), "none" | "default" | "reset") {
        return Ok(None);
    }
    let lower = match lower.as_str() {
        "grey" => "gray".to_string(),
        "darkgray" | "darkgrey" | "dark-grey" | "bright-black" => "dark-gray".to_string(),
        other => other.replace("bright-", "light-").replace("lightred", "light-red"),
    };
    if let Some((_, c)) = NAMED.iter().find(|(n, _)| *n == lower) {
        return Ok(Some(*c));
    }
    if let Ok(n) = lower.parse::<u8>() {
        return Ok(Some(Color::Indexed(n)));
    }
    let names: Vec<&str> = NAMED.iter().map(|(n, _)| *n).collect();
    let hint = match super::did_you_mean(&lower, &names) {
        Some(n) => format!("did you mean '{n}'?"),
        None => "use #rrggbb, a name like 'red' or 'light-blue', a number 0-255, or 'none'".to_string(),
    };
    Err(format!("'{t}' is not a colour; {hint}"))
}

fn hex_colour(hex: &str) -> Option<Color> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |s: &str| u8::from_str_radix(s, 16).ok();
    match hex.len() {
        6 => Some(Color::Rgb(v(&hex[0..2])?, v(&hex[2..4])?, v(&hex[4..6])?)),
        3 => Some(Color::Rgb(v(&hex[0..1])? * 17, v(&hex[1..2])? * 17, v(&hex[2..3])? * 17)),
        _ => None,
    }
}

/// A look for the cursor row: `"reversed|bold"`, `"underline"`, `"none"` (no modifier) or `"auto"`
/// (`Ok(None)`: reversed in the focused pane, underlined in the other, nothing with a colour).
pub fn parse_style(text: &str) -> Result<Option<Modifier>, String> {
    const NAMES: &[(&str, Modifier)] = &[
        ("reversed", Modifier::REVERSED),
        ("bold", Modifier::BOLD),
        ("underline", Modifier::UNDERLINED),
        ("italic", Modifier::ITALIC),
        ("dim", Modifier::DIM),
        ("strikethrough", Modifier::CROSSED_OUT),
    ];
    let lower = text.trim().to_lowercase();
    if matches!(lower.as_str(), "auto" | "default") {
        return Ok(None);
    }
    let mut out = Modifier::empty();
    for part in lower.split(['|', '+', ',', ' ']).filter(|p| !p.is_empty()) {
        let part = match part {
            "reverse" | "invert" | "inverted" => "reversed",
            "underlined" => "underline",
            "faint" => "dim",
            "crossed-out" | "crossed_out" => "strikethrough",
            p => p,
        };
        if part == "none" {
            continue;
        }
        match NAMES.iter().find(|(n, _)| *n == part) {
            Some((_, m)) => out |= *m,
            None => {
                let names: Vec<&str> = NAMES.iter().map(|(n, _)| *n).collect();
                let hint = match super::did_you_mean(part, &names) {
                    Some(n) => format!("did you mean '{n}'?"),
                    None => "use reversed, bold, underline, italic, dim, strikethrough, none or auto, joined with |".to_string(),
                };
                return Err(format!("'{part}' is not a style; {hint}"));
            }
        }
    }
    Ok(Some(out))
}
