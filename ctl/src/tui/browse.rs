//! Screens 2 (Playlists) and 4 (Albums), one component after ncmpcpp's playlist editor: the
//! collections on the left, the selected one's songs on the right. The songs load on a worker
//! once the left selection has rested, and are kept for the session.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use uzumaki_ctl::{CollectionInfo, Kind, Request, SongInfo};

use super::app::Out;
use super::jobs::{Job, Jobs};
use super::sort::{self, Sort};
use super::table::{self, GAP, Mark, Row, Table, bold, dim, frame, paint, theme};
use crate::fmt::{pad_left, pad_right, plural, text_width, truncate};

/// How long the left selection must rest before its songs are fetched.
pub const DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_LEFT: usize = 45;
const MIN_LEFT: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Left,
    Right,
}

#[derive(Default)]
struct Pane {
    /// Index into the full list (collections, or the shown collection's songs).
    cursor: usize,
    offset: usize,
    filter: String,
}

pub struct Browse {
    pub kind: Kind,
    items: Option<Vec<CollectionInfo>>,
    /// The list couldn't be loaded.
    failed: bool,
    left: Pane,
    right: Pane,
    /// How each pane is sorted (the cursors and numbers stay the original ones).
    left_sort: Option<Sort>,
    right_sort: Option<Sort>,
    pub focus: Focus,
    pub filtering: bool,
    /// Songs by collection id, for the session.
    tracks: HashMap<String, (CollectionInfo, Vec<SongInfo>)>,
    errors: HashMap<String, String>,
    inflight: HashSet<String>,
    /// The collection the right pane belongs to.
    shown: Option<String>,
    /// When the selection last moved to `shown`, until its songs were asked for.
    want: Option<Instant>,
    pub body: usize,
}

fn hit(f: &str, a: &str, b: &str) -> bool {
    f.is_empty() || a.to_lowercase().contains(f) || b.to_lowercase().contains(f)
}

impl Browse {
    pub fn new(kind: Kind) -> Self {
        Browse { kind, items: None, failed: false, left: Pane::default(), right: Pane::default(), left_sort: None, right_sort: None, focus: Focus::Left, filtering: false, tracks: HashMap::new(), errors: HashMap::new(), inflight: HashSet::new(), shown: None, want: None, body: 10 }
    }

    // ----------------------------------------------------------------------------------- data

    pub fn loaded(&self) -> bool {
        self.items.is_some()
    }

    pub fn set_items(&mut self, items: Vec<CollectionInfo>, now: Instant) {
        let first_load = self.items.is_none();
        let keep = self.items.as_ref().and_then(|_| self.selected()).map(|c| c.id.clone());
        let at = keep.and_then(|id| items.iter().position(|c| c.id == id)).unwrap_or(self.left.cursor).min(items.len().saturating_sub(1));
        self.items = Some(items);
        self.failed = false;
        self.left.cursor = at;
        if first_load {
            self.home(Focus::Left);
        }
        self.sync(now);
        self.fit();
    }

    pub fn set_failed(&mut self) {
        self.failed = self.items.is_none();
    }

    /// Forgets the loaded songs, so the next look fetches them again.
    pub fn forget_songs(&mut self) {
        self.tracks.clear();
        self.errors.clear();
        self.shown = None;
    }

    pub fn got(&mut self, id: String, result: Result<(CollectionInfo, Vec<SongInfo>), String>) {
        self.inflight.remove(&id);
        match result {
            Ok(loaded) => {
                self.errors.remove(&id);
                // Now the count is known: show it in the list too (playlists come without one).
                if let Some(item) = self.items.as_mut().and_then(|items| items.iter_mut().find(|c| c.id == id)) {
                    item.count = item.count.or(Some(loaded.1.len() as u32));
                }
                let shown = self.shown.as_ref() == Some(&id);
                self.tracks.insert(id, loaded);
                if shown {
                    self.home(Focus::Right);
                }
            }
            Err(e) => {
                self.errors.insert(id, e);
            }
        }
        self.fit();
    }

    /// Asks for the selected collection's songs once the selection has rested.
    pub fn tick(&mut self, now: Instant, jobs: &mut dyn Jobs) {
        let Some(at) = self.want else { return };
        if now.saturating_duration_since(at) < DEBOUNCE {
            return;
        }
        self.want = None;
        let Some(id) = self.shown.clone() else { return };
        if self.tracks.contains_key(&id) || self.inflight.contains(&id) {
            return;
        }
        self.errors.remove(&id);
        self.inflight.insert(id.clone());
        jobs.submit(Job::Tracks { kind: self.kind, id });
    }

