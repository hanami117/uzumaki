//! Screen 3: type a query, run it on a worker, and play, queue or add the results.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use uzumaki_ctl::{PickAction, Request, SongInfo};

use super::app::Out;
use super::jobs::{Job, Jobs};
use super::sort::{self, Sort};
use super::table::{self, Mark, Row, Table, coral, dim};
use crate::fmt::plural;

#[derive(PartialEq, Eq)]
pub enum State {
    /// No search yet.
    Idle,
    Searching,
    Done,
    Failed(String),
}

pub struct Search {
    /// The query that was run.
    pub query: String,
    /// What is being typed.
    pub input: String,
    pub typing: bool,
    pub results: Vec<SongInfo>,
    pub state: State,
    /// Number of the newest search sent; older answers are dropped.
    seq: u64,
    /// Index into `results` (the result's own number minus one, however they are sorted).
    pub cursor: usize,
    pub offset: usize,
    pub body: usize,
    pub sort: Option<Sort>,
}

impl Search {
    pub fn new() -> Self {
        Search { query: String::new(), input: String::new(), typing: false, results: Vec::new(), state: State::Idle, seq: 0, cursor: 0, offset: 0, body: 10, sort: None }
    }

    /// Called when the screen opens: with nothing to show, go straight to typing.
    pub fn opened(&mut self) {
        if self.results.is_empty() && self.state == State::Idle {
            self.typing = true;
        }
    }

    pub fn title(&self) -> String {
        match self.state {
            State::Idle => "Search".to_string(),
            State::Done => {
                let sort = self.sort.map(|s| format!(" · {}", s.describe())).unwrap_or_default();
                format!("Search: {} ({}){sort}", self.query, plural(self.results.len(), "result"))
            }
            _ => format!("Search: {}", self.query),
        }
    }

    /// An answer from the worker. Returns true when it was for the newest search and was taken.
    pub fn done(&mut self, seq: u64, result: Result<Vec<SongInfo>, String>) -> bool {
        if seq != self.seq {
            return false;
        }
        match result {
            Ok(songs) => {
                self.results = songs;
                self.state = State::Done;
            }
            Err(e) => self.state = State::Failed(e),
        }
        self.cursor = self.order().first().copied().unwrap_or(0);
        self.offset = 0;
        true
    }

    /// Indices of the results in the order shown.
    pub fn order(&self) -> Vec<usize> {
        sort::order_songs(&self.results, self.sort)
    }

    /// Sorts the results; the cursor stays on its result.
    pub fn set_sort(&mut self, sort: Option<Sort>) {
        self.sort = sort;
        self.fit();
    }

    fn fit(&mut self) {
        let pos = self.order().iter().position(|&i| i == self.cursor).unwrap_or(0);
        self.offset = table::scroll_to(self.offset, pos, self.body, self.results.len());
    }

    pub fn resize(&mut self, body: usize) {
        self.body = body.max(1);
        self.fit();
    }

    fn move_by(&mut self, delta: isize) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let pos = order.iter().position(|&i| i == self.cursor).unwrap_or(0) as isize;
        self.cursor = order[(pos + delta).clamp(0, order.len() as isize - 1) as usize];
        self.fit();
    }

    pub fn scroll(&mut self, delta: isize) {
        self.move_by(delta);
    }

    fn submit(&mut self, jobs: &mut dyn Jobs) {
        self.typing = false;
        let query = self.input.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.seq += 1;
        self.query = query.clone();
        self.state = State::Searching;
        self.results.clear();
        self.cursor = 0;
        self.offset = 0;
        jobs.submit(Job::Search { seq: self.seq, query });
    }

    /// Every key goes to the input while typing.
    pub fn typing_key(&mut self, key: KeyEvent, jobs: &mut dyn Jobs) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.typing = false,
            KeyCode::Enter => self.submit(jobs),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char(_) if ctrl => {}
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    pub fn key(&mut self, code: KeyCode) -> Out {
        let page = self.body.saturating_sub(1).max(1) as isize;
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX / 2),
            KeyCode::Char('/') => {
                self.typing = true;
                self.input.clear();
            }
            KeyCode::Enter => return self.pick(PickAction::Play, "Playing"),
            KeyCode::Char('a') => return self.pick(PickAction::Add, "Added"),
            KeyCode::Char('N') => return self.pick(PickAction::Next, "Playing next"),
            KeyCode::Esc => return Out::Back,
            _ => {}
        }
        Out::None
    }

    fn pick(&self, action: PickAction, said: &str) -> Out {
        match self.results.get(self.cursor) {
            Some(song) => Out::Request(Request::Pick { indices: vec![self.cursor], action }, Some(format!("{said}: {}", song.title))),
            None => Out::None,
        }
    }
}

/// `area`: the input line, then the song table.
pub fn draw(f: &mut Frame, area: Rect, s: &Search, playing: Option<&str>) {
    if area.height == 0 {
        return;
    }
    let input = if s.typing {
        Line::from(vec![Span::styled("> ", coral()), Span::raw(format!("{}▏", s.input))])
    } else {
        Line::styled("Press / to type a new query", dim())
    };
    f.render_widget(Paragraph::new(input), Rect { height: 1, ..area });
    let order = s.order();
    let rows: Vec<Row> = order.iter().map(|&i| Row { number: Some(i + 1), song: &s.results[i], playing: playing == Some(s.results[i].id.as_str()), kind: table::RowKind::Song }).collect();
    let empty = match &s.state {
        State::Idle => "Type a query and press Enter".to_string(),
        State::Searching => "Searching…".to_string(),
        State::Failed(e) => format!("Search failed: {e}"),
        State::Done => format!("No songs found for '{}'", s.query),
    };
    let selected = order.iter().position(|&i| i == s.cursor).map(|p| (p, Mark::Focus));
    let table_area = Rect { y: area.y + 1, height: area.height - 1, ..area };
    table::draw(f, table_area, &Table { rows: &rows, offset: s.offset, selected, num_w: s.results.len().to_string().len().max(1), empty: &empty, glyph: "▶" });
}
