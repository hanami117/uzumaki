//! Screen 5, Lyrics: the playing song's lyrics with the current line in the middle.
//!
//! The data comes from `Request::Lyrics` (the app answers from memory; it is not in `Status`,
//! which is polled twice a second). Following rmpc's lyrics pane, nothing here polls for the
//! clock: the screen redraws when the next line or word starts ([`Lyrics::until_next`]), at most
//! every [`MIN_REDRAW`], and the loop sleeps until then.
//!
//! The clock is uzu's interpolated one with the app's rule on top: it never steps backwards
//! except for a real seek ([`Lyrics::clock`]), so a late status poll doesn't flick the line back.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use uzumaki_ctl::{LyricLine, LyricsStatus};

use super::table::{bold, coral, dim};
use crate::fmt::text_width;

/// Lyrics redraw at most this often, however short the words.
pub const MIN_REDRAW: Duration = Duration::from_millis(100);
/// A report this much behind what was shown is a seek back; less is jitter and is ignored.
const SEEK_BACK: f64 = 1.0;
/// Between asks for lyrics that are still loading.
const LOADING_EVERY: Duration = Duration::from_secs(1);
/// Between asks after a failure or a reply for another song.
const RETRY_EVERY: Duration = Duration::from_millis(500);
/// Between asks while the app says the playing song has none: its own fetch may have failed
/// and be retried, and a later answer (plain or synced) should still show up.
const NONE_EVERY: Duration = Duration::from_secs(10);
/// The small label for plain lyrics.
pub const NOT_SYNCED: &str = "not time-synced";
/// The same in solo when the lyrics are longer than the box.
pub const NOT_SYNCED_SCROLL: &str = "not time-synced · j/k scroll";

/// What changes the frame: the line being sung and how many of its words have started.
pub type Cursor = (Option<usize>, usize);

#[derive(Default)]
pub struct Lyrics {
    /// The song (`SongInfo::id`) the lines belong to; `None` before any answer.
    pub song: Option<String>,
    pub status: LyricsStatus,
    pub lines: Vec<LyricLine>,
    /// The app is too old to know `Request::Lyrics`.
    pub unsupported: bool,
    /// The last ask failed (not "bad request"); try again after a while.
    failed: bool,
    /// `w` turned word-by-word off.
    pub lines_only: bool,
    /// Scrolled by hand: the first visible row (negative: blank rows above) (`g` follows the song again).
    pub manual: Option<isize>,
    /// The lowest clock shown for this song; only a seek back lowers it.
    floor: Option<f64>,
    asked: Option<Instant>,
    /// What the last redraw for this screen showed, and when.
    shown: Option<Cursor>,
    shown_at: Option<Instant>,
}

impl Lyrics {
    /// Takes in an answer. A different song restarts the clock and the scrolling.
    pub fn set(&mut self, song: String, status: LyricsStatus, lines: Vec<LyricLine>) {
        if self.song.as_deref() != Some(song.as_str()) {
            self.floor = None;
            self.manual = None;
            self.shown = None;
        }
        self.song = Some(song);
        self.status = status;
        self.lines = lines;
        self.failed = false;
    }

    /// No song, so nothing to show.
    pub fn clear(&mut self) {
        self.song = None;
        self.status = LyricsStatus::None;
        self.lines.clear();
        self.floor = None;
        self.manual = None;
        self.failed = false;
    }

    /// The screen was (re)entered or the mode changed: draw it again at once.
    pub fn reopened(&mut self) {
        self.shown = None;
        self.shown_at = None;
    }

    pub fn failed(&mut self) {
        self.failed = true;
    }

    pub fn synced(&self) -> bool {
        self.status == LyricsStatus::Synced
    }

    /// Whether the word-by-word highlight is on.
    pub fn words(&self) -> bool {
        !self.lines_only
    }

    /// Whether to ask the app now, given the song the status reports (`None`: nothing plays).
    pub fn due(&self, want: Option<&str>, now: Instant) -> bool {
        if self.unsupported {
            return false;
        }
        let since = |every: Duration| self.asked.is_none_or(|t| now.saturating_duration_since(t) >= every);
        match (want, self.song.as_deref()) {
            (None, _) => false,
            (Some(w), Some(have)) if w == have => {
                (self.status == LyricsStatus::Loading && since(LOADING_EVERY)) || (self.failed && since(RETRY_EVERY * 6)) || (self.status == LyricsStatus::None && since(NONE_EVERY))
            }
            (Some(_), _) => since(RETRY_EVERY),
        }
    }