    // ------------------------------------------------------------------------------ selection

    pub fn selected(&self) -> Option<&CollectionInfo> {
        let f = self.left.filter.to_lowercase();
        self.items.as_ref()?.get(self.left.cursor).filter(|c| hit(&f, &c.title, &c.subtitle))
    }

    fn songs(&self) -> Option<&[SongInfo]> {
        self.tracks.get(self.shown.as_ref()?).map(|t| t.1.as_slice())
    }

    /// Indices of the collections the filter lets through, in the sorted order.
    fn visible_left(&self) -> Vec<usize> {
        let f = self.left.filter.to_lowercase();
        let items = self.items.as_deref().unwrap_or_default();
        sort::order_collections(items, self.left_sort).into_iter().filter(|&i| hit(&f, &items[i].title, &items[i].subtitle)).collect()
    }

    /// Indices of the songs the filter lets through, in the sorted order.
    fn visible_right(&self) -> Vec<usize> {
        let f = self.right.filter.to_lowercase();
        let songs = self.songs().unwrap_or_default();
        sort::order_songs(songs, self.right_sort).into_iter().filter(|&i| hit(&f, &songs[i].title, &songs[i].artist)).collect()
    }

    fn visible(&self, focus: Focus) -> Vec<usize> {
        if focus == Focus::Left { self.visible_left() } else { self.visible_right() }
    }

    fn pane_mut(&mut self, focus: Focus) -> &mut Pane {
        if focus == Focus::Left { &mut self.left } else { &mut self.right }
    }

    pub fn focused_filter(&self) -> &str {
        if self.focus == Focus::Left { &self.left.filter } else { &self.right.filter }
    }

    /// Puts a pane's cursor on its first shown row.
    fn home(&mut self, focus: Focus) {
        if let Some(&i) = self.visible(focus).first() {
            self.pane_mut(focus).cursor = i;
        }
    }

    /// Sorts a pane. The cursor stays on its item (it is the item's own index), so only the
    /// scroll position needs to follow.
    pub fn set_sort(&mut self, focus: Focus, sort: Option<Sort>) {
        if focus == Focus::Left {
            self.left_sort = sort;
        } else {
            self.right_sort = sort;
        }
        self.fit();
    }

    /// Resets the right pane when the left selection moved to another collection.
    fn sync(&mut self, now: Instant) {
        let id = self.selected().map(|c| c.id.clone());
        if id != self.shown {
            self.shown = id;
            self.right = Pane::default();
            self.home(Focus::Right);
            self.want = self.shown.is_some().then_some(now);
        }
    }

    fn fit(&mut self) {
        for focus in [Focus::Left, Focus::Right] {
            let vis = self.visible(focus);
            let body = self.body;
            let pane = self.pane_mut(focus);
            let pos = vis.iter().position(|&i| i == pane.cursor).unwrap_or(0);
            pane.offset = table::scroll_to(pane.offset, pos, body, vis.len());
        }
    }

    fn move_by(&mut self, delta: isize) {
        let focus = self.focus;
        let vis = self.visible(focus);
        if vis.is_empty() {
            return;
        }
        let pane = self.pane_mut(focus);
        let pos = vis.iter().position(|&i| i == pane.cursor).unwrap_or(0) as isize;
        pane.cursor = vis[(pos + delta).clamp(0, vis.len() as isize - 1) as usize];
    }

    fn jump(&mut self, last: bool) {
        let focus = self.focus;
        let vis = self.visible(focus);
        if let Some(&i) = if last { vis.last() } else { vis.first() } {
            self.pane_mut(focus).cursor = i;
        }
    }

    pub fn resize(&mut self, body: usize) {
        self.body = body.max(1);
        self.fit();
    }

    pub fn scroll(&mut self, delta: isize, now: Instant) {
        self.move_by(delta);
        self.sync(now);
        self.fit();
    }

    // ------------------------------------------------------------------------------------ keys

