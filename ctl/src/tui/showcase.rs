//! The two views `F` cycles to from the normal screen, made to be screenshotted:
//!
//! - **Showcase:** a big cover on the left with the song, the bar and the settings under it, and
//!   Up Next on the right (still a working list: cursor, Enter, d, J/K, `/`).
//! - **Solo:** only the cover and the song, centred, without any frame or hints.
//!
//! Both size the cover as big as their layout allows while staying square in pixels
//! (`Caps::square_in`), and both have a text-only form for `i` (cover hidden). Showcase falls
//! back to the normal screen when the terminal is too small for it; Solo shrinks the cover, then
//! drops it. The geometry here is shared by drawing, the mouse and the cover's encoding.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::Image;
use uzumaki_ctl::RepeatMode;

use super::app::{App, COMPACT_BELOW};
use super::cover::Caps;
use super::table::{bold, coral, dim, frame};
use super::view::{self, bar_spans, help_rect, hline, hint_text, playing_song, put, render_line, song_lines, state_word, time_text};
use crate::fmt::{fmt_time, plural, repeat_name, text_width, truncate};

/// What `F` cycles through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Normal,
    Showcase,
    Solo,
}

impl Layout {
    pub fn next(self) -> Layout {
        match self {
            Layout::Normal => Layout::Showcase,
            Layout::Showcase => Layout::Solo,
            Layout::Solo => Layout::Normal,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Normal => "Normal",
            Layout::Showcase => "Showcase",
            Layout::Solo => "Solo",
        }
    }

    /// `UZUMAKI_VIEW`: `showcase` or `solo` (anything else is the normal screen).
    pub fn parse(value: Option<&str>) -> Layout {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("showcase" | "show") => Layout::Showcase,
            Some("solo") => Layout::Solo,
            _ => Layout::Normal,
        }
    }

    pub fn from_env() -> Layout {
        Layout::parse(std::env::var("UZUMAKI_VIEW").ok().as_deref())
    }
}

/// The Up Next column keeps at least this many columns (number, title, time).
pub const QUEUE_MIN: u16 = 34;
/// Showcase needs this much room, else the normal screen shows (it has compact forms).
pub const SHOWCASE_MIN_W: u16 = 70;
pub const SHOWCASE_MIN_H: u16 = COMPACT_BELOW;
/// The left panel's width without a cover, and its least width with one.
const PANEL_TEXT: u16 = 26;
/// A smaller cover isn't worth it: the view goes text-only.
const SHOWCASE_MIN_ROWS: u16 = 6;
const SOLO_MIN_ROWS: u16 = 4;
/// Showcase rows under the cover: a gap, title, artist, album, a gap, the bar, state and time,
/// the settings.
const SHOWCASE_TEXT: u16 = 8;
/// Solo rows under the cover: a gap, title, artist, album, a gap, the bar, the time.
const SOLO_TEXT: u16 = 7;
/// Solo's bar without a cover, and its least width with one.
const SOLO_BAR_MAX: u16 = 48;
const SOLO_BAR_MIN: u16 = 24;
/// Solo's song lines may be wider than the cover, up to this.
const SOLO_TEXT_W: u16 = 60;

/// Where the showcase puts things (screen cells).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Showcase {
    /// The cover's slot; `None` with the cover hidden or no room for it.
    pub cover: Option<Rect>,
    /// The song lines under it (or alone), as wide as the left panel.
    pub text: Rect,
    /// Column of the `│` between the panel and Up Next.
    pub divider: u16,
    /// The Up Next table: its header row, then the songs (the scrollbar is right of it).
    pub queue: Rect,
}

/// The showcase's layout in a `w`×`h` terminal, or `None` when it doesn't fit (the normal
/// screen shows then). `caps` is the cover's (`None`: hidden).
pub fn showcase(w: u16, h: u16, caps: Option<Caps>) -> Option<Showcase> {
    if w < SHOWCASE_MIN_W || h < SHOWCASE_MIN_H {
        return None;
    }
    // Left: border, two blanks, the panel, two blanks, the divider; right: a blank, the table,
    // a blank, the scrollbar, the border. The panel never takes more than 3/5.
    let panel_max = (w - 10 - QUEUE_MIN).min((w - 10) * 3 / 5);
    // A blank row above and below the block, inside the borders.
    let size = caps.map(|c| c.square_in(panel_max, h - 4 - SHOWCASE_TEXT)).filter(|s| s.height >= SHOWCASE_MIN_ROWS);
    let panel = size.map_or(PANEL_TEXT, |s| s.width.max(PANEL_TEXT)).min(panel_max);
    let block = size.map_or(SHOWCASE_TEXT - 1, |s| s.height + SHOWCASE_TEXT);
    let top = 1 + (h - 2).saturating_sub(block) / 2;
    let cover = size.map(|s| Rect { x: 3, y: top, width: s.width, height: s.height });
    let text = Rect { x: 3, y: top + size.map_or(0, |s| s.height + 1), width: panel, height: SHOWCASE_TEXT - 1 };
    let divider = 3 + panel + 2;
    let queue = Rect { x: divider + 2, y: 1, width: w - divider - 5, height: h - 2 };
    Some(Showcase { cover, text, divider, queue })
}

