//! The state of the screens and what each key does. Pure apart from the requests it sends
//! through a [`Backend`] and the jobs it gives the worker, so tests drive it against the fake app.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use uzumaki_ctl::{Kind, PlayState, Queue, Reply, Request, SeekTo, SongInfo, Status, VolumeTo};

use super::browse::{Browse, Focus};
use super::WAIT;
use super::cover::Cover;
use super::jobs::{Done, Jobs};
use super::lyrics::{self, Lyrics};
use super::search::Search;
use super::showcase::{self, Layout};
use super::sort::{self, Sort, View};
use super::table::{Row, RowKind};
use crate::config::Config;
use crate::fmt::repeat_name;
use crate::last::{self, Last};
use crate::run::Backend;

/// Between `Status` polls.
const STATUS_EVERY: Duration = Duration::from_millis(500);
/// Between `Queue` fetches when nothing else asked for one.
const QUEUE_EVERY: Duration = Duration::from_secs(3);
/// How long a message replaces the status line.
const MESSAGE_FOR: Duration = Duration::from_secs(2);
/// How long the config file's first problem shows at the start.
const CONFIG_NOTICE_FOR: Duration = Duration::from_secs(8);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Below this terminal height the player shrinks to two rows.
pub const COMPACT_BELOW: u16 = 18;
/// Screen rows above the body (top border, player, tab rule, context line, rule).
pub fn chrome_top(height: u16) -> u16 {
    if height < COMPACT_BELOW { 6 } else { 10 }
}
/// Screen rows below the body: the bottom border with the key hints.
pub const CHROME_BOTTOM: u16 = 1;
/// After an action, when to ask for the status again (MusicKit changes state a moment later).
const CONFIRM_AFTER: [Duration; 2] = [Duration::from_millis(300), Duration::from_secs(1)];
/// How long Space's guessed state survives statuses that still report the old one.
const GUESS_HOLD: Duration = Duration::from_millis(1500);
const SEEK_SECONDS: f64 = 10.0;
const VOLUME_STEP: f64 = 0.05;

/// Stands in for the "↻ Repeat" divider in [`App::rows`] (never shown as a song).
static REPEAT_ROW: uzumaki_ctl::SongInfo = uzumaki_ctl::SongInfo { id: String::new(), title: String::new(), artist: String::new(), album: String::new(), duration: 0.0, explicit: false, local: false, playable: true };

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Queue,
    Playlists,
    Search,
    Albums,
    Lyrics,
}

impl Screen {
    pub const ALL: [Screen; 5] = [Screen::Queue, Screen::Playlists, Screen::Search, Screen::Albums, Screen::Lyrics];

    pub fn number(self) -> usize {
        Screen::ALL.iter().position(|&s| s == self).unwrap_or(0) + 1
    }

    pub fn name(self) -> &'static str {
        match self {
            Screen::Queue => "Queue",
            Screen::Playlists => "Playlists",
            Screen::Search => "Search",
            Screen::Albums => "Albums",
            Screen::Lyrics => "Lyrics",
        }
    }

    /// Screen rows above the table's first song row: the frame, the column header (search adds
    /// its input line).
    pub fn body_top(self, height: u16) -> u16 {
        chrome_top(height) + 1 + u16::from(self == Screen::Search)
    }
}

/// What a screen asks the app to do after a key.
pub enum Out {
    None,
    /// Send this request; on success show the message.
    Request(Request, Option<String>),
    /// Fetch the collection list again.
    Reload,
    /// Step back to the queue screen.
    Back,
}

pub struct App {
    pub screen: Screen,
    /// Playlists, then albums.
    browse: [Browse; 2],
    pub search: Search,
    /// Screen 5.
    pub lyrics: Lyrics,
    /// The song's cover in the player (`i` shows or hides it).
    pub cover: Cover,
    jobs: Box<dyn Jobs>,
    /// Where to record the last numbered list (`uzu play <n>` follows it).
    pub last: Option<PathBuf>,
    /// `F`: the normal screen, or one of the screenshot views (only on the queue screen).
    pub layout: Layout,
    /// `l` in solo: lyrics wanted (kept for the session).
    pub solo_lyrics: bool,
    /// Solo is laid out with the lyrics (they are synced for the playing song, or on their way).
    lyrics_shown: bool,
    /// The geometry solo was last laid out with, to clear the screen when it changes.
    solo_seen: Option<showcase::Solo>,
    /// The layout changed: the loop clears the terminal so no picture is left behind.
    pub redraw: bool,
    /// Terminal size.
    pub width: u16,
    pub height: u16,
    pub status: Option<Status>,
    status_at: Instant,
    pub queue: Queue,
    have_queue: bool,
    /// Stand-in song for the station header row, its title the text (set with the queue).
    station_head: uzumaki_ctl::SongInfo,
    /// The text of the "∞ AutoPlay" divider above the suggested songs.
    autoplay_head: uzumaki_ctl::SongInfo,
    /// `title` is the History divider's text ("⟲ History · 7 songs"), set with each queue.
    history_head: uzumaki_ctl::SongInfo,
    /// The text of the "▸ Up Next · n songs" divider between the history and Up Next.
    upnext_head: uzumaki_ctl::SongInfo,
    /// Index into the full row list (playing song first).
    pub cursor: usize,
    /// The song under the cursor (id, which occurrence of that id), so it survives queue changes.
    key: Option<(String, usize)>,
    /// First visible row (index into the filtered list) and how many rows fit.
    pub offset: usize,
    pub body: usize,
    pub filter: String,
    pub filtering: bool,
    pub help: bool,
    /// The sort popup (`,`) is open for this view.
    pub sorting: Option<View>,
    /// The sort each view uses: what was picked, else the config's default.
    sorts: sort::Prefs,
    /// What the user picked (what `uzu.json` holds); the config's defaults are not saved there.
    picked: sort::Prefs,
    /// Where the sort choices are remembered (`None`: nowhere).
    sort_file: Option<PathBuf>,
    /// The app can't be reached; keep retrying.
    pub away: bool,
    pub quit: bool,
    message: Option<(String, Instant, Duration)>,
    /// What `uzu.toml` said (the defaults when there is no file).
    pub config: Config,
    last_status: Option<Instant>,
    last_queue: Option<Instant>,
    queue_dirty: bool,
    last_click: Option<(Instant, usize)>,
    /// Times at which a quick status poll is due.
    confirm: Vec<Instant>,
    /// Space's guess: (state before, state shown, until when, song id). Statuses that still say
    /// "before" for the same song don't undo it until then.
    guess: Option<(PlayState, PlayState, Instant, Option<String>)>,
}