    /// Keys while typing a filter for the focused pane.
    pub fn filter_key(&mut self, code: KeyCode, now: Instant) {
        let focus = self.focus;
        match code {
            KeyCode::Esc => {
                self.pane_mut(focus).filter.clear();
                self.filtering = false;
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Backspace => {
                self.pane_mut(focus).filter.pop();
            }
            KeyCode::Char(c) => self.pane_mut(focus).filter.push(c),
            _ => {}
        }
        let vis = self.visible(focus);
        let pane = self.pane_mut(focus);
        pane.offset = 0;
        if !vis.contains(&pane.cursor) {
            if let Some(&i) = vis.first() {
                pane.cursor = i;
            }
        }
        self.sync(now);
        self.fit();
    }

    pub fn key(&mut self, code: KeyCode, now: Instant) -> Out {
        let page = self.body.saturating_sub(1).max(1) as isize;
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.jump(false),
            KeyCode::End | KeyCode::Char('G') => self.jump(true),
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Left,
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Right,
            KeyCode::Tab | KeyCode::BackTab => self.focus = if self.focus == Focus::Left { Focus::Right } else { Focus::Left },
            KeyCode::Enter => match self.focus {
                Focus::Left => self.focus = Focus::Right,
                Focus::Right => return self.play_song(),
            },
            KeyCode::Char('P') => return self.play_all(),
            KeyCode::Char('R') => return Out::Reload,
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Esc => {
                let focus = self.focus;
                if !self.pane_mut(focus).filter.is_empty() {
                    self.pane_mut(focus).filter.clear();
                } else if focus == Focus::Right {
                    self.focus = Focus::Left;
                } else {
                    return Out::Back;
                }
            }
            _ => {}
        }
        self.sync(now);
        self.fit();
        Out::None
    }

    fn play_song(&self) -> Out {
        let (Some(c), Some(songs)) = (self.selected(), self.songs()) else { return Out::None };
        match songs.get(self.right.cursor).filter(|_| self.visible_right().contains(&self.right.cursor)) {
            Some(_) => {
                // A sorted pane plays in its order: the whole sorted list, from the cursor's place in it.
                let (index, order) = match self.right_sort {
                    None => (self.right.cursor, None),
                    Some(_) => {
                        let order = sort::order_songs(songs, self.right_sort);
                        (order.iter().position(|&i| i == self.right.cursor).unwrap_or(0), Some(order))
                    }
                };
                Out::Request(Request::PlayCollection { kind: self.kind, id: c.id.clone(), index: Some(index), order }, None)
            }
            None => Out::None,
        }
    }

    fn play_all(&self) -> Out {
        match self.selected() {
            Some(c) => Out::Request(Request::PlayCollection { kind: self.kind, id: c.id.clone(), index: None, order: self.songs().filter(|_| self.right_sort.is_some()).map(|s| sort::order_songs(s, self.right_sort)) }, Some(format!("Playing: {}", c.title))),
            None => Out::None,
        }
    }

    // ---------------------------------------------------------------------------------- title

    /// The context line: the list's name and count, with its sort; with the songs pane focused
    /// and sorted, the shown collection and that sort.
    pub fn title(&self) -> String {
        if let (Focus::Right, Some(s), Some(c)) = (self.focus, self.right_sort, self.selected()) {
            return format!("{} · {}", c.title, s.describe());
        }
        let name = if self.kind == Kind::Album { "Albums" } else { "Playlists" };
        let mut text = match &self.items {
            Some(items) => format!("{name} ({})", items.len()),
            None => name.to_string(),
        };
        if let Some(s) = self.left_sort {
            text.push_str(&format!(" · {}", s.describe()));
        }
        text
    }
}

// -------------------------------------------------------------------------------------- drawing

/// Left pane width: a third of the screen, between `MIN_LEFT` and `MAX_LEFT`, never over half.
fn left_width(total: usize) -> usize {
    (total / 3).max(MIN_LEFT).min(MAX_LEFT).min(total / 2)
}

fn extra_text(c: &CollectionInfo) -> String {
    if c.kind == Kind::Album { c.subtitle.clone() } else { c.count.map(|k| plural(k as usize, "song")).unwrap_or_default() }
}

/// Shown after a pinned playlist's title (dim), when there is room for it.
const PIN_TAG: &str = " pinned";

fn left_line(c: &CollectionInfo, n: usize, w: &[usize; 3], mark: Option<Mark>) -> Line<'static> {
    let [num_w, title_w, extra_w] = *w;
    let extra_style = if c.kind == Kind::Album { paint(theme().album) } else { dim() };
    let title_style = if c.favorites { bold() } else { Style::default() };
    let tag = c.pinned.is_some() && title_w >= PIN_TAG.len() + 4;
    let title = truncate(&c.title, if tag { title_w - PIN_TAG.len() } else { title_w });
    let title_parts = if tag {
        let rest = title_w - text_width(&title);
        vec![(title, title_style), (pad_right(PIN_TAG, rest), dim())]
    } else {
        vec![(pad_right(&title, title_w), title_style)]
    };
    let parts = [
        vec![(pad_left(&n.to_string(), num_w), paint(theme().number))],
        title_parts,
        vec![(pad_right(&truncate(&extra_text(c), extra_w), extra_w), extra_style)],
    ];
    let mut spans = Vec::new();
    for (i, group) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(GAP)));
        }
        for (text, style) in group {
            spans.push(Span::styled(text, mark.map_or(style, |m| style.add_modifier(m.style()))));
        }
    }
    Line::from(spans)
}

