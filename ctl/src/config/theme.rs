//! `[theme]`: the colours of the interactive screens. Five roles carry colour; everything else
//! (borders, text) is the terminal's own foreground. A role with no colour (`None`) is drawn in
//! that foreground too.
//!
//! - `palette`: `"default"` (the coral accent and the ANSI colours), `"terminal"` (only the 16
//!   colours of the terminal's own theme, no fixed RGB) or `"mono"` (no colour at all).
//! - `accent`, `number`, `album`, `time`, `paused`: one role each, on top of the palette.
//! - `NO_COLOR` (set, not empty) turns every colour off, unless `respect_no_color = false`.
//!
//! A colour is `"#ff7a8a"` or `"#f78"`, a name (`red`, `light-blue`, `dark-gray`, …), a number
//! 0-255 (the terminal's 256-colour table), or `"none"` for the terminal's foreground.

use ratatui::style::Color;

/// The accent of the screens, as `uzu` has always drawn it.
pub const CORAL: Color = Color::Rgb(255, 122, 138);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// The playing song, the current tab, key names in the help, sung lyrics words.
    pub accent: Option<Color>,
    /// The `#` column.
    pub number: Option<Color>,
    /// The Album column.
    pub album: Option<Color>,
    /// The Time column.
    pub time: Option<Color>,
    /// The "Paused" state.
    pub paused: Option<Color>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme { accent: Some(CORAL), number: Some(Color::Green), album: Some(Color::Cyan), time: Some(Color::Magenta), paused: Some(Color::Yellow) }
    }
}

pub const PALETTES: &[&str] = &["default", "terminal", "mono"];
pub const ROLES: &[&str] = &["accent", "number", "album", "time", "paused"];

impl Theme {
    pub fn palette(name: &str) -> Option<Theme> {
        match name {
            "default" => Some(Theme::default()),
            "terminal" => Some(Theme { accent: Some(Color::Red), ..Theme::default() }),
            "mono" => Some(Theme::mono()),
            _ => None,
        }
    }

    pub const fn mono() -> Theme {
        Theme { accent: None, number: None, album: None, time: None, paused: None }
    }

    pub fn set(&mut self, role: &str, colour: Option<Color>) {
        match role {
            "accent" => self.accent = colour,
            "number" => self.number = colour,
            "album" => self.album = colour,
            "time" => self.time = colour,
            "paused" => self.paused = colour,
            _ => {}
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
