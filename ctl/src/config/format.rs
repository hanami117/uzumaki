//! `[format] status_format`: one line of text from the playing song, for status bars (waybar,
//! sketchybar, starship, tmux). `uzu status --line` prints it.
//!
//! The text is copied as it is, with `{name}` replaced by a value, `{name:20}` cut to at most
//! 20 columns (with `…`), and `{{` and `}}` for a brace. Names:
//!
//! | name        | value                                   |
//! |-------------|-----------------------------------------|
//! | `artist` `title` `album` | the song's (empty when nothing plays) |
//! | `elapsed` `duration` `remaining` | `1:12`, `3:59`, `2:47`    |
//! | `percent`   | how far, 0-100 (no `%` sign)            |
//! | `state`     | `playing`, `paused`, `loading`, `stopped` |
//! | `icon`      | `▶` `‖` `…` `■`                         |
//! | `volume`    | 0-100 (no `%` sign)                     |
//! | `shuffle` `repeat` | `on`/`off`, `off`/`all`/`one`    |
//! | `source`    | the list playback started from          |
//! | `index` `count` | place of the song in that list, and its size |
//! | `queued`    | songs added with Play Next / Add to Queue |
//! | `autoplay`  | `on`/`off` (AutoPlay, ∞); empty when it is not offered |

use uzumaki_ctl::{PlayState, Status};

use crate::fmt::{fmt_time, repeat_name, truncate};

use super::{did_you_mean, list_of};

/// Used by `uzu status --line` when the file has no `status_format`.
pub const DEFAULT: &str = "{artist} — {title}";

pub const FIELDS: &[&str] = &["artist", "title", "album", "elapsed", "duration", "remaining", "percent", "state", "icon", "volume", "shuffle", "repeat", "source", "index", "count", "queued", "autoplay"];

#[derive(Clone, Debug, PartialEq)]
enum Part {
    Text(String),
    /// A field (index into `FIELDS`, `None` for an unknown name: it shows nothing) and its width.
    Field(Option<usize>, Option<usize>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    parts: Vec<Part>,
}

impl Format {
    /// Reads a format. Mistakes come back as messages (an unknown name shows nothing, an
    /// unclosed brace is text); the format is usable either way.
    pub fn parse(text: &str) -> (Format, Vec<String>) {
        let chars: Vec<char> = text.chars().collect();
        let (mut parts, mut problems, mut literal) = (Vec::new(), Vec::new(), String::new());
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '{' if chars.get(i + 1) == Some(&'{') => {
                    literal.push('{');
                    i += 2;
                }
                '}' if chars.get(i + 1) == Some(&'}') => {
                    literal.push('}');
                    i += 2;
                }
                '{' => {
                    let Some(len) = chars[i + 1..].iter().position(|c| *c == '}') else {
                        problems.push("a '{' has no closing '}' (write '{{' for a brace)".to_string());
                        literal.extend(&chars[i..]);
                        break;
                    };
                    let inner: String = chars[i + 1..i + 1 + len].iter().collect();
                    if !literal.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut literal)));
                    }
                    parts.push(field(&inner, &mut problems));
                    i += len + 2;
                }
                '}' => {
                    problems.push("a lone '}' (write '}}' for a brace)".to_string());
                    literal.push('}');
                    i += 1;
                }
                c => {
                    literal.push(c);
                    i += 1;
                }
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Text(literal));
        }
        (Format { parts }, problems)
    }

    pub fn render(&self, st: &Status) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Field(None, _) => {}
                Part::Field(Some(f), width) => {
                    let v = value(FIELDS[*f], st);
                    out.push_str(&match width {
                        Some(w) => truncate(&v, *w),
                        None => v,
                    });
                }
            }
        }
        out
    }
}

fn field(inner: &str, problems: &mut Vec<String>) -> Part {
    let (name, width) = match inner.split_once(':') {
        Some((n, w)) => (n.trim(), Some(w.trim())),
        None => (inner.trim(), None),
    };
    let width = width.and_then(|w| match w.parse::<usize>() {
        Ok(n) if n > 0 => Some(n),
        _ => {
            problems.push(format!("'{{{inner}}}': the width after ':' must be a whole number, 1 or more"));
            None
        }
    });
    let index = FIELDS.iter().position(|f| *f == name);
    if index.is_none() {
        let hint = match did_you_mean(name, FIELDS) {
            Some(n) => format!("did you mean {{{n}}}?"),
            None => format!("the names are {}", list_of(FIELDS, "", "")),
        };
        problems.push(format!("unknown name '{{{name}}}' in the format; {hint}"));
    }
    Part::Field(index, width)
}

fn value(name: &str, st: &Status) -> String {
    let song = st.song.as_ref();
    let text = |f: fn(&uzumaki_ctl::SongInfo) -> &String| song.map(|s| f(s).clone()).unwrap_or_default();
    match name {
        "artist" => text(|s| &s.artist),
        "title" => text(|s| &s.title),
        "album" => text(|s| &s.album),
        "elapsed" => fmt_time(st.position),
        "duration" => fmt_time(st.duration),
        "remaining" => fmt_time((st.duration - st.position).max(0.0)),
        "percent" => {
            if st.duration > 0.0 { ((st.position / st.duration * 100.0).clamp(0.0, 100.0) as u32).to_string() } else { "0".into() }
        }
        "state" => match st.state {
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Loading => "loading",
            PlayState::Stopped => "stopped",
        }
        .into(),
        "icon" => match st.state {
            PlayState::Playing => "▶",
            PlayState::Paused => "‖",
            PlayState::Loading => "…",
            PlayState::Stopped => "■",
        }
        .into(),
        "volume" => ((st.volume * 100.0).round() as u32).to_string(),
        "shuffle" => if st.shuffle { "on" } else { "off" }.into(),
        "repeat" => repeat_name(st.repeat).into(),
        "source" => st.source.clone(),
        "index" => st.index.map(|i| (i + 1).to_string()).unwrap_or_default(),
        "count" => if st.count > 0 { st.count.to_string() } else { String::new() },
        "queued" => st.queued.to_string(),
        "autoplay" => st.autoplay.map(|on| if on { "on" } else { "off" }).unwrap_or_default().into(),
        _ => String::new(),
    }
}

/// What `uzu status --line` prints: the format for the playing song, or `idle` when nothing is
/// loaded (a bar module that prints nothing disappears, which is usually what is wanted).
pub fn line(format: &Format, idle: &str, st: &Status) -> String {
    if st.song.is_none() || st.state == PlayState::Stopped {
        return idle.to_string();
    }
    format.render(st)
}