/// Where solo puts things.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Solo {
    pub cover: Option<Rect>,
    /// The columns the cover and the song lines are centred in (the whole width, or the left
    /// part when lyrics sit on the right).
    pub col: Rect,
    /// First song row.
    pub top: u16,
    /// Width the song lines are cut to (centred in `col`).
    pub text_w: u16,
    /// The bar's row and columns; the time row is under it, as wide.
    pub bar: Rect,
    /// Where the lyrics go (right of the cover, or under the song); `None` in plain solo.
    pub lyrics: Option<Rect>,
}

/// Lyrics go right of the cover whenever a left column (the cover and the song lines) of at
/// least [`LYRICS_LEFT_MIN`] columns and a lyrics column of at least [`LYRICS_MIN_COLS`] fit
/// side by side, and the terminal is at least [`LYRICS_WIDE_MIN_H`] rows high.
const LYRICS_WIDE_MIN_H: u16 = 10;
/// The left column's share of the width, and its least width.
const LYRICS_LEFT_MIN: u16 = 34;
/// Lyrics under the song keep at least this many rows (else solo is plain).
const LYRICS_MIN_ROWS: u16 = 6;
/// Lyrics keep at least this many columns (beside the cover, 2 of margin on each side).
const LYRICS_MIN_COLS: u16 = 32;

/// The cover and the song in `col`. The cover is at most `max_rows` high; the block is centred
/// in the column's height, or starts at its top row when `centre` is false.
fn solo_block(col: Rect, caps: Option<Caps>, max_rows: u16, centre: bool) -> Solo {
    let (w, h) = (col.width, col.height);
    let size = caps.map(|c| c.square_in(w.saturating_sub(4), max_rows)).filter(|s| s.height >= SOLO_MIN_ROWS);
    let block = size.map_or(SOLO_TEXT - 1, |s| s.height + SOLO_TEXT);
    let top = col.y + if centre { h.saturating_sub(block) / 2 } else { 0 };
    let cover = size.map(|s| Rect { x: col.x + (w - s.width) / 2, y: top, width: s.width, height: s.height });
    let first = top + size.map_or(0, |s| s.height + 1);
    let room = w.saturating_sub(4);
    let bar_w = match size {
        Some(s) => s.width.max(SOLO_BAR_MIN).min(room),
        None => room.min(SOLO_BAR_MAX),
    };
    let text_w = bar_w.max(SOLO_TEXT_W).min(room.max(1));
    Solo { cover, col, top: first, text_w, bar: Rect { x: col.x + (w - bar_w) / 2, y: first + 4, width: bar_w, height: 1 }, lyrics: None }
}

/// Solo's layout: the cover and the song centred in `w`×`h`; text alone when the cover is
/// hidden or would be tiny. Any size works (rows past the bottom are clipped).
pub fn solo(w: u16, h: u16, caps: Option<Caps>) -> Solo {
    solo_block(Rect { x: 0, y: 0, width: w, height: h }, caps, h.saturating_sub(2 + SOLO_TEXT), true)
}

