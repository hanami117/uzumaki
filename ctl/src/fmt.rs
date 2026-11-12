//! Pure formatting: styled spans, widths, times and the status block. Nothing here touches the
//! terminal; callers pass the width and whether to colour.

use uzumaki_ctl::{PlayState, RepeatMode, SongInfo, Status};

const ACCENT: &str = "\x1b[38;2;255;122;138m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

const MIN_BAR: usize = 10;
const MAX_BAR: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    Dim,
    Accent,
    Bold,
}

impl Style {
    fn code(self) -> &'static str {
        match self {
            Style::Plain => "",
            Style::Dim => DIM,
            Style::Accent => ACCENT,
            Style::Bold => BOLD,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

fn span(text: impl Into<String>, style: Style) -> Span {
    Span { text: text.into(), style }
}
pub fn plain(t: impl Into<String>) -> Span {
    span(t, Style::Plain)
}
pub fn dim(t: impl Into<String>) -> Span {
    span(t, Style::Dim)
}
pub fn accent(t: impl Into<String>) -> Span {
    span(t, Style::Accent)
}
pub fn bold(t: impl Into<String>) -> Span {
    span(t, Style::Bold)
}

// ------------------------------------------------------------------------------------- widths

/// Terminal columns a char takes: 0 for combining marks, 2 for CJK and emoji, else 1.
fn char_width(c: char) -> usize {
    let u = c as u32;
    match u {
        0x300..=0x36f | 0x200b..=0x200d | 0xfe00..=0xfe0f => 0,
        0x1100..=0x115f | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe6f | 0xff00..=0xff60 | 0xffe0..=0xffe6 | 0x1f300..=0x1faff | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}

pub fn text_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// The longest prefix of `s` that fits `w` columns, no ellipsis.
fn cut(s: &str, w: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = char_width(c);
        if used + cw > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out
}

/// `s`, or its start plus `…` when wider than `w`.
pub fn truncate(s: &str, w: usize) -> String {
    if text_width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = cut(s, w - 1);
    out.push('…');
    out
}

pub fn pad_right(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(text_width(s))))
}

pub fn pad_left(s: &str, w: usize) -> String {
    format!("{}{s}", " ".repeat(w.saturating_sub(text_width(s))))
}

// ------------------------------------------------------------------------------------ spans

fn push_styled(out: &mut String, text: &str, style: Style, color: bool) {
    if text.is_empty() {
        return;
    }
    if color && style != Style::Plain {
        out.push_str(style.code());
        out.push_str(text);
        out.push_str(RESET);
    } else {
        out.push_str(text);
    }
}

/// One terminal line from spans, cut with `…` so it never exceeds `width` columns.
pub fn render(spans: &[Span], width: usize, color: bool) -> String {
    let total: usize = spans.iter().map(|s| text_width(&s.text)).sum();
    let ellipsis = total > width && width > 0;
    let mut left = if total <= width { width } else { width.saturating_sub(1) };
    let mut out = String::new();
    let mut last = Style::Plain;
    for s in spans {
        if left == 0 {
            break;
        }
        let text = cut(&s.text, left);
        left -= text_width(&text);
        push_styled(&mut out, &text, s.style, color);
        last = s.style;
    }
    if ellipsis {
        push_styled(&mut out, "…", last, color);
    }
    out
}

// -------------------------------------------------------------------------------------- times

/// `m:ss`, or `h:mm:ss` from an hour up.
pub fn fmt_time(secs: f64) -> String {
    let t = if secs.is_finite() { secs.max(0.0) as u64 } else { 0 };
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{}:{s:02}", t / 60) }
}

pub fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

// --------------------------------------------------------------------------------------- status

fn tag(state: PlayState) -> &'static str {
    match state {
        PlayState::Playing => "[playing]",
        PlayState::Paused => "[paused]",
        PlayState::Loading => "[loading]",
        PlayState::Stopped => "[stopped]",
    }
}

pub fn repeat_name(r: RepeatMode) -> &'static str {
    match r {
        RepeatMode::Off => "off",
        RepeatMode::All => "all",
        RepeatMode::One => "one",
    }
}

pub fn song_line(song: &SongInfo) -> Vec<Span> {
    if song.artist.is_empty() {
        return vec![bold(song.title.clone())];
    }
    vec![plain(song.artist.clone()), dim(" - "), bold(song.title.clone())]
}

fn fraction(pos: f64, dur: f64) -> f64 {
    if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 }
}

fn progress_line(st: &Status, width: usize) -> Vec<Span> {
    let mut spans = vec![accent(tag(st.state))];
    if st.song.is_none() {
        return spans;
    }
    let mut used = tag(st.state).len();
    let mut add = |spans: &mut Vec<Span>, s: String| {
        used += text_width(&s);
        spans.push(plain(s));
    };
    if let Some(i) = st.index {
        add(&mut spans, format!(" #{}/{}", i + 1, st.count));
    }
    let time = if st.duration > 0.0 {
        let pct = (fraction(st.position, st.duration) * 100.0) as u32;
        format!("{}/{} ({pct}%)", fmt_time(st.position), fmt_time(st.duration))
    } else {
        fmt_time(st.position)
    };
    add(&mut spans, format!("   {time}"));
    let avail = width.saturating_sub(used + 3);
    if avail >= MIN_BAR {
        let cells = avail.min(MAX_BAR);
        let filled = (fraction(st.position, st.duration) * cells as f64).round() as usize;
        spans.push(plain("   "));
        spans.push(accent("━".repeat(filled)));
        spans.push(dim("─".repeat(cells - filled)));
    }
    spans
}

