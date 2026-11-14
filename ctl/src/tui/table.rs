//! The song table every screen shares: Artist | # | Title | Album | Time, with the playing song
//! bold coral, the selection marked and unplayable songs dim. Pure drawing.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use uzumaki_ctl::SongInfo;

use crate::fmt::{fmt_time, pad_left, pad_right, truncate};

#[cfg(test)]
pub use crate::config::theme::CORAL;
use crate::config::Columns;
use crate::config::theme::Theme;
use std::cell::Cell;

thread_local! {
    /// The colours and column shares drawing uses: `view::draw` sets them from the config at the
    /// start of each frame (drawing is one thread; a test's frames are its own thread's).
    static LOOK: Cell<(Theme, Columns)> = Cell::new((Theme::default(), Columns::default()));
}

pub fn set_look(theme: Theme, columns: Columns) {
    LOOK.with(|l| l.set((theme, columns)));
}

pub fn theme() -> Theme {
    LOOK.with(|l| l.get().0)
}

fn column_setup() -> Columns {
    LOOK.with(|l| l.get().1)
}

/// A foreground colour for a role; no colour is the terminal's own foreground.
pub fn paint(colour: Option<Color>) -> Style {
    colour.map_or_else(Style::default, |c| Style::default().fg(c))
}
pub const GAP: usize = 2;
/// Narrower than this and the Album column goes; narrower than `MIN_ARTIST_WIDTH`, Artist too.
const MIN_ALBUM_WIDTH: usize = 90;
const MIN_ARTIST_WIDTH: usize = 50;

/// The accent (coral unless `[theme]` says otherwise).
pub fn coral() -> Style {
    paint(theme().accent)
}

pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Borders, rules and tracks: the plain foreground, so they show on light themes too.
pub fn frame() -> Style {
    Style::default()
}

pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// One table row: a number (`None` shows the play mark), the song, and whether it is playing.
#[derive(Clone, Copy)]
pub struct Row<'a> {
    pub number: Option<usize>,
    pub song: &'a SongInfo,
    pub playing: bool,
    pub kind: RowKind,
}

/// What a row is. Only the queue has anything but `Song`: with repeat all, Up Next goes on with
/// a `Repeat` divider and the next lap's songs, dimmed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Song,
    /// The "↻ Repeat" divider before the next lap (`song` is a stand-in, not shown).
    Repeat,
    /// The "⟲ History · n songs" header above the songs played before this one (`song.title` is
    /// its text).
    History,
    /// A song played before the current one, shown dim above it as `H<k>`: `H1` is the one
    /// played just before the current song, so the oldest has the largest k. The row's `number`
    /// stays its entry number for Enter (`Request::History`); k is only the label.
    Past(usize),
    /// The "▸ Up Next · n songs" divider between the history and Up Next, where the playing
    /// row is not between them (the showcase). `song.title` is its text.
    UpNext,
    NextLap,
    /// The radio station's header (`song.title` is its text): Apple picks a station's songs as
    /// it goes, so after anything queued by hand come the songs it already played.
    Station,
    /// A song the station already played (dim; Enter plays it on its own).
    Played,
    /// The "∞ AutoPlay" header above the suggestions (`song.title` is its text).
    AutoPlay,
    /// A song AutoPlay suggests after Up Next (dim; Enter plays it, but it can't be moved or removed).
    Suggested,
}

impl RowKind {
    /// A line the cursor skips (it is not a song).
    pub fn is_divider(self) -> bool {
        matches!(self, RowKind::Repeat | RowKind::Station | RowKind::History | RowKind::UpNext | RowKind::AutoPlay)
    }
}

/// How the selected row shows: the focused pane inverts, the other one only underlines.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Focus,
    Dim,
}

impl Mark {
    pub fn style(self) -> Modifier {
        if self == Mark::Focus { Modifier::REVERSED } else { Modifier::UNDERLINED }
    }
}

pub struct Table<'a, 'r> {
    /// The rows to show, after filtering.
    pub rows: &'r [Row<'a>],
    /// First visible row (index into `rows`).
    pub offset: usize,
    /// Index into `rows` of the selected row.
    pub selected: Option<(usize, Mark)>,
    /// Width of the number column.
    pub num_w: usize,
    /// Shown dim in place of the rows when there are none.
    pub empty: &'r str,
    /// What the playing row shows in the number column: `▶`, `‖` or `■`.
    pub glyph: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Artist,
    Num,
    Title,
    Album,
    Time,
}

/// The columns that fit `width` and their widths (they add up to `width`).
fn columns(width: usize, num_w: usize, time_w: usize) -> Vec<(Col, usize)> {
    let setup = column_setup();
    let mut cols = vec![Col::Num, Col::Title, Col::Time];
    if setup.show_artist && width >= MIN_ARTIST_WIDTH {
        cols.insert(0, Col::Artist);
    }
    if setup.show_album && width >= MIN_ALBUM_WIDTH {
        let at = cols.iter().position(|c| *c == Col::Title).map_or(cols.len(), |i| i + 1);
        cols.insert(at, Col::Album);
    }
    let fixed = num_w + time_w + GAP * (cols.len() - 1);
    let flex = width.saturating_sub(fixed);
    let weight = |c: Col| match c {
        Col::Artist => usize::from(setup.artist),
        Col::Album => usize::from(setup.album),
        _ => usize::from(setup.title),
    };
    let total: usize = cols.iter().filter(|c| matches!(c, Col::Artist | Col::Title | Col::Album)).map(|c| weight(*c)).sum();
    let mut left = flex;
    let mut out: Vec<(Col, usize)> = cols
        .iter()
        .map(|&c| match c {
            Col::Num => (c, num_w),
            Col::Time => (c, time_w),
            Col::Title => (c, 0),
            _ => {
                let w = flex * weight(c) / total;
                left -= w;
                (c, w)
            }
        })
        .collect();
    for (c, w) in &mut out {
        if *c == Col::Title {
            *w = left; // the title takes what rounding left over
        }
    }
    out
}