    pub fn asked_now(&mut self, now: Instant) {
        self.asked = Some(now);
    }

    // ------------------------------------------------------------------------------- clock

    /// The clock to show for a raw position: never behind the floor unless it is a seek back.
    pub fn clock(&self, raw: f64) -> f64 {
        match self.floor {
            Some(f) if raw < f && f - raw < SEEK_BACK => f,
            _ => raw,
        }
    }

    pub fn observe(&mut self, raw: f64) {
        self.floor = Some(self.clock(raw));
    }

    /// The line sung at `t` (the last one that started) and, with word timing on, how many of
    /// its words have started. Plain lyrics have no cursor.
    pub fn cursor(&self, t: f64) -> Cursor {
        if !self.synced() {
            return (None, 0);
        }
        let Some(i) = self.lines.iter().rposition(|l| l.start <= t) else { return (None, 0) };
        let n = if self.words() { self.lines[i].words.iter().take_while(|w| w.start <= t).count() } else { 0 };
        (Some(i), n)
    }

    /// Seconds until the cursor changes (the next line or word starts), if it will.
    pub fn until_next(&self, t: f64) -> Option<f64> {
        if !self.synced() {
            return None;
        }
        let (line, _) = self.cursor(t);
        let next_line = self.lines.iter().map(|l| l.start).filter(|&s| s > t).fold(None, |a: Option<f64>, s| Some(a.map_or(s, |a| a.min(s))));
        let next_word = match line {
            Some(i) if self.words() => self.lines[i].words.iter().map(|w| w.start).find(|&s| s > t),
            _ => None,
        };
        [next_line, next_word].into_iter().flatten().reduce(f64::min).map(|s| s - t)
    }

    /// Whether the screen must be drawn again at clock `t`: the cursor moved, and the last
    /// redraw for it was at least [`MIN_REDRAW`] ago. Records the redraw when it says yes.
    pub fn changed(&mut self, t: f64, now: Instant) -> bool {
        let c = self.cursor(t);
        if self.shown == Some(c) || self.shown_at.is_some_and(|at| now.saturating_duration_since(at) < MIN_REDRAW) {
            return false;
        }
        self.shown = Some(c);
        self.shown_at = Some(now);
        true
    }

    /// How long the loop may sleep before the next redraw is due (`None`: no deadline).
    pub fn wake_in(&self, t: f64, now: Instant) -> Option<Duration> {
        let gate = self.shown_at.map_or(Duration::ZERO, |at| MIN_REDRAW.saturating_sub(now.saturating_duration_since(at)));
        let pending = self.shown != Some(self.cursor(t));
        let until = if pending { Duration::ZERO } else { Duration::from_secs_f64(self.until_next(t)?.max(0.0)) };
        Some(until.max(gate))
    }
}

// ------------------------------------------------------------------------------------ layout

/// One screen row of text: its pieces and whether the second voice sings it.
pub struct Row {
    pub line: usize,
    pub pieces: Vec<(String, Style)>,
    pub right: bool,
    width: usize,
}

/// A line's pieces, in order: its timed words when `words` and it has any, else its text split
/// at spaces. Each keeps the space after it. The flag says the piece is already sung.
fn pieces(line: &LyricLine, words: bool, sung_words: usize, whole: bool) -> Vec<(String, bool)> {
    if words && !line.words.is_empty() {
        return line.words.iter().enumerate().map(|(k, w)| (w.text.clone(), k < sung_words)).collect();
    }
    let mut out: Vec<(String, bool)> = line.text.split(' ').filter(|p| !p.is_empty()).map(|p| (format!("{p} "), whole)).collect();
    if let Some(last) = out.last_mut() {
        last.0.truncate(last.0.trim_end().len());
    }
    out
}

/// Cuts `text` to at most `w` columns.
fn clip(text: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cw = text_width(c.encode_utf8(&mut [0; 4]));
        if used + cw > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out
}