/// Solo with lyrics: cover and song on the left and the lyrics on the right whenever both fit
/// (the left column is 2/5 of the width, at least 34 columns, and leaves the lyrics 32; the
/// cover is the biggest square in pixels, from `caps`, that fits its column). Only when that
/// doesn't fit (narrow terminals) the lyrics go under the song, with the cover at most a third
/// of the height. `None` when there is no room for them (the plain solo shows then).
pub fn solo_lyrics(w: u16, h: u16, caps: Option<Caps>) -> Option<Solo> {
    if h >= LYRICS_WIDE_MIN_H && w >= LYRICS_LEFT_MIN + 4 + LYRICS_MIN_COLS {
        let left = (w * 2 / 5).clamp(LYRICS_LEFT_MIN, w - 4 - LYRICS_MIN_COLS);
        let lyrics = Rect { x: left + 2, y: 1, width: w - left - 4, height: h - 2 };
        let col = Rect { x: 0, y: 0, width: left, height: h };
        return Some(Solo { lyrics: Some(lyrics), ..solo_block(col, caps, h.saturating_sub(2 + SOLO_TEXT), true) });
    }
    // Under the song: the cover takes at most a third of the height, and what the lyrics need.
    let room = h.saturating_sub(2 + SOLO_TEXT + LYRICS_MIN_ROWS);
    let g = solo_block(Rect { x: 0, y: 1, width: w, height: h.saturating_sub(1) }, caps, room.min(h / 3), false);
    let y = g.top + SOLO_TEXT;
    let rows = h.saturating_sub(1).saturating_sub(y);
    (w >= LYRICS_MIN_COLS + 4 && rows >= LYRICS_MIN_ROWS).then(|| Solo { lyrics: Some(Rect { x: 2, y, width: w - 4, height: rows }), ..g })
}

// ------------------------------------------------------------------------------------ drawing

fn size_of(r: Rect) -> Size {
    Size::new(r.width, r.height)
}

/// The cover in a big slot, centred; a rounded box with `♪` when there is none (nothing
/// playing, the file still coming, or a song without artwork).
fn big_cover(f: &mut Frame, app: &App, slot: Rect) {
    // Under the help box a sixel or iTerm2 picture would show through: leave it out.
    if app.help && help_rect(f.area(), app.screen).intersects(slot) {
        return;
    }
    match playing_song(app).and_then(|song| app.cover.image(song, size_of(slot))) {
        Some(p) => {
            let size = p.size();
            let (w, h) = (size.width.min(slot.width), size.height.min(slot.height));
            let r = Rect { x: slot.x + (slot.width - w) / 2, y: slot.y + (slot.height - h) / 2, width: w, height: h };
            f.render_widget(Image::new(p), r);
        }
        None => {
            f.render_widget(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(frame()), slot);
            if slot.height >= 3 {
                let r = Rect { x: slot.x + 1, y: slot.y + slot.height / 2, width: slot.width.saturating_sub(2), height: 1 };
                f.render_widget(Paragraph::new(Line::styled("♪", frame())).centered(), r);
            }
        }
    }
}

/// `left` and `right` at the two ends of `width` columns.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used: usize = left.iter().chain(&right).map(|s| text_width(&s.content)).sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    spans.extend(right);
    Line::from(spans)
}

/// The state glyph and word, or "Connecting".
fn state_span(app: &App, word: bool) -> Span<'static> {
    match app.status.as_ref() {
        Some(st) => {
            let (glyph, name, style) = state_word(st.state);
            Span::styled(if word { format!("{glyph} {name}") } else { glyph.to_string() }, style)
        }
        None => Span::styled(if word { "… Connecting" } else { "…" }, dim()),
    }
}

/// `vol 82% · repeat all · shuffle on`, dropping from the end to fit `width`.
fn settings_text(app: &App, width: usize) -> String {
    let (shuffle, repeat) = app.flags();
    let mut parts = Vec::new();
    if let Some(st) = &app.status {
        parts.push(format!("vol {}%", (st.volume * 100.0).round() as i64));
    }
    parts.push(format!("repeat {}", repeat_name(repeat)));
    parts.push(format!("shuffle {}", if shuffle { "on" } else { "off" }));
    if repeat == RepeatMode::Off && !shuffle {
        parts.truncate(1);
    }
    while !parts.is_empty() && text_width(&parts.join(" · ")) > width {
        parts.pop();
    }
    parts.join(" · ")
}

/// The showcase's song lines, `width` columns: title, artist, album, a gap, the bar, the state
/// and time, the settings.
fn showcase_lines(app: &App, width: usize, now: Instant) -> Vec<Line<'static>> {
    let [title, artist, album] = song_lines(app, width);
    let time = Span::raw(time_text(app, now));
    vec![title, artist, album, Line::default(), Line::from(bar_spans(app, width, now, false)), spread(vec![state_span(app, true)], vec![time], width), Line::styled(settings_text(app, width), dim())]
}