impl App {
    pub fn new(now: Instant, jobs: Box<dyn Jobs>) -> Self {
        App {
            screen: Screen::Queue,
            browse: [Browse::new(Kind::Playlist), Browse::new(Kind::Album)],
            search: Search::new(),
            lyrics: Lyrics::default(),
            cover: Cover::hidden(),
            jobs,
            last: None,
            layout: Layout::Normal,
            solo_lyrics: false,
            lyrics_shown: false,
            solo_seen: None,
            redraw: false,
            width: 80,
            height: 24,
            status: None,
            status_at: now,
            queue: Queue::default(),
            have_queue: false,
            station_head: Default::default(),
            autoplay_head: Default::default(),
            history_head: Default::default(),
            upnext_head: Default::default(),
            cursor: 0,
            key: None,
            offset: 0,
            body: 10,
            filter: String::new(),
            filtering: false,
            help: false,
            sorting: None,
            sorts: sort::Prefs::default(),
            picked: sort::Prefs::default(),
            sort_file: None,
            away: false,
            quit: false,
            message: None,
            config: Config::default(),
            last_status: None,
            last_queue: None,
            queue_dirty: true,
            last_click: None,
            confirm: Vec::new(),
            guess: None,
        }
    }

    // ---------------------------------------------------------------------------------- sort

    /// Remembers sort choices in `path` from now on, and takes up the ones saved there.
    pub fn use_sort_file(&mut self, path: PathBuf) {
        self.picked = sort::load(&path);
        self.sort_file = Some(path);
        self.refresh_sorts();
    }

    /// Each view's sort: the remembered pick, else `[defaults] sort_*` of the config.
    fn refresh_sorts(&mut self) {
        for view in [View::Playlists, View::Albums, View::Songs, View::Search] {
            let sort = self.picked.get(view).or(self.config.sorts.get(view));
            self.sorts.set(view, sort);
            self.apply_sort(view, sort);
        }
    }

    pub fn sort_of(&self, view: View) -> Option<Sort> {
        self.sorts.get(view)
    }

    fn apply_sort(&mut self, view: View, sort: Option<Sort>) {
        match view {
            View::Playlists => self.browse[0].set_sort(Focus::Left, sort),
            View::Albums => self.browse[1].set_sort(Focus::Left, sort),
            View::Songs => self.browse.iter_mut().for_each(|b| b.set_sort(Focus::Right, sort)),
            View::Search => self.search.set_sort(sort),
        }
    }

    fn set_sort(&mut self, view: View, sort: Option<Sort>) {
        self.sorts.set(view, sort);
        self.picked.set(view, sort);
        self.apply_sort(view, sort);
        if let Some(path) = &self.sort_file {
            sort::save(path, &self.picked);
        }
    }

    /// What `,` sorts on this screen (`None` on the queue: it plays in order).
    fn sort_view(&self) -> Option<View> {
        match self.screen {
            Screen::Queue | Screen::Lyrics => None,
            Screen::Search => Some(View::Search),
            Screen::Playlists | Screen::Albums => Some(match (self.browse(self.screen).focus, self.screen) {
                (Focus::Right, _) => View::Songs,
                (Focus::Left, Screen::Albums) => View::Albums,
                (Focus::Left, _) => View::Playlists,
            }),
        }
    }

    /// `,`: opens the popup, or says why not.
    fn open_sort(&mut self, now: Instant) {
        match self.sort_view() {
            Some(view) => self.sorting = Some(view),
            None if self.screen == Screen::Lyrics => self.set_message("Nothing to sort here", now),
            None => self.set_message("The queue plays in order; sort a playlist instead", now),
        }
    }

    /// The popup's key: a listed one sorts, `,` restores the original order, any other closes it.
    fn sort_key(&mut self, view: View, code: KeyCode) {
        self.sorting = None;
        if let KeyCode::Char(c) = code {
            if let Some(sort) = view.pick(c) {
                self.set_sort(view, sort);
            }
        }
    }

    // ----------------------------------------------------------------------------------- rows