/// Every line wrapped into rows of at most `width` columns. `cursor` says which line is being
/// sung and how many of its words are; the others are past (dim) or still to come (plain).
pub fn rows(lyrics: &Lyrics, width: usize, cursor: Cursor) -> Vec<Row> {
    let width = width.max(1);
    let synced = lyrics.synced();
    let mut out = Vec::new();
    for (i, line) in lyrics.lines.iter().enumerate() {
        let current = synced && cursor.0 == Some(i);
        let past = synced && cursor.0.is_some_and(|c| i < c);
        let (base, sung) = if current {
            (bold(), coral().add_modifier(Modifier::BOLD))
        } else if past {
            (dim(), dim())
        } else {
            (Style::default(), Style::default())
        };
        // Line mode (or no word timing): the line being sung is coral as a whole.
        let timed = lyrics.words() && !line.words.is_empty();
        let ps = pieces(line, lyrics.words(), cursor.1, current && !timed);
        let mut row = Row { line: i, pieces: Vec::new(), right: line.second_voice, width: 0 };
        for (text, is_sung) in ps {
            let text = if text_width(&text) > width { clip(&text, width) } else { text };
            let w = text_width(&text);
            if row.width + text_width(text.trim_end()) > width && !row.pieces.is_empty() {
                trim_row(&mut row);
                let next = Row { line: i, pieces: Vec::new(), right: line.second_voice, width: 0 };
                out.push(std::mem::replace(&mut row, next));
            }
            row.width += w;
            row.pieces.push((text, if is_sung && current { sung } else { base }));
        }
        if row.pieces.is_empty() {
            // A line with no text is a pause: keep its place.
            row.pieces.push((String::new(), base));
        }
        trim_row(&mut row);
        out.push(row);
    }
    out
}

/// Drops the space after a row's last piece, so centring and right-aligning are exact.
fn trim_row(row: &mut Row) {
    if let Some(last) = row.pieces.last_mut() {
        let t = last.0.trim_end().len();
        row.width -= text_width(&last.0) - text_width(&last.0[..t]);
        last.0.truncate(t);
    }
}

/// The row to put at the top of a body `height` rows high: the middle of the current line in
/// the middle of the body (negative at the start of the song: blank rows above). Before the
/// first line and for plain lyrics it is the top.
pub fn follow_top(rows: &[Row], cursor: Cursor, height: usize) -> isize {
    let Some(line) = cursor.0 else { return 0 };
    let first = rows.iter().position(|r| r.line == line).unwrap_or(0);
    let count = rows.iter().filter(|r| r.line == line).count().max(1);
    (first + count / 2) as isize - (height / 2) as isize
}

/// How far a hand-scrolled view may go. Plain lyrics: from the first row to the last at the
/// bottom. Synced ones: from the first line in the middle to the last line in the middle, as
/// following shows them.
pub fn scroll_range(rows: usize, height: usize, synced: bool) -> (isize, isize) {
    if !synced {
        return (0, rows.saturating_sub(height) as isize);
    }
    let middle = (height / 2) as isize;
    (-middle, (rows as isize - 1 - middle).max(-middle))
}

/// The text shown when there are no lines.
pub fn empty_text(l: &Lyrics, playing: bool, signed_in: bool) -> &'static str {
    if l.unsupported {
        "Update Uzumaki for lyrics"
    } else if !playing {
        "Nothing playing"
    } else {
        match l.status {
            LyricsStatus::Loading => "Loading lyrics…",
            _ if l.failed => "Couldn't load the lyrics, retrying…",
            LyricsStatus::None if !signed_in => "Sign in for lyrics",
            _ => "No lyrics",
        }
    }
}

/// Draws the lyrics in `area`, from row `top` (see [`follow_top`]; negative leaves blank rows).
pub fn draw(f: &mut Frame, area: Rect, rows: &[Row], top: isize) {
    for k in 0..area.height as usize {
        let Some(row) = usize::try_from(top + k as isize).ok().and_then(|i| rows.get(i)) else { continue };
        let pad = if row.right { (area.width as usize).saturating_sub(row.width) } else { (area.width as usize).saturating_sub(row.width) / 2 };
        let mut spans = vec![Span::raw(" ".repeat(pad))];
        spans.extend(row.pieces.iter().map(|(t, s)| Span::styled(t.clone(), *s)));
        f.render_widget(Paragraph::new(Line::from(spans)), Rect { x: area.x, y: area.y + k as u16, width: area.width, height: 1 });
    }
}

/// A short message in the middle of `area`.
pub fn draw_message(f: &mut Frame, area: Rect, text: &str) {
    if area.height == 0 {
        return;
    }
    let r = Rect { x: area.x, y: area.y + area.height / 2, width: area.width, height: 1 };
    f.render_widget(Paragraph::new(Line::styled(text.to_string(), dim())).centered(), r);
}