fn cell(col: Col, w: usize, text: &str) -> String {
    let t = truncate(text, w);
    if matches!(col, Col::Num | Col::Time) { pad_left(&t, w) } else { pad_right(&t, w) }
}

fn cell_style(col: Col) -> Style {
    match col {
        Col::Num => paint(theme().number),
        Col::Album => paint(theme().album),
        Col::Time => paint(theme().time),
        _ => Style::default(),
    }
}

fn header_text(col: Col) -> &'static str {
    match col {
        Col::Artist => "Artist",
        Col::Num => "#",
        Col::Title => "Title",
        Col::Album => "Album",
        Col::Time => "Time",
    }
}

fn row_line(row: &Row, cols: &[(Col, usize)], mark: Option<Mark>, glyph: &str) -> Line<'static> {
    if row.kind.is_divider() {
        // Where the title column starts.
        let indent: usize = cols.iter().take_while(|(c, _)| *c != Col::Title).map(|(_, w)| w + GAP).sum();
        let text = match row.kind {
            RowKind::Station => format!("◉ {}", row.song.title),
            RowKind::History | RowKind::UpNext | RowKind::AutoPlay => row.song.title.clone(),
            _ => "↻ Repeat".to_string(),
        };
        return Line::from(vec![Span::raw(" ".repeat(indent)), Span::styled(text, dim())]);
    }
    let mut spans = Vec::new();
    for (i, &(col, w)) in cols.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(GAP)));
        }
        let text = match col {
            Col::Artist => row.song.artist.clone(),
            Col::Num => match row.kind {
                RowKind::Past(k) => format!("H{k}"),
                _ => row.number.map_or(glyph.to_string(), |n| n.to_string()),
            },
            Col::Title => row.song.title.clone(),
            Col::Album => row.song.album.clone(),
            Col::Time => if row.song.duration > 0.0 { fmt_time(row.song.duration) } else { String::new() },
        };
        let mut style = cell_style(col);
        if row.playing {
            style = coral().add_modifier(Modifier::BOLD);
        }
        if !row.song.playable || matches!(row.kind, RowKind::NextLap | RowKind::Played | RowKind::Suggested | RowKind::Past(_)) {
            style = style.add_modifier(Modifier::DIM);
        }
        if let Some(m) = mark {
            style = style.add_modifier(m.style());
        }
        spans.push(Span::styled(cell(col, w, &text), style));
    }
    Line::from(spans)
}

/// Draws the column header on the first row of `area` and the rows below it.
pub fn draw(f: &mut Frame, area: Rect, t: &Table) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let time_w = t.rows.iter().map(|r| fmt_time(r.song.duration).len()).max().unwrap_or(4).max(4);
    let cols = columns(area.width as usize, t.num_w, time_w);
    let head: Vec<Span> = cols
        .iter()
        .enumerate()
        .flat_map(|(i, &(c, w))| {
            let sep = if i > 0 { Some(Span::raw(" ".repeat(GAP))) } else { None };
            sep.into_iter().chain([Span::styled(cell(c, w, header_text(c)), bold())])
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(head)), Rect { height: 1, ..area });
    let body = Rect { y: area.y + 1, height: area.height - 1, ..area };
    let lines: Vec<Line> = if t.rows.is_empty() {
        vec![Line::styled(t.empty.to_string(), dim())]
    } else {
        t.rows
            .iter()
            .enumerate()
            .skip(t.offset)
            .take(body.height as usize)
            .map(|(i, r)| row_line(r, &cols, t.selected.filter(|(s, _)| *s == i).map(|(_, m)| m), t.glyph))
            .collect()
    };
    f.render_widget(Paragraph::new(lines), body);
    let bar = Rect { x: area.x + area.width + 1, width: 1, ..area };
    scrollbar(f, bar, t.rows.len(), t.offset, body.height as usize, true);
}

/// A scrollbar down the one-column `area`, when `len` rows don't all fit in `view`. With
/// `arrows` it has ▲ ▼ ends and a `█` thumb; without, a thin `┃` thumb on a `│` track (the
/// browse screens' divider).
pub fn scrollbar(f: &mut Frame, area: Rect, len: usize, offset: usize, view: usize, arrows: bool) {
    if view == 0 || len <= view || area.width == 0 || area.height < 3 {
        return;
    }
    let (begin, end, thumb) = if arrows { (Some("▲"), Some("▼"), "█") } else { (None, None, "┃") };
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(begin).end_symbol(end).track_symbol(Some("│")).thumb_symbol(thumb).style(frame());
    // The thumb is as long as view/len of the track and reaches the end at the last offset.
    let mut state = ScrollbarState::new(len - view + 1).position(offset).viewport_content_length(view);
    f.render_stateful_widget(bar, area, &mut state);
}

/// Scrolls `offset` so position `pos` shows in a window of `body` rows over `len` rows.
pub fn scroll_to(offset: usize, pos: usize, body: usize, len: usize) -> usize {
    let body = body.max(1);
    let mut o = offset;
    if pos < o {
        o = pos;
    } else if pos >= o + body {
        o = pos + 1 - body;
    }
    o.min(len.saturating_sub(body))
}