pub fn volume_spans(st: &Status) -> Vec<Span> {
    let mut spans = vec![
        dim("volume: "),
        plain(format!("{}%", (st.volume * 100.0).round() as i64)),
        dim("   repeat: "),
        plain(repeat_name(st.repeat)),
        dim("   shuffle: "),
        plain(if st.shuffle { "on" } else { "off" }),
    ];
    if !st.source.is_empty() {
        spans.push(dim("   from: "));
        spans.push(plain(st.source.clone()));
    }
    spans
}

/// The `uzu status` block: song, progress bar, settings, and notes about the app's state.
pub fn status_block(st: &Status, width: usize, color: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(song) = &st.song {
        lines.push(render(&song_line(song), width, color));
    }
    lines.push(render(&progress_line(st, width), width, color));
    lines.push(render(&volume_spans(st), width, color));
    let mut notes = Vec::new();
    if !st.connected {
        notes.push("(connecting to Apple Music…)");
    } else if !st.signed_in {
        notes.push("not signed in");
    }
    if !notes.is_empty() {
        lines.push(render(&[dim(notes.join("  "))], width, color));
    }
    if let Some(e) = &st.error {
        lines.push(render(&[accent(e.clone())], width, color));
    }
    lines
}

/// Redraws a block in place: back up over the `prev` lines drawn last time, overwrite each line,
/// and blank any that are left over.
pub fn watch_frame(prev: usize, lines: &[String]) -> String {
    let mut out = String::new();
    if prev > 0 {
        out.push_str(&format!("\x1b[{prev}F"));
    }
    for l in lines {
        out.push_str(&format!("\x1b[2K{l}\n"));
    }
    if prev > lines.len() {
        let extra = prev - lines.len();
        out.push_str(&"\x1b[2K\n".repeat(extra));
        out.push_str(&format!("\x1b[{extra}F"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> SongInfo {
        SongInfo { id: "1".into(), title: "Title".into(), artist: "Artist".into(), duration: 245.0, playable: true, ..Default::default() }
    }

    fn playing() -> Status {
        Status {
            state: PlayState::Playing,
            song: Some(song()),
            position: 83.0,
            duration: 245.0,
            volume: 0.8,
            repeat: RepeatMode::All,
            source: "Anima ;)".into(),
            index: Some(2),
            count: 12,
            connected: true,
            signed_in: true,
            ..Default::default()
        }
    }

    #[test]
    fn times() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(83.9), "1:23");
        assert_eq!(fmt_time(3599.0), "59:59");
        assert_eq!(fmt_time(3600.0), "1:00:00");
        assert_eq!(fmt_time(9660.0), "2:41:00");
        assert_eq!(fmt_time(f64::NAN), "0:00");
    }

    #[test]
    fn status_block_at_fixed_width() {
        let lines = status_block(&playing(), 80, false);
        assert_eq!(lines[0], "Artist - Title");
        assert_eq!(lines[1], format!("[playing] #3/12   1:23/4:05 (33%)   {}{}", "━".repeat(15), "─".repeat(29)));
        assert_eq!(lines[2], "volume: 80%   repeat: all   shuffle: off   from: Anima ;)");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn bar_dropped_when_narrow_and_lines_never_exceed_width() {
        let lines = status_block(&playing(), 40, false);
        assert_eq!(lines[1], "[playing] #3/12   1:23/4:05 (33%)");
        for w in [10, 20, 32, 40, 60] {
            for l in status_block(&playing(), w, false) {
                assert!(text_width(&l) <= w, "{w}: {l}");
            }
        }
    }

    #[test]
    fn stopped_and_notes() {
        let st = Status { volume: 0.5, ..Default::default() };
        assert_eq!(status_block(&st, 80, false), vec!["[stopped]", "volume: 50%   repeat: off   shuffle: off", "(connecting to Apple Music…)"]);
        let st = Status { connected: true, error: Some("boom".into()), ..Default::default() };
        let lines = status_block(&st, 80, false);
        assert_eq!(&lines[2..], ["not signed in", "boom"]);
    }

    #[test]
    fn colours_only_when_asked() {
        let plain_line = &status_block(&playing(), 80, false)[0];
        assert!(!plain_line.contains('\x1b'));
        let coloured = status_block(&playing(), 80, true);
        assert!(coloured[0].contains("\x1b[1mTitle\x1b[0m"));
        assert!(coloured[1].starts_with("\x1b[38;2;255;122;138m[playing]\x1b[0m"));
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("日本語です", 5), "日本…");
        assert_eq!(truncate("abc", 0), "");
        let line = render(&[plain("abc"), bold("defgh")], 6, true);
        assert_eq!(line, "abc\x1b[1mde\x1b[0m\x1b[1m…\x1b[0m");
    }

    #[test]
    fn watch_frame_overwrites_and_clears_leftovers() {
        let a = vec!["x".to_string(), "y".to_string()];
        assert_eq!(watch_frame(0, &a), "\x1b[2Kx\n\x1b[2Ky\n");
        assert_eq!(watch_frame(3, &a), "\x1b[3F\x1b[2Kx\n\x1b[2Ky\n\x1b[2K\n\x1b[1F");
    }
}