fn draw_left(f: &mut Frame, area: Rect, b: &Browse) {
    let items = b.items.as_deref().unwrap_or_default();
    let num_w = items.len().to_string().len().max(1);
    let flex = (area.width as usize).saturating_sub(num_w + 2 * GAP);
    let extra_w = if b.kind == Kind::Album {
        flex * 2 / 5
    } else {
        items.iter().map(|c| text_width(&extra_text(c))).max().unwrap_or(0).min(flex / 2)
    };
    let w = [num_w, flex - extra_w, extra_w];
    let head = format!("{}{}{}{}{}", pad_left("#", num_w), " ".repeat(GAP), pad_right("Title", w[1]), " ".repeat(GAP), pad_right(if b.kind == Kind::Album { "Artist" } else { "Songs" }, extra_w));
    f.render_widget(Paragraph::new(Line::styled(truncate(&head, area.width as usize), bold())), Rect { height: 1, ..area });
    let mark = if b.focus == Focus::Left { Mark::Focus } else { Mark::Dim };
    let vis = b.visible_left();
    let lines: Vec<Line> = if vis.is_empty() {
        let text = match (&b.items, b.failed) {
            (None, true) => "Couldn't load the list (R retries)",
            (None, false) => "Loading…",
            (Some(i), _) if i.is_empty() => if b.kind == Kind::Album { "No albums" } else { "No playlists" },
            _ => "Nothing matches the filter",
        };
        vec![Line::styled(text, dim())]
    } else {
        vis.iter().skip(b.left.offset).take((area.height as usize).saturating_sub(1)).map(|&i| left_line(&items[i], i + 1, &w, (i == b.left.cursor).then_some(mark))).collect()
    };
    f.render_widget(Paragraph::new(lines), Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area });
}

fn draw_right(f: &mut Frame, area: Rect, b: &Browse, playing: Option<&str>) {
    let songs = b.songs().unwrap_or_default();
    let vis = b.visible_right();
    let rows: Vec<Row> = vis.iter().map(|&i| Row { number: Some(i + 1), song: &songs[i], playing: playing == Some(songs[i].id.as_str()), kind: table::RowKind::Song }).collect();
    let failed;
    let empty = match (&b.shown, b.songs()) {
        (None, _) => "",
        (Some(_), Some(s)) if s.is_empty() => "(no songs)",
        (Some(_), Some(_)) => "No song matches the filter",
        (Some(id), None) => match b.errors.get(id) {
            Some(e) => {
                failed = format!("Couldn't load the songs: {e}");
                &failed
            }
            None => "Loading…",
        },
    };
    let mark = if b.focus == Focus::Right { Mark::Focus } else { Mark::Dim };
    let selected = vis.iter().position(|&i| i == b.right.cursor).map(|p| (p, mark));
    let offset = b.right.offset;
    table::draw(f, area, &Table { rows: &rows, offset, selected, num_w: songs.len().to_string().len().max(1), empty, glyph: "▶" });
}

pub fn draw(f: &mut Frame, area: Rect, b: &Browse, playing: Option<&str>) {
    if area.width < 8 || area.height == 0 {
        return;
    }
    let lw = left_width(area.width as usize) as u16;
    let left = Rect { width: lw - 1, ..area };
    let right = Rect { x: area.x + lw + 2, width: area.width - lw - 2, ..area };
    draw_left(f, left, b);
    let bar: Vec<Line> = (0..area.height).map(|_| Line::styled("│", frame())).collect();
    let divider = Rect { x: area.x + lw, width: 1, ..area };
    f.render_widget(Paragraph::new(bar), divider);
    // The left list's scrollbar replaces the divider when it is longer than its pane.
    table::scrollbar(f, Rect { y: divider.y + 1, height: divider.height.saturating_sub(1), ..divider }, b.visible_left().len(), b.left.offset, (area.height as usize).saturating_sub(1), false);
    draw_right(f, right, b, playing);
}