/// The title in the right column's top border: "Up Next · 7 songs · 33 min", or what is being
/// typed, a message, the app being away.
fn up_next_title(app: &App, width: usize, now: Instant) -> Line<'static> {
    if let Some(text) = app.filter_text() {
        return Line::from(truncate(&format!(" /{text}▏ "), width));
    }
    if app.away {
        return Line::styled(truncate(" Uzumaki went away, retrying… ", width), coral());
    }
    if let Some(m) = app.message(now) {
        return Line::from(truncate(&format!(" {m} "), width));
    }
    let n = app.queue.up_next.len();
    let mut spans = vec![Span::raw(" "), Span::styled("Up Next", bold())];
    if n > 0 {
        let secs: f64 = app.queue.up_next.iter().map(|s| s.duration).sum();
        spans.push(Span::styled(format!(" · {} · {}", plural(n, "song"), view::short_length(secs)), dim()));
    }
    if !app.filter.is_empty() {
        spans.push(Span::raw(format!("  /{}", app.filter)));
    }
    spans.push(Span::raw(" "));
    let line = Line::from(spans);
    if line.width() > width { Line::from(truncate(&line.to_string(), width)) } else { line }
}

const SHOWCASE_HINTS: &[&str] = &["Space play/pause", "F view", "Enter play", "d remove", "J/K move", "/ filter", "Esc back", "? help"];

/// The showcase: one rounded box split in two, the playing song on the left, Up Next right.
pub fn draw_showcase(f: &mut Frame, app: &App, now: Instant, g: &Showcase) {
    let area = f.area();
    let (w, h) = (area.width, area.height);
    let d = g.divider;
    let buf = f.buffer_mut();
    hline(buf, 0, w, ("╭", "╮"));
    for y in 1..h - 1 {
        put(buf, 0, y, "│", 1, frame());
        put(buf, d, y, "│", 1, frame());
        put(buf, w - 1, y, "│", 1, frame());
    }
    hline(buf, h - 1, w, ("╰", "╯"));
    put(buf, d, 0, "┬", 1, frame());
    put(buf, d, h - 1, "┴", 1, frame());
    // Titles in the top border: plain spaces around bold words, so the dashes stop short.
    put(buf, 2, 0, " ", 1, Style::default());
    put(buf, 3, 0, "Now Playing", (d - 4) as usize, bold());
    put(buf, 14, 0, " ", (d.saturating_sub(15)) as usize, Style::default());
    let right_w = (w - d - 4) as usize;
    render_line(f, up_next_title(app, right_w, now), Rect { x: d + 2, y: 0, width: right_w as u16, height: 1 });

    if let Some(slot) = g.cover {
        big_cover(f, app, slot);
    }
    for (i, line) in showcase_lines(app, g.text.width as usize, now).into_iter().enumerate() {
        render_line(f, line, Rect { y: g.text.y + i as u16, height: 1, ..g.text });
    }
    view::draw_queue(f, g.queue, app);

    // Key hints in the bottom border, under Up Next.
    let items: &[&str] = if app.filter_text().is_some() { &["Enter keep", "Esc clear", "Backspace delete"] } else { SHOWCASE_HINTS };
    let text = hint_text(items, (w - d) as usize - 6);
    if !text.is_empty() {
        put(f.buffer_mut(), d + 2, h - 1, &format!(" {text} "), (w - d) as usize - 4, dim());
    }
}

/// `line` cut to `width` and centred on row `y`, in the columns `col`.
fn centred(f: &mut Frame, line: Line<'static>, y: u16, width: u16, col: Rect) {
    let width = width.min(col.width);
    let line = if line.width() > width as usize { Line::styled(truncate(&line.to_string(), width as usize), line.spans.first().map_or(line.style, |s| s.style)) } else { line };
    render_line(f, line.centered(), Rect { x: col.x + (col.width - width) / 2, y, width, height: 1 });
}