    /// Every row: the playing song, then Up Next, then (repeat all) the "↻ Repeat" divider and
    /// the next lap, numbered on from Up Next (only Enter takes those: `Jump` knows them).
    pub fn rows(&self) -> Vec<Row<'_>> {
        let (n, lap) = (self.queue.up_next.len(), &self.queue.next_lap);
        let mut rows = Vec::with_capacity(n + lap.len() + self.queue.autoplay.len() + 3);
        let past = self.queue.history.len();
        if past > 0 {
            // Oldest first, as the app sends it: entry i is `H(past - i)`, so the song played just
            // before the current one is H1. The number stays the entry number Enter sends.
            rows.push(Row { number: None, song: &self.history_head, playing: false, kind: RowKind::History });
            rows.extend(self.queue.history.iter().enumerate().map(|(i, song)| Row { number: Some(self.entries_before_history() + i + 1), song, playing: false, kind: RowKind::Past(past - i) }));
        }
        if let Some(song) = &self.queue.current {
            rows.push(Row { number: None, song, playing: true, kind: RowKind::Song });
        } else if past > 0 && n > 0 {
            rows.push(Row { number: None, song: &self.upnext_head, playing: false, kind: RowKind::UpNext });
        }
        if self.queue.current.is_some() && past > 0 && n > 0 && self.showcase().is_some() {
            // The playing song is on the left, so nothing in the list marks where the history ends.
            rows.push(Row { number: None, song: &self.upnext_head, playing: false, kind: RowKind::UpNext });
        }
        rows.extend(self.queue.up_next.iter().enumerate().map(|(i, song)| Row { number: Some(i + 1), song, playing: false, kind: RowKind::Song }));
        if !lap.is_empty() {
            rows.push(Row { number: None, song: &REPEAT_ROW, playing: false, kind: RowKind::Repeat });
            rows.extend(lap.iter().enumerate().map(|(i, song)| Row { number: Some(n + i + 1), song, playing: false, kind: RowKind::NextLap }));
        }
        if !self.queue.autoplay.is_empty() {
            // Suggestions numbered on after Up Next and the next lap (only Enter takes them: `Jump`).
            rows.push(Row { number: None, song: &self.autoplay_head, playing: false, kind: RowKind::AutoPlay });
            rows.extend(self.queue.autoplay.iter().enumerate().map(|(i, song)| Row { number: Some(n + lap.len() + i + 1), song, playing: false, kind: RowKind::Suggested }));
        }
        if self.queue.station.is_some() {
            rows.push(Row { number: None, song: &self.station_head, playing: false, kind: RowKind::Station });
            let from = n + lap.len() + self.queue.autoplay.len();
            if self.queue.history.is_empty() {
                rows.extend(self.queue.station_played.iter().enumerate().map(|(i, song)| Row { number: Some(from + i + 1), song, playing: false, kind: RowKind::Played }));
            }
        }
        rows
    }

    /// Width of the queue table's number column: the largest entry number, or `H<k>` of the oldest
    /// history song when that is wider.
    pub fn num_width(&self) -> usize {
        let q = &self.queue;
        let digits = |n: usize| n.to_string().len();
        let top = self.entries_before_history() + q.history.len();
        let past = if q.history.is_empty() { 0 } else { 1 + digits(q.history.len()) };
        digits(top).max(past).max(1)
    }

    /// Where the cursor goes for "the playing song" in the showcase, whose list shows the history,
    /// then Up Next: the first Up Next song; with none, the playing row if it shows, else the
    /// first song the cursor can stand on (never a divider or the history header).
    fn first_up_next(&self) -> Option<usize> {
        let rows = self.rows();
        let vis = self.visible();
        let current = rows.iter().position(|r| r.playing);
        let next = vis.iter().copied().find(|&i| rows[i].kind == RowKind::Song && !rows[i].playing && current.is_none_or(|c| i > c));
        next.or_else(|| current.filter(|c| vis.contains(c))).or_else(|| self.selectable().first().copied())
    }

    /// The rows the cursor can stand on: `visible` without the divider.
    fn selectable(&self) -> Vec<usize> {
        let rows = self.rows();
        self.visible().into_iter().filter(|&i| !rows[i].kind.is_divider()).collect()
    }

    /// Indices (into [`App::rows`]) of the rows the filter lets through.
    pub fn browse(&self, screen: Screen) -> &Browse {
        &self.browse[usize::from(screen == Screen::Albums)]
    }

    /// The song playing now, for marking it in lists.
    pub fn playing_id(&self) -> Option<&str> {
        self.status.as_ref().filter(|st| st.state != PlayState::Stopped).and_then(|st| st.song.as_ref()).map(|s| s.id.as_str())
    }

    /// The filter being typed on this screen, if any.
    pub fn filter_text(&self) -> Option<&str> {
        match self.screen {
            Screen::Queue => self.filtering.then_some(self.filter.as_str()),
            Screen::Playlists | Screen::Albums => Some(self.browse(self.screen)).filter(|b| b.filtering).map(|b| b.focused_filter()),
            Screen::Search | Screen::Lyrics => None,
        }
    }

    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        let hit = |s: &SongInfo| f.is_empty() || s.title.to_lowercase().contains(&f) || s.artist.to_lowercase().contains(&f);
        // The showcase lists only Up Next: the playing song is on the left.
        let up_next_only = self.showcase().is_some();
        let divider = f.is_empty(); // a filter hides it
        self.rows().iter().enumerate().filter(|(_, r)| if r.kind.is_divider() { divider } else { hit(r.song) && !(up_next_only && r.playing) }).map(|(i, _)| i).collect()
    }

    /// The showcase's layout, when it is what shows (on the queue screen, with room for it).
    pub fn showcase(&self) -> Option<showcase::Showcase> {
        (self.screen == Screen::Queue && self.layout == Layout::Showcase).then(|| showcase::showcase(self.width, self.height, self.cover.caps())).flatten()
    }

    /// The solo view shows.
    pub fn solo(&self) -> bool {
        self.screen == Screen::Queue && self.layout == Layout::Solo
    }

    /// Solo has the lyrics on screen.
    pub fn solo_lyrics_shown(&self) -> bool {
        self.solo() && self.solo_lyrics && self.lyrics_shown
    }

    /// Solo's geometry: with the lyrics when they show (and fit), else the plain one.
    pub fn solo_geometry(&self) -> Option<showcase::Solo> {
        if !self.solo() {
            return None;
        }
        let caps = self.cover.caps();
        let with = if self.solo_lyrics_shown() { showcase::solo_lyrics_in(self.width, self.height, caps, self.config.layout.lyrics) } else { None };
        Some(with.unwrap_or_else(|| showcase::solo(self.width, self.height, caps)))
    }

    /// The lyrics are being followed: on screen 5, or in solo with `l` on.
    fn lyrics_wanted(&self) -> bool {
        self.screen == Screen::Lyrics || (self.solo() && self.solo_lyrics)
    }

    /// Plain lyrics in solo are taller than their box, so `j`/`k` scroll them.
    pub fn solo_plain_scrolls(&self) -> bool {
        let l = &self.lyrics;
        if !self.solo_lyrics_shown() || l.synced() || l.lines.is_empty() {
            return false;
        }
        let (width, height) = self.lyrics_box();
        lyrics::rows(l, width, (None, 0)).len() > height
    }

    /// Decides whether solo is laid out with lyrics. They show once the playing song's are
    /// there (synced, or plain: those sit still and `j`/`k` scroll them), and stay while the next
    /// song's are asked for (no jumping cover between songs); no lyrics, a failure or an old app
    /// take them away.
    fn update_solo_lyrics(&mut self) {
        let id = self.playing_id();
        let l = &self.lyrics;
        let shown = if !self.solo() || !self.solo_lyrics || id.is_none() || l.unsupported {
            false
        } else if l.song.as_deref() == id {
            match l.status {
                uzumaki_ctl::LyricsStatus::Synced => !l.lines.is_empty(),
                uzumaki_ctl::LyricsStatus::Plain => !l.lines.is_empty(),
                uzumaki_ctl::LyricsStatus::Loading => self.lyrics_shown,
                _ => false,
            }
        } else {
            self.lyrics_shown
        };
        self.lyrics_shown = shown;
    }

    /// Clears the screen when solo's layout moved (lyrics came or went, the cover changed
    /// place), so no picture is left where the cover was, and gives the cover its new size.
    pub fn track_solo(&mut self) {
        self.update_solo_lyrics();
        let now = self.solo_geometry();
        if self.solo_seen.is_some() && now.is_some() && now != self.solo_seen {
            self.redraw = true;
        }
        self.solo_seen = now;
        self.sync_cover();
    }

    pub fn flags(&self) -> (bool, uzumaki_ctl::RepeatMode) {
        match &self.status {
            Some(st) => (st.shuffle, st.repeat),
            None => (self.queue.shuffle, self.queue.repeat),
        }
    }

    /// AutoPlay (∞) on or off; `None` where the app doesn't offer it (an older app, local files,
    /// a station) or isn't known yet.
    pub fn autoplay(&self) -> Option<bool> {
        match &self.status {
            Some(st) => st.autoplay,
            None => self.queue.autoplay_on,
        }
    }

    /// Seconds into the song, moving on between polls while it plays.
    pub fn position(&self, now: Instant) -> f64 {
        let Some(st) = &self.status else { return 0.0 };
        let moving = if st.state == PlayState::Playing { now.saturating_duration_since(self.status_at).as_secs_f64() } else { 0.0 };
        let p = st.position + moving;
        if st.duration > 0.0 { p.min(st.duration) } else { p }
    }

    pub fn message(&self, now: Instant) -> Option<&str> {
        self.message.as_ref().filter(|(_, at, hold)| now.saturating_duration_since(*at) < *hold).map(|(m, _, _)| m.as_str())
    }

    /// A short message in the status line (`[defaults] notifications = false` hides these).
    pub fn set_message(&mut self, text: impl Into<String>, now: Instant) {
        if self.config.notifications {
            self.message = Some((text.into(), now, MESSAGE_FOR));
        }
    }

    /// An error from the app: shown whatever the notifications setting is.
    fn set_error(&mut self, text: impl Into<String>, now: Instant) {
        self.message = Some((text.into(), now, MESSAGE_FOR));
    }

    /// Takes up what `uzu.toml` said. Its problems show in the status line for a while (and
    /// `uzu config` lists them all), so a typo never leaves the screen broken or silent.
    pub fn set_config(&mut self, config: Config, now: Instant) {
        if let Some(text) = config.status_line() {
            self.message = Some((text, now, CONFIG_NOTICE_FOR));
        }
        self.config = config;
        self.refresh_sorts();
    }

    /// Where the screens start, as `[layout]` says: the screen and, on the queue, its view.
    /// `UZUMAKI_VIEW` (`env_view`) still beats the file.
    pub fn start(&mut self, be: &mut dyn Backend, env_view: bool, now: Instant) {
        if !env_view && let Some(view) = &self.config.layout.start_view {
            self.layout = Layout::parse(Some(view));
        }
        let screen = Screen::ALL[self.config.layout.start_screen.min(Screen::ALL.len() - 1)];
        if screen != Screen::Queue {
            // A view only shows on the queue; the screen asked for wins.
            self.layout = Layout::Normal;
            self.goto(screen, be, now);
        }
    }

    // ------------------------------------------------------------------------------ selection

    fn key_of(&self, index: usize) -> Option<(String, usize)> {
        let rows = self.rows();
        let id = &rows.get(index)?.song.id;
        Some((id.clone(), rows[..index].iter().filter(|r| &r.song.id == id).count()))
    }

    fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.rows().len().saturating_sub(1));
        self.key = self.key_of(self.cursor);
        self.fit();
    }

    /// Moves the cursor back onto its song after the queue changed (else keeps the place).
    fn resolve_cursor(&mut self) {
        let found = self.key.as_ref().and_then(|(id, nth)| self.rows().iter().enumerate().filter(|(_, r)| &r.song.id == id).nth(*nth).map(|(i, _)| i));
        self.set_cursor(found.unwrap_or(self.cursor));
    }

    fn move_cursor(&mut self, delta: isize) {
        let vis = self.selectable();
        if vis.is_empty() {
            return;
        }
        let pos = vis.iter().position(|&i| i == self.cursor).unwrap_or(0) as isize;
        let to = (pos + delta).clamp(0, vis.len() as isize - 1) as usize;
        self.set_cursor(vis[to]);
    }

    /// Home / g: with history above, the first Up Next song; pressed again there, the top of the
    /// history. Without history, simply the first row.
    fn cursor_home(&mut self) {
        let has_history = self.rows().iter().any(|r| matches!(r.kind, RowKind::Past(_)));
        match self.first_up_next() {
            Some(i) if has_history && self.cursor != i => self.set_cursor(i),
            _ => self.cursor_to(false),
        }
    }

    fn cursor_to(&mut self, last: bool) {
        let vis = self.selectable();
        if let Some(&i) = if last { vis.last() } else { vis.first() } {
            self.set_cursor(i);
        }
    }

    /// Records the terminal size (see [`App::resize`]).
    pub fn resize_to(&mut self, width: u16, height: u16) {
        self.width = width;
        self.resize(height);
    }

    /// Records the terminal height, how many rows each screen's table shows, and scrolls so the
    /// cursors show.
    pub fn resize(&mut self, height: u16) {
        self.height = height;
        let body = |screen: Screen| height.saturating_sub(screen.body_top(height) + CHROME_BOTTOM).max(1) as usize;
        self.body = match self.showcase() {
            Some(g) => g.queue.height.saturating_sub(1).max(1) as usize,
            None => body(Screen::Queue),
        };
        // The rows change with the view (the showcase adds the Up Next divider): follow the song.
        self.resolve_cursor();
        self.keep_cursor_shown();
        self.fit();
        for b in &mut self.browse {
            b.resize(body(Screen::Playlists));
        }
        self.search.resize(body(Screen::Search));
    }

    /// In the showcase the playing row is hidden: a cursor left on it moves to the first Up Next
    /// song (after a jump the cursor's song starts playing, and leaves the list).
    fn keep_cursor_shown(&mut self) {
        if self.showcase().is_none() {
            return;
        }
        if !self.visible().contains(&self.cursor)
            && let Some(i) = self.first_up_next()
        {
            self.set_cursor(i);
        }
    }

    fn fit(&mut self) {
        let vis = self.visible();
        let pos = vis.iter().position(|&i| i == self.cursor).unwrap_or(0);
        if pos < self.offset {
            self.offset = pos;
        } else if pos >= self.offset + self.body {
            self.offset = pos + 1 - self.body;
        }
        self.offset = self.offset.min(vis.len().saturating_sub(1));
        self.show_history_top(&vis, pos);
    }

    /// In the showcase, with the cursor on the first Up Next song and a history above it: the
    /// history's divider shows when everything down to the cursor fits (offset 0); when it does
    /// not, the list starts at the "Up Next" divider, never at a history song with the divider
    /// cut off above it.
    fn show_history_top(&mut self, vis: &[usize], pos: usize) {
        if !self.filter.is_empty() || self.queue.history.is_empty() || self.showcase().is_none() || self.first_up_next() != Some(self.cursor) {
            return;
        }
        let rows = self.rows();
        let up_div = pos.checked_sub(1).filter(|&p| rows[vis[p]].kind == RowKind::UpNext);
        self.offset = if pos < self.body { 0 } else { up_div.unwrap_or(pos) };
    }

    // ---------------------------------------------------------------------------------- data

    pub fn set_status(&mut self, mut st: Status, now: Instant) {
        if let Some((from, to, until, song)) = &self.guess {
            if now < *until && st.state == *from && st.song.as_ref().map(|s| &s.id) == song.as_ref() {
                st.state = *to;
            } else {
                self.guess = None;
            }
        }
        let changed = match &self.status {
            None => true,
            Some(old) => old.song.as_ref().map(|s| &s.id) != st.song.as_ref().map(|s| &s.id) || old.queued != st.queued || old.shuffle != st.shuffle || old.repeat != st.repeat || old.autoplay != st.autoplay,
        };
        self.queue_dirty |= changed;
        self.status = Some(st);
        self.status_at = now;
    }

    pub fn set_queue(&mut self, q: Queue, now: Instant) {
        let first = !self.have_queue;
        let old_pos = self.visible().iter().position(|&i| i == self.cursor).unwrap_or(0);
        let screen_row = old_pos.saturating_sub(self.offset);
        self.upnext_head.title = format!("\u{25b8} Up Next \u{b7} {}", crate::fmt::plural(q.up_next.len(), "song"));
        self.history_head.title = format!("\u{27f2} History \u{b7} {}", crate::fmt::plural(q.history.len(), "song"));
        self.autoplay_head.title = "\u{221e} AutoPlay \u{b7} similar music will keep playing".to_string();
        self.station_head.title = q.station.as_ref().map(|n| format!("{n} \u{2014} songs are picked as you listen")).unwrap_or_default();
        self.queue = q;
        self.have_queue = true;
        self.queue_dirty = false;
        self.last_queue = Some(now);
        if first {
            let current = self.rows().iter().position(|r| r.playing).unwrap_or(0);
            self.offset = current;
            self.set_cursor(current);
        } else {
            self.resolve_cursor();
            let pos = self.visible().iter().position(|&i| i == self.cursor).unwrap_or(0);
            self.offset = pos.saturating_sub(screen_row);
            self.fit();
        }
        self.keep_cursor_shown();
    }

    /// Sends a request; errors become a message or the "went away" state. Status and Queue
    /// replies update the screen.
    fn send(&mut self, be: &mut dyn Backend, req: Request, now: Instant) -> Option<Reply> {
        match be.request(&req) {
            Ok(Reply::Error { message }) => {
                self.set_error(message, now);
                None
            }
            Ok(reply) => {
                if self.away {
                    self.away = false;
                    self.queue_dirty = true;
                }
                match &reply {
                    Reply::Status(st) => self.set_status(st.clone(), now),
                    Reply::Queue(q) => self.set_queue(q.clone(), now),
                    _ => {}
                }
                Some(reply)
            }
            Err(_) => {
                self.away = true;
                None
            }
        }
    }

    /// Polls what is due. Returns true when it talked to the app (so the screen may have changed).
    pub fn tick(&mut self, be: &mut dyn Backend, now: Instant) -> bool {
        let mut talked = false;
        if self.confirm.iter().any(|&t| now >= t) {
            self.confirm.retain(|&t| now < t);
            self.last_status = None;
        }
        if self.last_status.is_none_or(|t| now.saturating_duration_since(t) >= STATUS_EVERY) {
            self.last_status = Some(now);
            self.send(be, Request::Status, now);
            talked = true;
        }
        let due = self.queue_dirty || self.last_queue.is_none_or(|t| now.saturating_duration_since(t) >= QUEUE_EVERY);
        if due && !self.away && self.screen == Screen::Queue && !self.solo() {
            self.last_queue = Some(now);
            self.send(be, Request::Queue, now);
            talked = true;
        }
        if matches!(self.screen, Screen::Playlists | Screen::Albums) {
            let i = usize::from(self.screen == Screen::Albums);
            self.browse[i].tick(now, &mut *self.jobs);
        }
        talked |= self.lyrics_tick(be, now);
        self.track_solo();
        talked | self.drain(now)
    }

    /// Hands the playing song's cover file to the cover (which encodes it once per file and
    /// size), with the size the current view gives it.
    pub fn sync_cover(&mut self) {
        let st = self.status.as_ref().filter(|st| st.state != PlayState::Stopped);
        let song = st.and_then(|st| st.song.as_ref()).map(|s| s.id.as_str());
        let path = st.and_then(|st| st.cover.as_deref());
        let cells = self.cover_cells();
        self.cover.sync(song, path, cells, &mut *self.jobs);
    }

    /// The cells the view gives the cover (`None`: it shows none).
    fn cover_cells(&self) -> Option<ratatui::layout::Size> {
        let caps = self.cover.caps()?;
        let slot = if let Some(g) = self.solo_geometry() {
            g.cover
        } else if self.screen == Screen::Queue && self.layout == Layout::Showcase {
            match showcase::showcase(self.width, self.height, Some(caps)) {
                Some(g) => g.cover,
                None => return Some(caps.cells()),
            }
        } else {
            return Some(caps.cells());
        };
        slot.map(|r| ratatui::layout::Size::new(r.width, r.height))
    }

    /// `F` (and Esc, 1-5 leaving a view): shows `layout`. The views live on the queue screen.
    fn set_layout(&mut self, layout: Layout, be: &mut dyn Backend, now: Instant) {
        if layout == self.layout {
            return;
        }
        self.layout = layout;
        self.redraw = true;
        if layout != Layout::Normal && self.screen != Screen::Queue {
            self.goto(Screen::Queue, be, now);
        }
        self.resize(self.height);
        self.sync_cover();
    }

    /// Takes in what the worker finished. Stale answers are dropped: a song list is kept by id
    /// (it shows only when its collection is selected), a search only if it is the newest.
    pub(super) fn drain(&mut self, now: Instant) -> bool {
        let done = self.jobs.poll();
        let any = !done.is_empty();
        for d in done {
            match d {
                Done::Tracks { kind, id, result } => self.browse[usize::from(kind == Kind::Album)].got(id, result),
                Done::Cover { key, kitty_id, result } => self.cover.done(key, kitty_id, result),
                Done::Search { seq, result } => {
                    let failed = result.as_ref().err().cloned();
                    if self.search.done(seq, result) {
                        match failed {
                            Some(e) => self.set_message(format!("Search failed: {e}"), now),
                            None => {
                                if let Some(path) = &self.last {
                                    last::save(path, &Last::Search);
                                }
                            }
                        }
                    }
                }
            }
        }
        any
    }

    /// Switches screen, loading what it shows the first time.
    fn goto(&mut self, screen: Screen, be: &mut dyn Backend, now: Instant) {
        self.screen = screen;
        match screen {
            Screen::Queue => self.queue_dirty = true,
            Screen::Search => self.search.opened(),
            Screen::Lyrics => self.lyrics.reopened(),
            Screen::Playlists | Screen::Albums => {
                if !self.browse(screen).loaded() {
                    self.load_collections(screen, be, now);
                }
            }
        }
        self.resize(self.height);
    }

    fn load_collections(&mut self, screen: Screen, be: &mut dyn Backend, now: Instant) {
        let (i, req) = if screen == Screen::Albums { (1, Request::Albums) } else { (0, Request::Playlists) };
        match self.send(be, req, now) {
            Some(Reply::Collections { mut items }) => {
                uzumaki_ctl::pinned_first(&mut items); // as `uzu playlists` numbers them
                self.browse[i].set_items(items, now)
            }
            _ => self.browse[i].set_failed(),
        }
    }

    /// Asks for a fresh status at the next tick (after an action that changes the song).
    fn poll_soon(&mut self, now: Instant) {
        self.last_status = None;
        self.confirm_soon(now);
    }

    /// Asks again at +300 ms and +1 s, for an action whose effect shows late.
    fn confirm_soon(&mut self, now: Instant) {
        self.confirm = CONFIRM_AFTER.iter().map(|d| now + *d).collect();
    }

    /// Space: sends the toggle and shows the new state at once; the next real status overrides it.
    fn toggle(&mut self, be: &mut dyn Backend, now: Instant) {
        let before = self.status.as_ref().map(|st| st.state);
        if self.send(be, Request::Toggle, now).is_none() {
            return;
        }
        // The reply still carries the old state, so no immediate poll: it would undo the guess.
        self.last_status = Some(now);
        self.confirm_soon(now);
        let Some(before) = before else { return };
        let (to, word) = if before == PlayState::Playing { (PlayState::Paused, "Paused") } else { (PlayState::Playing, "Playing") };
        if let Some(st) = &mut self.status {
            st.state = to;
            self.guess = Some((before, to, now + GUESS_HOLD, st.song.as_ref().map(|s| s.id.clone())));
        }
        self.set_message(word, now);
    }

    // ------------------------------------------------------------------------------------ keys

    pub fn handle_key(&mut self, key: KeyEvent, be: &mut dyn Backend, now: Instant) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if let Some(view) = self.sorting {
            self.sort_key(view, key.code);
            return;
        }
        // The keymap (`[keys]` in uzu.toml) turns the user's keys into the default ones the code
        // below answers to. Typing a filter or a query takes every key as it comes.
        let key = if self.typing() {
            key
        } else {
            match self.config.keys.translate(key) {
                Some(k) => k,
                None => return,
            }
        };
        let comma = key.code == KeyCode::Char(',');
        match self.screen {
            Screen::Queue => {
                if self.filtering {
                    self.filter_key(key.code);
                } else if self.solo() {
                    // Solo shows no list, so only the common keys, `l` and `w` for lyrics, and Esc back.
                    if !self.common_key(key.code, be, now) {
                        match key.code {
                            KeyCode::Esc => self.set_layout(Layout::Normal, be, now),
                            KeyCode::Char('l') => {
                                self.solo_lyrics = !self.solo_lyrics;
                                self.lyrics.reopened();
                                self.set_message(if self.solo_lyrics { "Lyrics shown" } else { "Lyrics hidden (l to show)" }, now);
                                self.track_solo();
                            }
                            KeyCode::Char('j' | 'k' | 'g' | 'G') | KeyCode::PageDown | KeyCode::PageUp if self.solo_plain_scrolls() => self.lyrics_key(key.code, be, now),
                            KeyCode::Char('w') if self.solo_lyrics => {
                                self.lyrics.lines_only = !self.lyrics.lines_only;
                                self.lyrics.reopened();
                                self.set_message(if self.lyrics.words() { "Lyrics: word by word" } else { "Lyrics: line by line" }, now);
                            }
                            _ => {}
                        }
                    }
                } else if comma {
                    self.open_sort(now);
                } else if !self.common_key(key.code, be, now) {
                    self.command_key(key.code, be, now);
                }
                self.keep_cursor_shown();
                self.fit();
            }
            Screen::Playlists | Screen::Albums => {
                let i = usize::from(self.screen == Screen::Albums);
                if self.browse[i].filtering {
                    self.browse[i].filter_key(key.code, now);
                } else if comma {
                    self.open_sort(now);
                } else if !self.common_key(key.code, be, now) {
                    let out = self.browse[i].key(key.code, now);
                    self.apply(out, be, now);
                }
            }
            Screen::Search => {
                if self.search.typing {
                    self.search.typing_key(key, &mut *self.jobs);
                } else if comma {
                    self.open_sort(now);
                } else if !self.common_key(key.code, be, now) {
                    let out = self.search.key(key.code);
                    self.apply(out, be, now);
                }
            }
            Screen::Lyrics => {
                if comma {
                    self.open_sort(now);
                } else if !self.common_key(key.code, be, now) {
                    self.lyrics_key(key.code, be, now);
                }
            }
        }
        self.drain(now);
    }

    /// A filter or the search box is taking text.
    fn typing(&self) -> bool {
        match self.screen {
            Screen::Queue => self.filtering,
            Screen::Playlists | Screen::Albums => self.browse[usize::from(self.screen == Screen::Albums)].filtering,
            Screen::Search => self.search.typing,
            Screen::Lyrics => false,
        }
    }

    fn apply(&mut self, out: Out, be: &mut dyn Backend, now: Instant) {
        match out {
            Out::None => {}
            Out::Request(req, message) => {
                if self.send(be, req, now).is_some() {
                    self.poll_soon(now);
                    if let Some(m) = message {
                        self.set_message(m, now);
                    }
                }
            }
            Out::Reload => {
                let screen = self.screen;
                self.browse[usize::from(screen == Screen::Albums)].forget_songs();
                self.load_collections(screen, be, now);
            }
            Out::Back => self.goto(Screen::Queue, be, now),
        }
    }

    /// The keys every screen shares. Returns true when the key was one of them.
    fn common_key(&mut self, code: KeyCode, be: &mut dyn Backend, now: Instant) -> bool {
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('i') => {
                let on = self.cover.toggle();
                self.set_message(if on { "Cover: on" } else { "Cover: off" }, now);
                // The views lay out again around the cover (or without it).
                self.redraw |= self.layout != Layout::Normal;
                self.resize(self.height);
                self.sync_cover();
            }
            KeyCode::Char('F') => {
                let next = self.layout.next();
                self.set_layout(next, be, now);
                self.set_message(format!("{} view · F next", next.name()), now);
            }
            KeyCode::Char(' ') => self.toggle(be, now),
            KeyCode::Char('n') => self.act(be, Request::Next, now),
            KeyCode::Char('p') => self.act(be, Request::Prev, now),
            KeyCode::Char('s') => {
                if let Some(Reply::Status(st)) = self.send(be, Request::Shuffle { on: None }, now) {
                    self.set_message(format!("Shuffle: {}", if st.shuffle { "on" } else { "off" }), now);
                }
            }
            KeyCode::Char('r') => {
                if let Some(Reply::Status(st)) = self.send(be, Request::Repeat { mode: None }, now) {
                    self.set_message(format!("Repeat: {}", repeat_name(st.repeat)), now);
                }
            }
            KeyCode::Char('A') => self.toggle_autoplay(be, now),
            KeyCode::Char('f') => self.act(be, Request::Seek { to: SeekTo::Relative(SEEK_SECONDS) }, now),
            KeyCode::Char('b') => self.act(be, Request::Seek { to: SeekTo::Relative(-SEEK_SECONDS) }, now),
            KeyCode::Left if matches!(self.screen, Screen::Queue | Screen::Search | Screen::Lyrics) => self.act(be, Request::Seek { to: SeekTo::Relative(-SEEK_SECONDS) }, now),
            KeyCode::Right if matches!(self.screen, Screen::Queue | Screen::Search | Screen::Lyrics) => self.act(be, Request::Seek { to: SeekTo::Relative(SEEK_SECONDS) }, now),
            KeyCode::Char('+' | '=') => self.act(be, Request::Volume { to: VolumeTo::Change(VOLUME_STEP) }, now),
            KeyCode::Char('-') => self.act(be, Request::Volume { to: VolumeTo::Change(-VOLUME_STEP) }, now),
            KeyCode::Char(c @ '1'..='5') => {
                self.set_layout(Layout::Normal, be, now);
                self.goto(Screen::ALL[c as usize - '1' as usize], be, now);
            }
            _ => return false,
        }
        true
    }

    fn filter_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.filter.clear();
                self.filtering = false;
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => {}
        }
        let vis = self.selectable();
        if !vis.contains(&self.cursor) {
            if let Some(&i) = vis.first() {
                self.set_cursor(i);
            }
        }
        self.offset = 0;
    }

    fn command_key(&mut self, code: KeyCode, be: &mut dyn Backend, now: Instant) {
        let page = self.body.saturating_sub(1).max(1) as isize;
        match code {
            KeyCode::Esc if self.filter.is_empty() && self.layout != Layout::Normal => self.set_layout(Layout::Normal, be, now),
            KeyCode::Esc if self.filter.is_empty() => self.quit = true,
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-page),
            KeyCode::PageDown => self.move_cursor(page),
            KeyCode::Home | KeyCode::Char('g') => self.cursor_home(),
            KeyCode::End | KeyCode::Char('G') => self.cursor_to(true),
            KeyCode::Char('o') => self.select_playing(now),
            KeyCode::Enter => self.activate(be, now),
            KeyCode::Char('d') | KeyCode::Delete => self.remove_selected(be, now),
            KeyCode::Char('J') => self.move_selected(be, true, now),
            KeyCode::Char('K') => self.move_selected(be, false, now),
            _ => {}
        }
    }

    /// A playback request whose answer is a status; the song may change a moment later.
    fn act(&mut self, be: &mut dyn Backend, req: Request, now: Instant) {
        if self.send(be, req, now).is_some() {
            self.poll_soon(now);
        }
    }

    fn select_playing(&mut self, now: Instant) {
        if self.queue.current.is_none() {
            return;
        }
        if self.showcase().is_some() {
            // The playing song is on the left; the list starts with the history, so what plays
            // next is the first Up Next song, not the list's top.
            if let Some(i) = self.first_up_next() {
                self.set_cursor(i);
            }
            return;
        }
        let current = self.rows().iter().position(|r| r.playing).unwrap_or(0);
        if self.visible().contains(&current) {
            self.set_cursor(current);
        } else {
            self.set_message("The playing song is filtered out", now);
        }
    }

    fn selected_number(&self) -> Option<Option<usize>> {
        self.rows().get(self.cursor).filter(|r| !r.kind.is_divider()).map(|r| r.number)
    }

    fn activate(&mut self, be: &mut dyn Backend, now: Instant) {
        match self.selected_number() {
            Some(Some(n)) => {
                let start = self.entries_before_history();
                let req = if n > start { Request::History { index: n - start - 1 } } else { Request::Jump { index: n - 1 } };
                self.act(be, req, now);
            },
            Some(None) => self.act(be, Request::Seek { to: SeekTo::Absolute(0.0) }, now),
            None => {}
        }
    }

    /// Why entry `n` (past Up Next) can't be removed or moved.
    fn uneditable(&self, n: usize) -> &'static str {
        let (up, lap, auto) = (self.queue.up_next.len(), self.queue.next_lap.len(), self.queue.autoplay.len());
        if n > self.entries_before_history() {
            "History songs can't be edited"
        } else if n > up + lap + auto {
            "Songs the station played can't be edited"
        } else if n > up + lap {
            "AutoPlay songs can't be edited (A turns AutoPlay off)"
        } else {
            "The next lap can't be edited"
        }
    }

    /// How many entries `Jump` knows before the History ones: Up Next, the next lap, AutoPlay's
    /// suggestions and the songs a station played (the numbers the rows carry).
    fn entries_before_history(&self) -> usize {
        let q = &self.queue;
        q.up_next.len() + q.next_lap.len() + q.autoplay.len() + q.station_played.len()
    }

    /// The `A` key: AutoPlay on or off. The answer is the queue, with the new state.
    fn toggle_autoplay(&mut self, be: &mut dyn Backend, now: Instant) {
        if let Some(Reply::Queue(q)) = self.send(be, Request::Autoplay { on: None }, now) {
            self.poll_soon(now);
            self.set_message(
                match q.autoplay_on {
                    Some(true) => "AutoPlay: on \u{221e}",
                    Some(false) => "AutoPlay: off",
                    None => "AutoPlay isn't offered here",
                },
                now,
            );
        }
    }

    fn remove_selected(&mut self, be: &mut dyn Backend, now: Instant) {
        match self.selected_number() {
            Some(Some(n)) if n > self.queue.up_next.len() => self.set_message(self.uneditable(n), now),
            Some(Some(n)) => {
                if self.send(be, Request::Remove { indices: vec![n - 1] }, now).is_some() {
                    self.set_message("Removed 1 song", now);
                }
            }
            Some(None) => self.set_message("Can't remove the playing song", now),
            None => {}
        }
    }

    fn move_selected(&mut self, be: &mut dyn Backend, down: bool, now: Instant) {
        let Some(Some(n)) = self.selected_number() else {
            if self.selected_number().is_some() {
                self.set_message("Can't move the playing song", now);
            }
            return;
        };
        if n > self.queue.up_next.len() {
            self.set_message(self.uneditable(n), now);
            return;
        }
        if (down && n >= self.queue.up_next.len()) || (!down && n == 1) {
            self.set_message(if down { "Already at the end" } else { "Already first in Up Next" }, now);
            return;
        }
        let (from, to) = (n - 1, if down { n } else { n - 2 });
        if self.send(be, Request::Move { from, to }, now).is_some() {
            let target = if down { self.cursor + 1 } else { self.cursor - 1 };
            self.set_cursor(target);
        }
    }

    // -------------------------------------------------------------------------------- lyrics

    /// The lyrics' size: the body's columns and rows (the same box `view::draw` gives them).
    fn lyrics_box(&self) -> (usize, usize) {
        if let Some(r) = self.solo_geometry().and_then(|g| g.lyrics).filter(|_| self.solo_lyrics_shown()) {
            return (r.width as usize, r.height as usize);
        }
        let h = self.height;
        (self.width.saturating_sub(5) as usize, h.saturating_sub(1).saturating_sub(chrome_top(h)) as usize)
    }

    /// Polls the lyrics when this screen is up: asks the app when the song changed (or they are
    /// still loading), keeps the clock's floor, and says whether the frame must be redrawn
    /// because a line or word started. No timer: [`App::wait`] sleeps until that moment.
    fn lyrics_tick(&mut self, be: &mut dyn Backend, now: Instant) -> bool {
        if !self.lyrics_wanted() || self.away {
            return false;
        }
        let want = self.playing_id().map(String::from);
        let mut talked = false;
        if want.is_none() {
            if self.lyrics.song.is_some() {
                self.lyrics.clear();
            }
        } else if self.lyrics.due(want.as_deref(), now) {
            self.fetch_lyrics(be, now);
            talked = true;
        }
        let raw = self.position(now);
        let t = self.lyrics.clock(raw);
        self.lyrics.observe(raw);
        talked | self.lyrics.changed(t, now)
    }

    fn fetch_lyrics(&mut self, be: &mut dyn Backend, now: Instant) {
        self.lyrics.asked_now(now);
        match be.request(&Request::Lyrics) {
            Ok(Reply::Lyrics { song_id, status, lines }) => {
                self.away = false;
                self.lyrics.set(song_id, status, lines);
            }
            // An app from before lyrics answers every request it can't read like this.
            Ok(Reply::Error { message }) if message.starts_with("bad request") => self.lyrics.unsupported = true,
            Ok(_) => self.lyrics.failed(),
            Err(_) => self.away = true,
        }
    }

    /// How long the loop may wait for input: [`WAIT`], less when a line or word is about to
    /// start on the lyrics screen, so the redraw comes on time without polling.
    pub fn wait(&self, now: Instant) -> Duration {
        let playing = self.status.as_ref().is_some_and(|st| st.state == PlayState::Playing);
        if self.lyrics_wanted() && playing {
            if let Some(d) = self.lyrics.wake_in(self.lyrics.clock(self.position(now)), now) {
                return d.clamp(Duration::from_millis(1), WAIT);
            }
        }
        WAIT
    }

    /// The lyrics' rows for the clock at `now`, the cursor, and the first row to show.
    pub fn lyrics_layout(&self, now: Instant) -> (Vec<lyrics::Row>, lyrics::Cursor, isize) {
        let (width, height) = self.lyrics_box();
        let cursor = self.lyrics.cursor(self.lyrics.clock(self.position(now)));
        let rows = lyrics::rows(&self.lyrics, width, cursor);
        // Solo has no scrolling: it always follows the song.
        let top = match self.lyrics.manual.filter(|_| !self.solo() || !self.lyrics.synced()) {
            Some(t) => {
                let (lo, hi) = lyrics::scroll_range(rows.len(), height, self.lyrics.synced());
                t.clamp(lo, hi)
            }
            // Plain lyrics in solo all fit: centred in the box.
            None if self.solo() && !self.lyrics.synced() => -(height.saturating_sub(rows.len()) as isize / 2),
            None => lyrics::follow_top(&rows, cursor, height),
        };
        (rows, cursor, top)
    }

    /// Scrolls by hand, from where the view is now.
    fn scroll_lyrics(&mut self, delta: isize, now: Instant) {
        let (_, height) = self.lyrics_box();
        let (rows, _, top) = self.lyrics_layout(now);
        let (lo, hi) = lyrics::scroll_range(rows.len(), height, self.lyrics.synced());
        self.lyrics.manual = Some(top.saturating_add(delta).clamp(lo, hi));
    }

    fn lyrics_key(&mut self, code: KeyCode, be: &mut dyn Backend, now: Instant) {
        let page = self.lyrics_box().1.saturating_sub(1).max(1) as isize;
        match code {
            KeyCode::Esc => self.goto(Screen::Queue, be, now),
            KeyCode::Char('w') => {
                self.lyrics.lines_only = !self.lyrics.lines_only;
                self.lyrics.reopened();
                self.set_message(if self.lyrics.words() { "Lyrics: word by word" } else { "Lyrics: line by line" }, now);
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll_lyrics(1, now),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_lyrics(-1, now),
            KeyCode::PageDown => self.scroll_lyrics(page, now),
            KeyCode::PageUp => self.scroll_lyrics(-page, now),
            KeyCode::Char('g') | KeyCode::Home => {
                self.lyrics.manual = None;
                if self.lyrics.synced() {
                    self.set_message("Following the song", now);
                }
            }
            KeyCode::Char('G') | KeyCode::End => self.scroll_lyrics(isize::MAX / 2, now),
            _ => {}
        }
    }

    // ---------------------------------------------------------------------------------- mouse

    pub fn handle_mouse(&mut self, ev: MouseEvent, be: &mut dyn Backend, now: Instant) {
        if self.help || self.filtering || self.sorting.is_some() || self.solo() {
            return;
        }
        if self.screen != Screen::Queue {
            let delta = match ev.kind {
                MouseEventKind::ScrollUp => -3,
                MouseEventKind::ScrollDown => 3,
                _ => return,
            };
            match self.screen {
                Screen::Search if !self.search.typing => self.search.scroll(delta),
                Screen::Lyrics => self.scroll_lyrics(delta, now),
                Screen::Playlists | Screen::Albums => {
                    let i = usize::from(self.screen == Screen::Albums);
                    if !self.browse[i].filtering {
                        self.browse[i].scroll(delta, now);
                    }
                }
                _ => {}
            }
            return;
        }
        match ev.kind {
            MouseEventKind::ScrollUp => self.move_cursor(-3),
            MouseEventKind::ScrollDown => self.move_cursor(3),
            MouseEventKind::Down(MouseButton::Left) => {
                // The showcase's list is the right column only.
                let (top, inside) = match self.showcase() {
                    Some(g) => (g.queue.y + 1, ev.column >= g.queue.x && ev.column < g.queue.x + g.queue.width),
                    None => (Screen::Queue.body_top(self.height), true),
                };
                let row = (ev.row as usize).checked_sub(top as usize).filter(|r| *r < self.body && inside);
                let vis = self.visible();
                let Some(&index) = row.and_then(|r| vis.get(self.offset + r)) else { return };
                if self.rows().get(index).is_some_and(|r| r.kind.is_divider()) {
                    return;
                }
                self.set_cursor(index);
                let double = self.last_click.is_some_and(|(at, i)| i == index && now.saturating_duration_since(at) < DOUBLE_CLICK);
                self.last_click = if double { None } else { Some((now, index)) };
                if double {
                    self.activate(be, now);
                }
            }
            _ => {}
        }
        self.fit();
    }
}