/// Solo: the cover and the song in the middle of the terminal, nothing else. A message (or
/// the app being away) shows on the last row while it lasts.
pub fn draw_solo(f: &mut Frame, app: &App, now: Instant, g: &Solo) {
    let area = f.area();
    if let Some(slot) = g.cover {
        big_cover(f, app, slot);
    }
    let [title, artist, album] = song_lines(app, g.text_w as usize);
    for (i, line) in [title, artist, album].into_iter().enumerate() {
        centred(f, line, g.top + i as u16, g.text_w, g.col);
    }
    if let Some(r) = g.lyrics {
        // The screen-5 renderer, in this rectangle.
        view::draw_lyrics(f, r, app, now);
    }
    let bar = g.bar;
    render_line(f, Line::from(bar_spans(app, bar.width as usize, now, false)), bar);
    // The time under the bar: elapsed at its left end, the length at its right, the state between.
    let st = app.status.as_ref().filter(|st| st.song.is_some());
    let elapsed = st.map_or("--:--".to_string(), |_| fmt_time(app.position(now)));
    let total = st.map_or("--:--".to_string(), |st| if st.duration > 0.0 { fmt_time(st.duration) } else { "?".into() });
    let width = bar.width as usize;
    let glyph = state_span(app, false);
    let gx = width.saturating_sub(text_width(&glyph.content)) / 2;
    let left = format!("{elapsed}{}", " ".repeat(gx.saturating_sub(text_width(&elapsed))));
    let time = spread(vec![Span::raw(left), glyph], vec![Span::raw(total)], width);
    render_line(f, time, Rect { y: bar.y + 1, ..bar });
    let note = if app.away { Some(Line::styled("Uzumaki went away, retrying…", coral())) } else { app.message(now).map(|m| Line::styled(m.to_string(), dim())) };
    // With no message, the lyrics' absence is a dim hint on the last row (only when asked for).
    let note = note.or_else(|| view::solo_lyrics_hint(app).map(|t| Line::styled(t, dim())));
    if let Some(line) = note
        && area.height > 1
    {
        let all = Rect { x: 0, y: 0, width: area.width, height: area.height };
        centred(f, line, area.height - 1, area.width.saturating_sub(2), all);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> Caps {
        Caps::halfblocks((10, 20))
    }

    #[test]
    fn layouts_cycle_and_parse() {
        assert_eq!(Layout::Normal.next(), Layout::Showcase);
        assert_eq!(Layout::Showcase.next(), Layout::Solo);
        assert_eq!(Layout::Solo.next(), Layout::Normal);
        assert_eq!(Layout::parse(Some(" Solo ")), Layout::Solo);
        assert_eq!(Layout::parse(Some("showcase")), Layout::Showcase);
        assert_eq!(Layout::parse(Some("whatever")), Layout::Normal);
        assert_eq!(Layout::parse(None), Layout::Normal);
    }

    #[test]
    fn showcase_cover_is_as_big_as_the_height_allows_and_square() {
        let g = showcase(160, 40, Some(caps())).unwrap();
        let c = g.cover.unwrap();
        assert_eq!(c.height, 40 - 12);
        assert_eq!(c.width, c.height * 2, "square at 10x20 px cells");
        assert_eq!(g.text.y, c.y + c.height + 1);
        assert!(g.text.y + g.text.height < 40 - 1, "inside the box");
        assert!(g.queue.width >= QUEUE_MIN && g.queue.x + g.queue.width + 3 == 160);
        assert!(g.divider > c.x + c.width);
    }

    #[test]
    fn showcase_is_width_bound_on_narrow_terminals_and_gives_up_when_too_small() {
        let g = showcase(80, 60, Some(caps())).unwrap();
        let c = g.cover.unwrap();
        assert!(c.width <= 80 - 10 - QUEUE_MIN && c.width == c.height * 2, "{c:?}");
        assert!(showcase(SHOWCASE_MIN_W - 1, 40, Some(caps())).is_none());
        assert!(showcase(120, SHOWCASE_MIN_H - 1, Some(caps())).is_none());
        // Hidden cover: a text panel, centred vertically.
        let g = showcase(120, 30, None).unwrap();
        assert!(g.cover.is_none() && g.text.width == PANEL_TEXT && g.text.y > 5);
    }

    #[test]
    fn solo_centres_and_shrinks_then_drops_the_cover() {
        let g = solo(120, 40, Some(caps()));
        let c = g.cover.unwrap();
        assert_eq!(c.height, 40 - 9);
        assert_eq!(c.x, (120 - c.width) / 2);
        assert_eq!(g.bar.width, c.width);
        assert!(g.bar.y + 1 < 40);
        assert!(solo(40, 14, Some(caps())).cover.is_some_and(|c| c.width <= 36));
        assert!(solo(120, 10, Some(caps())).cover.is_none(), "too short: text only");
        let t = solo(120, 40, None);
        assert!(t.cover.is_none() && t.bar.width == SOLO_BAR_MAX);
        for (w, h) in [(0, 0), (1, 1), (5, 3), (12, 2)] {
            let _ = solo(w, h, Some(caps()));
        }
    }
}
