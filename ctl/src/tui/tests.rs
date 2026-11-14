//! The screen's keys against the fake app, and its frames on a `TestBackend`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::{Color, Modifier};
use uzumaki_ctl::{serve, Client, Kind, PickAction, PlayState, Queue, RepeatMode, Request, Server, SongInfo, Status};

use super::app::{App, Screen};
use super::jobs::Inline;
use super::table::{CORAL, RowKind};
use super::view::{self, bar_text, short_length};
use crate::fmt::text_width;
use crate::tests::{fake_handler_logged, socket};

// ------------------------------------------------------------------------------------- frames

fn s(id: &str, title: &str, artist: &str, album: &str, secs: f64) -> SongInfo {
    SongInfo { id: id.into(), title: title.into(), artist: artist.into(), album: album.into(), duration: secs, playable: true, ..Default::default() }
}

/// Playing "Instant Crush" at 1:12/3:59, one queued song, then a mix of scripts.
fn fixture(now: Instant) -> App {
    let mut app = App::new(now, Box::new(Inline::new(Box::new(NoApp))));
    let mut gone = s("u1", "Unavailable Song", "Someone", "Somewhere", 180.0);
    gone.playable = false;
    let q = Queue {
        current: Some(s("c", "Instant Crush", "Daft Punk", "Random Access Memories", 239.0)),
        up_next: vec![
            s("a", "Группа крови", "Кино", "Группа крови", 289.0),
            s("b", "夜に駆ける", "YOASOBI", "THE BOOK", 261.0),
            s("c2", "First Love", "宇多田ヒカル", "First Love", 258.0),
            s("d", "Midnight City", "M83", "Hurry Up, We're Dreaming", 244.0),
            gone,
            s("e", "Весна", "Земфира", "Вендетта", 195.0),
            s("f", "Lose Yourself to Dance", "Daft Punk", "Random Access Memories", 353.0),
        ],
        queued: 1,
        source: "Anima".into(),
        shuffle: true,
        repeat: RepeatMode::All,
        next_lap: Vec::new(),
        ..Default::default()
    };
    app.set_queue(q, now);
    app.set_status(Status { state: PlayState::Playing, song: app.queue.current.clone(), position: 72.0, duration: 239.0, volume: 0.82, shuffle: true, repeat: RepeatMode::All, queued: 1, connected: true, ..Default::default() }, now);
    app
}

fn draw(app: &App, now: Instant, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| view::draw(f, app, now)).unwrap();
    t.backend().buffer().clone()
}

/// Screen text by rows; a wide character's second cell is skipped, so each line is as wide as it looks.
fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            let mut out = String::new();
            let mut skip = 0;
            for x in 0..buf.area.width {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let sym = buf[(x, y)].symbol();
                skip = text_width(sym).saturating_sub(1);
                out.push_str(sym);
            }
            out
        })
        .collect()
}

fn screen(w: u16, h: u16) -> (Vec<String>, Buffer) {
    let now = Instant::now();
    let mut app = fixture(now);
    app.resize(h);
    let buf = draw(&app, now, w, h);
    (lines(&buf), buf)
}

#[test]
fn wide_frame_is_the_boxed_player_over_the_table() {
    let (text, _) = screen(140, 20);
    for l in &text {
        println!("{l}");
    }
    assert!(text[0].starts_with('╭') && text[0].ends_with('╮') && text[0].contains('┬'), "{}", text[0]);
    assert!(text[2].starts_with("│  Instant Crush") && text[2].contains("│ Status:  ▶ Playing"), "{}", text[2]);
    assert!(text[3].contains("Daft Punk") && text[4].contains("Random Access Memories"));
    assert!(text[6].contains("uzumaki · ? help") && text[6].contains("Queue:   8 songs · 33 min"), "{}", text[6]);
    assert!(text[1].contains("Volume:  82%") && text[3].contains("Repeat:  all     Shuffle: on") && text[4].contains("Time:    1:12 / 3:59"));
    assert!(text[7].starts_with("├─ 1 Queue ── 2 Playlists ── 3 Search ── 4 Albums ── 5 Lyrics ") && text[7].contains('┴') && text[7].ends_with('┤'), "{}", text[7]);
    assert!(text[8].starts_with("│ Playing from: Anima") && text[9].starts_with('├'));
    for col in ["Artist", "#", "Title", "Album", "Time"] {
        assert!(text[10].contains(col), "{}", text[10]);
    }
    assert!(text[11].contains('▶') && text[11].contains("Instant Crush"));
    assert!(text[12].contains("Кино") && text[12].contains(" 1 "));
    assert!(text[19].starts_with("╰─ Space play/pause · n/p next/prev") && text[19].ends_with('╯'), "{}", text[19]);
    assert!(text.iter().all(|l| text_width(l) == 140));
}

#[test]
fn narrow_frame_drops_album_then_artist() {
    let (text, _) = screen(60, 14);
    for l in &text {
        println!("{l}");
    }
    assert!(text[6].contains("Artist") && !text[6].contains("Album"), "{}", text[6]);
    assert!(text[6].contains("Title") && text[6].contains("Time"));
    let (tiny, _) = screen(40, 14);
    assert!(!tiny[6].contains("Artist") && !tiny[6].contains("Album") && tiny[6].contains("Title") && tiny[6].contains("Time"), "{}", tiny[6]);
    let (wide, _) = screen(140, 20);
    assert!(wide[10].contains("Album"));
}

#[test]
fn wide_characters_keep_columns_aligned() {
    for w in [140u16, 60] {
        let (text, _) = screen(w, 20);
        for row in &text[11..19] {
            assert_eq!(text_width(row), w as usize, "{row:?}");
        }
        // The Time column ends at the right edge on Latin, Cyrillic and Japanese rows alike.
        let end = |row: &str| row.trim_end_matches('│').trim_end().ends_with("4:21") || row.trim_end_matches('│').trim_end().ends_with("4:18");
        assert!(text[13].contains("夜に駆ける") && end(&text[13]), "{}", text[13]);
        assert!(text[14].contains("宇多田ヒカル") || w == 60, "{}", text[14]);
        assert!(end(&text[14]), "{}", text[14]);
    }
}

#[test]
fn row_styles() {
    let now = Instant::now();
    let mut app = fixture(now);
    app.resize(20);
    // Cursor onto up-next row 2 (Japanese), so the playing row and the selection differ.
    let mut be = NoApp;
    for _ in 0..2 {
        app.handle_key(key(KeyCode::Down), &mut be, now);
    }
    let buf = draw(&app, now, 140, 20);
    let playing = buf[(2, 11)].style();
    assert_eq!(playing.fg, Some(Color::Rgb(255, 122, 138)));
    assert!(playing.add_modifier.contains(Modifier::BOLD) && !playing.add_modifier.contains(Modifier::REVERSED));
    let selected = buf[(2, 13)].style();
    assert!(selected.add_modifier.contains(Modifier::REVERSED));
    // Up-next number green, artist default, album cyan, time magenta; unplayable rows dim.
    let at = |y: u16, sym: &str| (0..140).rev().find(|&x| buf[(x, y)].symbol() == sym).unwrap();
    assert_eq!(buf[(at(12, "1"), 12)].style().fg, Some(Color::Green));
    assert_eq!(buf[(2, 12)].style().fg, Some(Color::Reset));
    assert_eq!(buf[(at(12, "и").min(110), 12)].style().fg, Some(Color::Cyan));
    assert!(buf[(2, 16)].style().add_modifier.contains(Modifier::DIM));
    assert_eq!(buf[(at(12, "9"), 12)].style().fg, Some(Color::Magenta));
}

#[test]
fn progress_bar_and_lengths() {
    assert_eq!(bar_text(0.0, 5), ("╸".to_string(), "────".to_string()));
    assert_eq!(bar_text(0.5, 10), ("━━━━━╸".to_string(), "────".to_string()));
    assert_eq!(bar_text(1.0, 4), ("━━━━".to_string(), String::new()));
    assert_eq!(short_length(39720.0), "11 h 2 min");
    assert_eq!(short_length(2280.0), "38 min");
    assert_eq!(short_length(45.0), "45 s");
    assert_eq!(short_length(7200.0), "2 h");
    assert_eq!(short_length(60.0), "1 min");
    let (text, buf) = screen(140, 20);
    // The bar sits in `[ ]` in the status column: 29 columns, 72/239 of them filled.
    assert_eq!(buf[(108, 5)].style().fg, Some(CORAL));
    assert!(text[5].contains("[━━━━━━━━╸─") && text[5].contains("─]"), "{}", text[5]);
}

#[test]
fn clock_ticks_between_polls_and_stops_when_paused() {
    let now = Instant::now();
    let mut app = fixture(now);
    let later = now + Duration::from_secs(5);
    assert!((app.position(later) - 77.0).abs() < 0.01);
    let mut st = app.status.clone().unwrap();
    st.state = PlayState::Paused;
    app.set_status(st, now);
    assert!((app.position(later) - 72.0).abs() < 0.01);
}

#[test]
fn messages_and_away_replace_the_context_line() {
    let now = Instant::now();
    let mut app = fixture(now);
    app.resize(24);
    let ctx = |app: &App, at: Instant| lines(&draw(app, at, 100, 24))[8].clone();
    assert!(ctx(&app, now).starts_with("│ Playing from: Anima"));
    app.set_message("Shuffle: on", now);
    assert!(ctx(&app, now).starts_with("│ Shuffle: on"), "{}", ctx(&app, now));
    // After 2 s the message is gone.
    assert!(ctx(&app, now + Duration::from_secs(3)).starts_with("│ Playing from: Anima"));
    app.away = true;
    assert!(ctx(&app, now).starts_with("│ Uzumaki went away, retrying…"));
    // No source: just "Queue".
    let mut app = fixture(now);
    let mut q = app.queue.clone();
    q.source.clear();
    app.set_queue(q, now);
    app.resize(24);
    assert!(ctx(&app, now).starts_with("│ Queue "), "{}", ctx(&app, now));
}

// ---------------------------------------------------------------------------------------- keys

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

/// A backend for keys that never reach the app.
struct NoApp;

impl crate::run::Backend for NoApp {
    fn request(&mut self, _: &Request) -> std::io::Result<uzumaki_ctl::Reply> {
        Err(std::io::ErrorKind::NotConnected.into())
    }
}

struct Rig {
    _server: Server,
    log: Arc<Mutex<Vec<Request>>>,
    client: Client,
    app: App,
    now: Instant,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let log: Arc<Mutex<Vec<Request>>> = Arc::default();
        let handler = fake_handler_logged(log.clone());
        Rig::serving(name, log, handler)
    }
    fn serving(name: &str, log: Arc<Mutex<Vec<Request>>>, handler: impl Fn(Request) -> uzumaki_ctl::Reply + Send + Sync + 'static) -> Rig {
        let path = socket(&format!("tui-{name}"));
        let server = serve(&path, handler).unwrap();
        let now = Instant::now();
        let jobs = Inline::new(Box::new(Client::connect_to(&path).unwrap()));
        let mut rig = Rig { _server: server, log, client: Client::connect_to(&path).unwrap(), app: App::new(now, Box::new(jobs)), now };
        rig.app.resize(15);
        assert!(rig.app.tick(&mut rig.client, now));
        rig
    }
    fn press(&mut self, keys: &[KeyEvent]) {
        for k in keys {
            self.app.handle_key(*k, &mut self.client, self.now);
        }
    }
    fn last(&self) -> Request {
        self.log.lock().unwrap().last().cloned().unwrap()
    }
    fn sent(&self, req: &Request) -> bool {
        self.log.lock().unwrap().contains(req)
    }
}

#[test]
fn first_tick_loads_status_and_queue_with_the_cursor_on_the_playing_song() {
    let rig = Rig::new("first");
    assert!(rig.app.status.is_some());
    assert_eq!(rig.app.queue.up_next.len(), 29);
    assert_eq!(rig.app.cursor, 0);
    let log = rig.log.lock().unwrap();
    assert_eq!(*log, vec![Request::Status, Request::Queue]);
}

#[test]
fn enter_jumps_to_the_numbered_row() {
    let mut rig = Rig::new("enter");
    rig.press(&[ch('j'), ch('j'), key(KeyCode::Down), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Jump { index: 2 }); // row 3 -> up-next index 2
    // On the playing row Enter restarts the song.
    rig.press(&[ch('o'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Seek { to: uzumaki_ctl::SeekTo::Absolute(0.0) });
}

#[test]
fn edit_keys_send_the_right_requests() {
    let mut rig = Rig::new("edit");
    rig.press(&[ch('d')]); // on the playing row: refused, nothing sent
    assert!(!rig.log.lock().unwrap().iter().any(|r| matches!(r, Request::Remove { .. })));
    assert_eq!(rig.app.message(rig.now), Some("Can't remove the playing song"));
    rig.press(&[ch('j'), ch('j'), ch('d')]);
    assert_eq!(rig.last(), Request::Remove { indices: vec![1] });
    assert_eq!(rig.app.message(rig.now), Some("Removed 1 song"));
    // The reply (a 3-song queue) replaced the list.
    assert_eq!(rig.app.queue.up_next.len(), 3);
    rig.press(&[ch('g'), ch('j'), ch('J')]);
    assert_eq!(rig.last(), Request::Move { from: 0, to: 1 });
    assert_eq!(rig.app.cursor, 2, "the selection follows the moved song");
    rig.press(&[ch('K')]);
    assert_eq!(rig.last(), Request::Move { from: 1, to: 0 });
    assert_eq!(rig.app.cursor, 1);
}

#[test]
fn playback_keys() {
    use uzumaki_ctl::{SeekTo, VolumeTo};
    let mut rig = Rig::new("playback");
    rig.press(&[ch(' ')]);
    assert_eq!(rig.last(), Request::Toggle);
    rig.press(&[ch('n')]);
    assert_eq!(rig.last(), Request::Next);
    rig.press(&[ch('p')]);
    assert_eq!(rig.last(), Request::Prev);
    rig.press(&[key(KeyCode::Right)]);
    assert_eq!(rig.last(), Request::Seek { to: SeekTo::Relative(10.0) });
    rig.press(&[key(KeyCode::Left)]);
    assert_eq!(rig.last(), Request::Seek { to: SeekTo::Relative(-10.0) });
    rig.press(&[ch('+')]);
    assert_eq!(rig.last(), Request::Volume { to: VolumeTo::Change(0.05) });
    rig.press(&[ch('-')]);
    assert_eq!(rig.last(), Request::Volume { to: VolumeTo::Change(-0.05) });
    rig.press(&[ch('s')]);
    assert_eq!(rig.last(), Request::Shuffle { on: None });
    rig.press(&[ch('r')]);
    assert_eq!(rig.last(), Request::Repeat { mode: None });
    // The fake doesn't know Toggle/Volume/Shuffle: its error shows as a message.
    assert!(rig.app.message(rig.now).is_some());
    assert!(rig.sent(&Request::Toggle));
}

#[test]
fn filter_narrows_rows_but_keeps_real_numbers() {
    let mut rig = Rig::new("filter");
    rig.press(&[ch('/'), ch('m'), ch('8'), ch('3')]);
    assert!(rig.app.filtering);
    let rows = rig.app.rows();
    let numbers: Vec<_> = rig.app.visible().iter().map(|&i| rows[i].number).collect();
    assert_eq!(&numbers[..3], &[Some(3), Some(7), Some(11)]);
    drop(rows);
    rig.press(&[key(KeyCode::Enter)]);
    assert!(!rig.app.filtering && rig.app.filter == "m83");
    rig.press(&[ch('j'), key(KeyCode::Enter)]); // the second M83 song is number 7
    assert_eq!(rig.last(), Request::Jump { index: 6 });
    // Esc clears a kept filter before it quits; Esc while typing clears and stops typing.
    rig.press(&[key(KeyCode::Esc)]);
    assert!(rig.app.filter.is_empty() && !rig.app.quit);
    rig.press(&[ch('/'), ch('x'), key(KeyCode::Esc)]);
    assert!(!rig.app.filtering && rig.app.filter.is_empty() && !rig.app.quit);
    // While typing, command keys are text.
    rig.press(&[ch('/'), ch('q')]);
    assert!(!rig.app.quit && rig.app.filter == "q");
}

#[test]
fn quitting_and_help() {
    let mut rig = Rig::new("quit");
    rig.press(&[ch('?')]);
    assert!(rig.app.help);
    let text = lines(&draw(&rig.app, rig.now, 100, 24)).join("\n");
    assert!(text.contains("Keys") && text.contains("play / pause"));
    rig.press(&[ch('q')]); // any key closes the help, it doesn't quit
    assert!(!rig.app.help && !rig.app.quit);
    rig.press(&[key(KeyCode::Esc)]);
    assert!(rig.app.quit);
    let mut rig = Rig::new("ctrlc");
    rig.press(&[KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)]);
    assert!(rig.app.quit);
}

#[test]
fn selection_follows_its_song_when_the_queue_changes() {
    let now = Instant::now();
    let mut app = fixture(now);
    app.resize(15);
    let mut be = NoApp;
    for _ in 0..4 {
        app.handle_key(ch('j'), &mut be, now); // "Midnight City" (number 4)
    }
    assert_eq!(app.rows()[app.cursor].song.id, "d");
    // The song before it is removed: the cursor moves up with it.
    let mut q = app.queue.clone();
    q.up_next.remove(0);
    app.set_queue(q, now);
    assert_eq!(app.rows()[app.cursor].song.id, "d");
    assert_eq!(app.rows()[app.cursor].number, Some(3));
    // Its song is gone: the cursor stays on the place.
    let mut q = app.queue.clone();
    q.up_next.retain(|s| s.id != "d");
    let at = app.cursor;
    app.set_queue(q, now);
    assert_eq!(app.cursor, at);
}

#[test]
fn losing_the_app_sets_away_and_recovery_refetches() {
    let now = Instant::now();
    let mut app = App::new(now, Box::new(Inline::new(Box::new(NoApp))));
    assert!(app.tick(&mut NoApp, now));
    assert!(app.away);
    let mut rig = Rig::new("away");
    rig.app.away = true;
    let later = rig.now + Duration::from_secs(1);
    rig.app.tick(&mut rig.client, later);
    assert!(!rig.app.away);
    assert!(rig.log.lock().unwrap().iter().filter(|r| **r == Request::Queue).count() >= 2);
}

#[test]
fn polling_is_throttled_and_mouse_works() {
    let mut rig = Rig::new("mouse");
    let before = rig.log.lock().unwrap().len();
    assert!(!rig.app.tick(&mut rig.client, rig.now + Duration::from_millis(100)));
    assert_eq!(rig.log.lock().unwrap().len(), before);
    // A status poll after 500 ms; the queue only after 3 s.
    assert!(rig.app.tick(&mut rig.client, rig.now + Duration::from_millis(600)));
    assert_eq!(rig.log.lock().unwrap().len(), before + 1);
    let first = Screen::Queue.body_top(15);
    let click = |row: u16| MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row, modifiers: KeyModifiers::NONE };
    rig.app.handle_mouse(click(first + 3), &mut rig.client, rig.now);
    assert_eq!(rig.app.cursor, 3);
    rig.app.handle_mouse(click(first + 3), &mut rig.client, rig.now + Duration::from_millis(100));
    assert_eq!(rig.last(), Request::Jump { index: 2 });
    let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: 5, row: 6, modifiers: KeyModifiers::NONE };
    rig.app.handle_mouse(wheel, &mut rig.client, rig.now);
    assert_eq!(rig.app.cursor, 6);
}

// ------------------------------------------------------------------- playlists, albums, search

impl Rig {
    /// Lets time pass and ticks, so a rested selection fetches its songs.
    fn advance(&mut self, ms: u64) {
        self.now += Duration::from_millis(ms);
        self.app.tick(&mut self.client, self.now);
    }
    fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }
    fn frame(&mut self, w: u16, h: u16) -> Vec<String> {
        self.app.resize(h);
        let text = lines(&draw(&self.app, self.now, w, h));
        for l in &text {
            println!("{l}");
        }
        text
    }
}

fn playlists_loaded(name: &str) -> Rig {
    let mut rig = Rig::new(name);
    rig.press(&[ch('2')]);
    rig.advance(300);
    rig
}

#[test]
fn screen_two_lists_playlists_and_loads_the_selected_ones_songs() {
    let mut rig = Rig::new("pl-load");
    rig.press(&[ch('2')]);
    assert_eq!(rig.app.screen, Screen::Playlists);
    assert!(rig.requests().contains(&Request::Playlists));
    let text = rig.frame(120, 18);
    assert!(text[8].starts_with("│ Playlists (4)"), "{}", text[8]);
    assert!(text[10].contains("Title") && text[10].contains("Songs"), "{}", text[10]);
    assert!(text[11].contains("Favourite Songs") && text[11].contains("6 songs"), "{}", text[11]);
    assert!(text[11].contains("Loading…"), "songs aren't asked for before the selection rests: {}", text[11]);
    rig.advance(100);
    assert!(!rig.requests().iter().any(|r| matches!(r, Request::Tracks { .. })));
    rig.advance(200);
    assert!(rig.requests().contains(&Request::Tracks { kind: Kind::Playlist, id: "p.fav".into() }));
    let text = rig.frame(120, 18);
    assert!(text[11].contains("Lose Yourself to Dance") && !text[11].contains("Loading…"), "{}", text[11]);
    // Moving on and resting asks for the next one; going back uses the cache.
    rig.press(&[ch('j')]);
    rig.advance(300);
    assert!(rig.requests().contains(&Request::Tracks { kind: Kind::Playlist, id: "p.anima".into() }));
    let asked = rig.requests().len();
    rig.press(&[ch('k')]);
    rig.advance(300);
    let tracks = rig.requests().iter().filter(|r| matches!(r, Request::Tracks { .. })).count();
    assert_eq!(tracks, 2, "{asked}");
}

#[test]
fn pinned_playlists_come_first_and_say_so() {
    let log: Arc<Mutex<Vec<Request>>> = Arc::default();
    let handler = crate::tests::pinned_handler(log.clone());
    let mut rig = Rig::serving("pl-pinned", log, handler);
    rig.press(&[ch('2')]);
    rig.advance(300);
    let text = rig.frame(120, 18);
    assert!(text[11].contains("1  Road Trip Two pinned") && text[11].contains("6 songs"), "{}", text[11]);
    assert!(text[12].contains("2  Anima ;) pinned"), "{}", text[12]);
    assert!(text[13].contains("3  Favourite Songs") && !text[13].contains("pinned"), "{}", text[13]);
    assert!(text[14].contains("4  Road Trip ") && !text[14].contains("pinned"), "{}", text[14]);
    // The numbers are `uzu playlists`' numbers: P on the first plays the pinned one.
    rig.press(&[ch('P')]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Playlist, id: "p.road2".into(), index: None, order: None });
    // Narrow: the tag gives way to the title.
    let text = rig.frame(40, 18);
    assert!(text.iter().any(|l| l.contains("1  Road …")) && !text.iter().any(|l| l.contains("pinned")), "{text:?}");
}

/// Pinned playlists stay on top, in pin order, under every sort; the others sort below them.
/// The numbers are still `uzu playlists`' numbers.
#[test]
fn pinned_playlists_stay_on_top_under_any_sort() {
    let log: Arc<Mutex<Vec<Request>>> = Arc::default();
    let handler = crate::tests::pinned_handler(log.clone());
    let mut rig = Rig::serving("pl-pinned-sort", log, handler);
    rig.press(&[ch('2')]);
    rig.advance(300);
    // The numbers of the four rows, from the first: 1 Road Trip Two (pin), 2 Anima (pin),
    // 3 Favourite Songs, 4 Road Trip.
    let rows = |rig: &mut Rig| -> Vec<String> {
        let text = rig.frame(120, 18);
        text[11..15].iter().map(|l| l.trim_start_matches('│').split_whitespace().next().unwrap().to_string()).collect()
    };
    // By name: "Anima" would sort before "Road Trip Two", but the pins keep their own order.
    rig.press(&[ch(','), ch('n')]);
    assert_eq!(rows(&mut rig), ["1", "2", "3", "4"]);
    rig.press(&[ch(','), ch('N')]);
    assert_eq!(rows(&mut rig), ["1", "2", "4", "3"]);
    // Original order reversed ("newest first") moves only the unpinned ones.
    rig.press(&[ch(','), ch('O')]);
    assert_eq!(rows(&mut rig), ["1", "2", "4", "3"]);
    rig.press(&[ch(','), ch(',')]);
    assert_eq!(rows(&mut rig), ["1", "2", "3", "4"]);
}

#[test]
fn flicking_through_the_list_asks_only_for_the_one_it_rests_on() {
    let mut rig = Rig::new("pl-flick");
    rig.press(&[ch('2')]);
    rig.advance(300);
    rig.press(&[ch('j')]);
    rig.advance(100);
    rig.press(&[ch('j')]);
    rig.advance(100);
    rig.press(&[ch('j')]);
    rig.advance(300);
    let asked: Vec<_> = rig.requests().into_iter().filter(|r| matches!(r, Request::Tracks { .. })).collect();
    assert_eq!(asked, vec![Request::Tracks { kind: Kind::Playlist, id: "p.fav".into() }, Request::Tracks { kind: Kind::Playlist, id: "p.road2".into() }]);
}

#[test]
fn enter_opens_the_list_and_plays_from_a_song() {
    let mut rig = playlists_loaded("pl-play");
    rig.press(&[ch('j'), ch('j')]); // Road Trip
    rig.advance(300);
    rig.press(&[key(KeyCode::Enter)]);
    rig.press(&[ch('j'), ch('j'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Playlist, id: "p.road".into(), index: Some(2), order: None });
    rig.press(&[ch('h'), ch('P')]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Playlist, id: "p.road".into(), index: None, order: None });
    assert_eq!(rig.app.message(rig.now), Some("Playing: Road Trip"));
    // Enter on the left pane moves to the right and sends nothing.
    let n = rig.requests().len();
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.requests().len(), n);
    // P also works from the right pane.
    rig.press(&[ch('P')]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Playlist, id: "p.road".into(), index: None, order: None });
}

#[test]
fn panes_switch_with_arrows_hl_and_tab_and_esc_steps_back() {
    let mut rig = playlists_loaded("pl-focus");
    let focus = |r: &Rig| r.app.browse(Screen::Playlists).focus;
    use super::browse::Focus;
    rig.press(&[key(KeyCode::Right)]);
    assert_eq!(focus(&rig), Focus::Right);
    rig.press(&[key(KeyCode::Left)]);
    assert_eq!(focus(&rig), Focus::Left);
    rig.press(&[ch('l')]);
    assert_eq!(focus(&rig), Focus::Right);
    rig.press(&[ch('h')]);
    rig.press(&[key(KeyCode::Tab)]);
    assert_eq!(focus(&rig), Focus::Right);
    rig.press(&[key(KeyCode::Esc)]);
    assert_eq!(focus(&rig), Focus::Left);
    assert!(!rig.app.quit);
    rig.press(&[key(KeyCode::Esc)]);
    assert_eq!(rig.app.screen, Screen::Queue);
    assert!(!rig.app.quit);
    // Left/Right seek on the queue screen, not on the browse screens.
    rig.press(&[key(KeyCode::Right)]);
    assert!(matches!(rig.last(), Request::Seek { .. }));
}

#[test]
fn filter_keeps_real_numbers_and_works_per_pane() {
    let mut rig = playlists_loaded("pl-filter");
    rig.press(&[ch('/'), ch('r'), ch('o'), ch('a'), ch('d'), ch('q')]); // "q" is text here
    assert!(!rig.app.quit);
    rig.press(&[key(KeyCode::Backspace), key(KeyCode::Enter)]);
    let text = rig.frame(120, 18);
    assert!(text[8].contains("/road"), "{}", text[8]);
    assert!(text[11].contains("3") && text[11].contains("Road Trip"), "{}", text[11]);
    assert!(text[12].contains("4") && text[12].contains("Road Trip Two"), "{}", text[12]);
    assert!(!text.iter().any(|l| l.contains("Favourite")));
    rig.advance(300);
    rig.press(&[key(KeyCode::Esc)]);
    let text = rig.frame(120, 18);
    assert!(text[11].contains("Favourite Songs"), "Esc clears the filter first: {}", text[11]);
    // A filter on the right pane filters songs by title or artist and keeps their numbers.
    rig.press(&[ch('l'), ch('/'), ch('m'), ch('8'), ch('3'), key(KeyCode::Enter)]);
    let text = rig.frame(120, 18);
    assert!(text[11].contains("Midnight City") && text[11].contains(" 4 "), "{}", text[11]);
    assert!(!text[11].contains("Daft Punk"));
    rig.press(&[key(KeyCode::Enter)]);
    assert!(matches!(rig.last(), Request::PlayCollection { index: Some(3), .. }), "{:?}", rig.last());
}

#[test]
fn albums_screen_and_reload() {
    let mut rig = Rig::new("albums");
    rig.press(&[ch('4')]);
    assert_eq!(rig.app.screen, Screen::Albums);
    rig.advance(300);
    let text = rig.frame(120, 18);
    assert!(text[8].starts_with("│ Albums (2)"), "{}", text[8]);
    assert!(text[10].contains("Artist"), "{}", text[10]);
    assert!(text[11].contains("Discovery") && text[11].contains("Daft Punk"), "{}", text[11]);
    assert!(rig.requests().contains(&Request::Tracks { kind: Kind::Album, id: "l.1".into() }));
    rig.press(&[ch('j'), ch('P')]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Album, id: "l.2".into(), index: None, order: None });
    // Opening a loaded screen again doesn't reload; `R` does, and keeps the selection.
    rig.press(&[ch('1'), ch('4')]);
    assert_eq!(rig.requests().iter().filter(|r| **r == Request::Albums).count(), 1);
    rig.press(&[ch('R')]);
    assert_eq!(rig.requests().iter().filter(|r| **r == Request::Albums).count(), 2);
    assert_eq!(rig.app.browse(Screen::Albums).selected().unwrap().id, "l.2");
}

#[test]
fn browse_frames() {
    let mut rig = playlists_loaded("pl-frame");
    rig.press(&[key(KeyCode::Enter)]);
    let text = rig.frame(120, 18);
    assert!(text[7].contains("1 Queue") && text[7].contains("2 Playlists") && text[7].contains("3 Search") && text[7].contains("4 Albums"), "{}", text[7]);
    // The right pane is 78 columns here: Artist, #, Title and Time (Album needs 90).
    for col in ["Artist", "#", "Title", "Time"] {
        assert!(text[10].contains(col), "{}", text[10]);
    }
    assert!(text[11].contains('│') && text[11].contains("Favourite Songs") && text[11].contains("Daft Punk"), "{}", text[11]);
    assert!(text[17].starts_with("╰─ Space play/pause · n/p next/prev · Enter open/play · Tab pane"), "{}", text[17]);
    assert!(text[2].contains("Lose Yourself to Dance"), "{}", text[2]);
    // The current tab is coral and bold, the others dim.
    let buf = draw(&rig.app, rig.now, 120, 18);
    let col = |line: &str, needle: &str| line[..line.find(needle).unwrap()].chars().count() as u16;
    let x = col(&text[7], "2 Playlists");
    assert_eq!(buf[(x, 7)].style().fg, Some(Color::Rgb(255, 122, 138)));
    assert!(buf[(x, 7)].style().add_modifier.contains(Modifier::BOLD));
    let q = col(&text[7], "1 Queue");
    assert!(!buf[(q, 7)].style().add_modifier.contains(Modifier::DIM));
    // The right pane's selected row is inverted and the left one's only underlined (the focus is right).
    let split = col(&text[11][3..], "│") + 1;
    assert!(buf[(split + 3, 11)].style().add_modifier.contains(Modifier::REVERSED));
    assert!(buf[(2, 11)].style().add_modifier.contains(Modifier::UNDERLINED) && !buf[(2, 11)].style().add_modifier.contains(Modifier::REVERSED));
    // The playing song (s0) is bold coral in the right pane.
    rig.press(&[ch('j')]);
    let buf = draw(&rig.app, rig.now, 120, 18);
    assert_eq!(buf[(split + 3, 11)].style().fg, Some(Color::Rgb(255, 122, 138)));
    let narrow = rig.frame(60, 14);
    assert!(narrow[4].starts_with("│ Playlists (4)"));
    assert!(narrow[6].contains("Title") && narrow[6].contains("Time") && !narrow[6].contains("Album") && !narrow[6].contains("Artist"), "{}", narrow[6]);
    for l in &narrow[2..12] {
        assert_eq!(text_width(l), 60, "{l:?}");
    }
}

#[test]
fn browse_help_lists_its_keys() {
    let mut rig = playlists_loaded("pl-help");
    rig.press(&[ch('?')]);
    let text = rig.frame(100, 28).join("\n");
    assert!(text.contains("Keys: Playlists") && text.contains("play the whole list") && text.contains("reload the list") && text.contains("seek +10 s"), "{text}");
}

#[test]
fn songs_that_fail_to_load_show_the_error() {
    // The worker can't reach the app: the pane says so instead of loading forever.
    let mut rig = Rig::new("pl-fail");
    rig.app = App::new(rig.now, Box::new(Inline::new(Box::new(NoApp))));
    rig.press(&[ch('2')]);
    rig.advance(300);
    let text = rig.frame(120, 18);
    assert!(text[11].contains("Couldn't load the songs: Uzumaki isn't reachable"), "{}", text[11]);
}

#[test]
fn search_input_runs_the_query_and_results_number_from_one() {
    let mut rig = Rig::new("search");
    rig.press(&[ch('3')]);
    assert_eq!(rig.app.screen, Screen::Search);
    assert!(rig.app.search.typing, "an empty search screen starts in the input");
    // Typing: command keys are text and nothing reaches the app.
    let n = rig.requests().len();
    rig.press(&[ch('q'), ch('n'), ch('2'), ch('s'), key(KeyCode::Backspace), key(KeyCode::Backspace), key(KeyCode::Backspace), key(KeyCode::Backspace)]);
    assert!(!rig.app.quit && rig.app.screen == Screen::Search && rig.requests().len() == n);
    rig.press(&[ch('d'), ch('a'), ch('f'), ch('t')]);
    let text = rig.frame(120, 18);
    assert!(text[10].starts_with("│ > daft▏"), "{}", text[10]);
    assert!(text[8].starts_with("│ Search: daft▏"), "{}", text[8]);
    rig.press(&[key(KeyCode::Enter)]);
    assert!(rig.requests().contains(&Request::Search { query: "daft".into() }));
    assert!(!rig.app.search.typing);
    let text = rig.frame(120, 18);
    assert!(text[8].starts_with("│ Search: daft (6 results)"), "{}", text[8]);
    assert!(text[11].contains("Title"), "{}", text[11]);
    assert!(text[12].contains('1') && text[12].contains("Lose Yourself to Dance"), "{}", text[12]);
    assert!(text[13].contains('2') && text[13].contains("Instant Crush"), "{}", text[13]);
    // Enter on result 2 plays it, `a` adds, `N` plays next.
    rig.press(&[ch('j'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Pick { indices: vec![1], action: PickAction::Play });
    assert_eq!(rig.app.message(rig.now), Some("Playing: Instant Crush"));
    rig.press(&[ch('j'), ch('a')]);
    assert_eq!(rig.last(), Request::Pick { indices: vec![2], action: PickAction::Add });
    assert_eq!(rig.app.message(rig.now), Some("Added: Around the World"));
    rig.press(&[ch('N')]);
    assert_eq!(rig.last(), Request::Pick { indices: vec![2], action: PickAction::Next });
    assert_eq!(rig.app.message(rig.now), Some("Playing next: Around the World"));
}

#[test]
fn search_keys_while_typing_and_leaving_the_input() {
    let mut rig = Rig::new("search-keys");
    rig.press(&[ch('3'), ch('x'), ch('y'), KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)]);
    assert_eq!(rig.app.search.input, "");
    assert!(!rig.app.quit);
    rig.press(&[ch('a'), key(KeyCode::Enter)]);
    // Esc leaves the input but keeps the results; `/` starts a new query; Esc again goes to the queue.
    rig.press(&[ch('/'), ch('z'), key(KeyCode::Esc)]);
    assert!(!rig.app.search.typing && rig.app.search.results.len() == 6 && rig.app.screen == Screen::Search);
    rig.press(&[ch('2')]);
    assert_eq!(rig.app.screen, Screen::Playlists);
    rig.press(&[ch('3')]);
    assert!(!rig.app.search.typing, "with results the input isn't opened");
    rig.press(&[key(KeyCode::Esc)]);
    assert_eq!(rig.app.screen, Screen::Queue);
    // Empty input does nothing.
    rig.press(&[ch('3'), ch('/'), key(KeyCode::Enter)]);
    assert_eq!(rig.requests().iter().filter(|r| matches!(r, Request::Search { .. })).count(), 1);
}

#[test]
fn a_search_is_recorded_for_uzu_play_and_stale_answers_are_dropped() {
    let mut rig = Rig::new("search-last");
    let path = socket("tui-search-last").with_extension("last");
    rig.app.last = Some(path.clone());
    rig.press(&[ch('3'), ch('a'), key(KeyCode::Enter)]);
    rig.app.tick(&mut rig.client, rig.now);
    assert_eq!(crate::last::load(&path), Some(crate::last::Last::Search));
    // An answer for an older search changes nothing.
    assert!(!rig.app.search.done(0, Ok(vec![])));
    assert_eq!(rig.app.search.results.len(), 6);
}

#[test]
fn search_frames() {
    let mut rig = Rig::new("search-frame");
    rig.press(&[ch('3')]);
    let empty = rig.frame(120, 18);
    assert!(empty[10].starts_with("│ > ▏") && empty[12].contains("Type a query"), "{}", empty[10]);
    rig.press(&[ch('d'), ch('a'), key(KeyCode::Enter)]);
    let text = rig.frame(120, 18);
    assert!(text[7].contains("3 Search"));
    let buf = draw(&rig.app, rig.now, 120, 18);
    // Result 1 is the playing song: bold coral, and the selection inverts it.
    assert_eq!(buf[(2, 12)].style().fg, Some(Color::Rgb(255, 122, 138)));
    assert!(buf[(2, 12)].style().add_modifier.contains(Modifier::REVERSED));
    assert!(text[17].starts_with("╰─ Space play/pause · n/p next/prev · Enter play · / new query"), "{}", text[17]);
    let narrow = rig.frame(60, 14);
    assert!(narrow[4].starts_with("│ Search: da (6 results)") && narrow[7].contains("Title") && !narrow[7].contains("Album"), "{}", narrow[7]);
}

// -------------------------------------------------------------------------------- play state

fn in_state(state: PlayState, now: Instant) -> App {
    let mut app = fixture(now);
    let mut st = app.status.clone().unwrap();
    st.state = state;
    if state == PlayState::Stopped {
        st.position = 0.0;
    }
    app.set_status(st, now);
    app.resize(12);
    app
}

/// Row of the first status field on a full-height frame.
const FIELDS: usize = 1;

#[test]
fn each_state_shows_in_the_status_column_bar_and_glyph() {
    let now = Instant::now();
    let cases = [
        (PlayState::Playing, "Status:  ▶ Playing", "▶"),
        (PlayState::Paused, "Status:  ‖ Paused", "‖"),
        (PlayState::Loading, "Status:  … Loading", "‖"),
        (PlayState::Stopped, "Status:  ■ Stopped", "■"),
    ];
    for (state, status, glyph) in cases {
        let mut app = in_state(state, now);
        app.resize(24);
        let buf = draw(&app, now, 100, 24);
        let text = lines(&buf);
        assert!(text[FIELDS].contains("Volume:  82%"), "{}", text[FIELDS]);
        assert!(text[FIELDS + 1].contains(status), "{state:?}: {}", text[FIELDS + 1]);
        assert!(text[FIELDS + 2].contains("Repeat:  all     Shuffle: on"), "{}", text[FIELDS + 2]);
        assert!(text[FIELDS + 3].contains(if state == PlayState::Stopped { "Time:    0:00 / 3:59" } else { "Time:    1:12 / 3:59" }), "{}", text[FIELDS + 3]);
        assert!(text[11].contains(glyph), "{state:?}: {}", text[11]);
        let word = &buf[(76, 2)];
        let title = &buf[(3, 2)];
        let bar = &buf[(68, 5)];
        let tail = &buf[(96, 5)];
        assert_eq!(buf[(67, 5)].symbol(), "[");
        assert!(!tail.modifier.contains(Modifier::DIM), "the unplayed part is the plain foreground, so it shows on light themes");
        match state {
            PlayState::Playing => {
                assert_eq!((bar.fg, bar.modifier.contains(Modifier::DIM)), (CORAL, false));
                assert!(word.fg == CORAL);
                assert!(title.fg == CORAL && title.modifier.contains(Modifier::BOLD));
            }
            PlayState::Paused => {
                assert!(!bar.modifier.contains(Modifier::DIM) && bar.fg != CORAL);
                assert!(word.fg == Color::Yellow);
                assert!(title.fg != CORAL && title.modifier.contains(Modifier::BOLD));
            }
            _ => {
                assert!(!bar.modifier.contains(Modifier::DIM) && bar.fg != CORAL);
                assert!(word.modifier.contains(Modifier::DIM));
                assert!(title.fg != CORAL);
            }
        }
    }
}

#[test]
fn nothing_playing_is_plain_and_the_status_follows_on_every_screen() {
    let now = Instant::now();
    let mut app = in_state(PlayState::Stopped, now);
    let mut st = app.status.clone().unwrap();
    st.song = None;
    app.set_status(st, now);
    app.resize(24);
    let text = lines(&draw(&app, now, 100, 24));
    assert!(text[2].contains("Nothing playing") && text[2].contains("■ Stopped") && text[4].contains("--:-- / --:--"), "{}", text[2]);
    let mut app = in_state(PlayState::Paused, now);
    app.resize(24);
    for screen in Screen::ALL {
        app.screen = screen;
        let text = lines(&draw(&app, now, 100, 24));
        assert!(text[2].contains("Instant Crush") && text[2].contains("‖ Paused"), "{screen:?}: {}", text[2]);
        assert!(text[1].contains("Volume:  82%") && text[7].contains("1 Queue"), "{screen:?}");
    }
}

#[test]
fn paused_and_stopped_clocks_do_not_tick() {
    let now = Instant::now();
    let later = now + Duration::from_secs(5);
    for (state, at) in [(PlayState::Paused, 72.0), (PlayState::Stopped, 0.0), (PlayState::Loading, 72.0), (PlayState::Playing, 77.0)] {
        assert_eq!(in_state(state, now).position(later), at, "{state:?}");
    }
}

#[test]
fn space_flips_the_state_at_once_and_quick_polls_confirm() {
    let mut rig = Rig::new("space");
    let state = |rig: &Rig| rig.app.status.as_ref().unwrap().state;
    assert_eq!(state(&rig), PlayState::Playing);
    let pos = rig.app.position(rig.now);
    rig.press(&[ch(' ')]);
    assert_eq!(rig.last(), Request::Toggle);
    assert_eq!(state(&rig), PlayState::Paused, "optimistic flip");
    assert_eq!(rig.app.message(rig.now), Some("Paused"));
    assert_eq!(rig.app.position(rig.now + Duration::from_secs(3)), pos, "the paused clock stands still");
    // No poll right away (it would undo the guess), then +300 ms and +1 s, then the 500 ms cadence.
    let polls = |rig: &Rig| rig.requests().iter().filter(|r| **r == Request::Status).count();
    let n = polls(&rig);
    rig.advance(100);
    assert_eq!(polls(&rig), n);
    rig.advance(200);
    assert_eq!(polls(&rig), n + 1, "+300 ms");
    assert_eq!(state(&rig), PlayState::Paused, "the real status agrees");
    rig.advance(100);
    assert_eq!(polls(&rig), n + 1);
    rig.advance(600);
    assert_eq!(polls(&rig), n + 2, "+1 s");
    // Space again: back to playing, and the fake (paused now) is overridden by the optimistic Playing.
    rig.press(&[ch(' ')]);
    assert_eq!(state(&rig), PlayState::Playing);
    assert_eq!(rig.app.message(rig.now), Some("Playing"));
    rig.advance(300);
    assert_eq!(state(&rig), PlayState::Playing);
}

#[test]
fn the_guess_survives_stale_statuses_until_confirmed_or_expired() {
    let mut rig = Rig::new("guess");
    let state = |rig: &Rig| rig.app.status.as_ref().unwrap().state;
    rig.press(&[ch(' ')]); // Playing, guessed Paused
    let mut st = rig.app.status.clone().unwrap();
    st.state = PlayState::Playing; // MusicKit hasn't paused yet
    rig.app.set_status(st.clone(), rig.now + Duration::from_millis(300));
    assert_eq!(state(&rig), PlayState::Paused, "a stale status doesn't flip it back");
    rig.app.set_status(st.clone(), rig.now + Duration::from_secs(2));
    assert_eq!(state(&rig), PlayState::Playing, "after 1.5 s the app's word wins");
    // Another state (here Loading) wins at once.
    rig.press(&[ch(' ')]);
    st.state = PlayState::Loading;
    rig.app.set_status(st.clone(), rig.now + Duration::from_millis(300));
    assert_eq!(state(&rig), PlayState::Loading);
    // From stopped, Space shows Playing.
    st.state = PlayState::Stopped;
    rig.app.set_status(st, rig.now + Duration::from_secs(5));
    rig.press(&[ch(' ')]);
    assert_eq!(state(&rig), PlayState::Playing);
}

#[test]
fn other_actions_also_poll_at_300_ms_and_1_s() {
    for (name, keys) in [("n", vec![ch('n')]), ("p", vec![ch('p')]), ("seek", vec![key(KeyCode::Right)]), ("enter", vec![ch('j'), key(KeyCode::Enter)])] {
        let mut rig = Rig::new(&format!("quick-{name}"));
        rig.advance(600); // settle the regular cadence
        rig.press(&keys);
        let polls = |rig: &Rig| rig.requests().iter().filter(|r| **r == Request::Status).count();
        let n = polls(&rig);
        rig.advance(10); // the immediate poll
        let n1 = polls(&rig);
        rig.advance(290);
        assert_eq!(polls(&rig), n1 + 1, "{name}: +300 ms");
        rig.advance(700);
        assert!(polls(&rig) >= n1 + 2, "{name}: +1 s ({n})");
    }
}

// ------------------------------------------------------------------------------- the new frame

/// The fixture with the playing song 2:24 long, as in the design.
fn short_song(now: Instant) -> App {
    let mut app = fixture(now);
    let mut q = app.queue.clone();
    q.current.as_mut().unwrap().duration = 144.0;
    app.set_queue(q, now);
    let mut st = app.status.clone().unwrap();
    st.song = app.queue.current.clone();
    st.duration = 144.0;
    st.volume = 0.95;
    app.set_status(st, now);
    app
}

fn show(app: &mut App, now: Instant, w: u16, h: u16) -> Vec<String> {
    app.resize(h);
    let text = lines(&draw(app, now, w, h));
    for l in &text {
        println!("{l}");
    }
    text
}

#[test]
fn tabs_are_written_into_the_rule_and_the_current_one_is_coral() {
    let now = Instant::now();
    let mut app = short_song(now);
    for screen in Screen::ALL {
        app.screen = screen;
        let text = show(&mut app, now, 100, 24);
        let buf = draw(&app, now, 100, 24);
        for other in Screen::ALL {
            let label = format!("{} {}", other.number(), other.name());
            let x = text[7].find(&label).map(|i| text[7][..i].chars().count() as u16).unwrap_or_else(|| panic!("{label}: {}", text[7]));
            let style = buf[(x, 7)].style();
            if other == screen {
                assert_eq!(style.fg, Some(CORAL), "{label}");
                assert!(style.add_modifier.contains(Modifier::BOLD));
            } else {
                assert!(!style.add_modifier.contains(Modifier::DIM) && !style.add_modifier.contains(Modifier::BOLD) && style.fg != Some(CORAL), "{label}");
            }
        }
    }
    // Squeezed, the other tabs shrink to their numbers and the current one keeps its name.
    app.screen = Screen::Search;
    let text = show(&mut app, now, 40, 14);
    assert!(text[3].contains("3 Search") && !text[3].contains("Queue") && text[3].contains("1"), "{}", text[3]);
}

#[test]
fn every_screen_has_its_context_line() {
    let mut rig = Rig::new("context");
    assert!(rig.frame(100, 24)[8].starts_with("│ Playing from: Anima"));
    rig.press(&[ch('2')]);
    assert!(rig.frame(100, 24)[8].starts_with("│ Playlists (4)"));
    rig.press(&[ch('4')]);
    assert!(rig.frame(100, 24)[8].starts_with("│ Albums (2)"));
    rig.press(&[ch('3'), ch('d'), ch('a'), key(KeyCode::Enter)]);
    assert!(rig.frame(100, 24)[8].starts_with("│ Search: da (6 results)"));
    // A message replaces it for two seconds.
    rig.press(&[key(KeyCode::Enter)]);
    assert!(rig.frame(100, 24)[8].starts_with("│ Playing: "), "{}", rig.frame(100, 24)[8]);
    rig.now += Duration::from_secs(3);
    assert!(rig.frame(100, 24)[8].starts_with("│ Search: da"));
}

#[test]
fn hints_follow_the_screen_and_the_typing_mode() {
    let mut rig = Rig::new("hints");
    let bottom = |rig: &mut Rig, w: u16| rig.frame(w, 24).pop().unwrap();
    assert!(bottom(&mut rig, 120).starts_with("╰─ Space play/pause · n/p next/prev · Enter play · d remove · J/K move · / filter · F view · q quit "));
    rig.press(&[ch('2')]);
    assert!(bottom(&mut rig, 120).contains("Enter open/play · Tab pane · P play all"));
    rig.press(&[ch('/')]);
    assert!(bottom(&mut rig, 120).starts_with("╰─ Enter keep · Esc clear"), "{}", bottom(&mut rig, 120));
    rig.press(&[key(KeyCode::Esc), ch('3')]);
    assert!(bottom(&mut rig, 120).starts_with("╰─ Enter search · Esc cancel"), "the empty search opens typing");
    rig.press(&[key(KeyCode::Esc)]);
    assert!(bottom(&mut rig, 120).contains("/ new query · a add · N next"));
    rig.press(&[ch('1'), ch('/')]);
    assert!(bottom(&mut rig, 120).starts_with("╰─ Enter keep"));
    // Cut to the width, whole hints only.
    rig.press(&[key(KeyCode::Esc), key(KeyCode::Esc)]);
    let narrow = bottom(&mut rig, 50);
    assert!(narrow.starts_with("╰─ Space play/pause · n/p next/prev ─") && !narrow.contains("Enter") && text_width(&narrow) == 50, "{narrow}");
}

#[test]
fn scrollbar_shows_only_when_the_list_is_longer_than_the_view() {
    let now = Instant::now();
    let mut app = fixture(now);
    // 8 rows: 8 fit at 20 high (no bar), 5 fit at 13 (bar).
    let none = show(&mut app, now, 100, 20);
    assert!(none.iter().all(|l| !l.contains('▲') && !l.contains('▼')));
    let text = show(&mut app, now, 100, 13);
    // Compact player: header at row 6, bar from the header row down in the column before the border.
    let col: Vec<String> = text.iter().map(|l| l.chars().rev().nth(1).unwrap().to_string()).collect();
    assert_eq!(col[6], "▲");
    assert_eq!(col[7], "█");
    assert_eq!(col[text.len() - 2], "▼");
    // At the bottom the thumb is at the end.
    app.handle_key(key(KeyCode::End), &mut NoApp, now);
    let text = show(&mut app, now, 100, 13);
    let col: Vec<String> = text.iter().map(|l| l.chars().rev().nth(1).unwrap().to_string()).collect();
    assert_eq!(col[text.len() - 3], "█");
    // The browse screens: the songs pane's bar sits at the right edge, the list's on the divider.
    let mut rig = playlists_loaded("scrollbar");
    let text = rig.frame(100, 13);
    assert!(text[6..].iter().any(|l| l.contains('▲')), "{text:?}");
    assert!(text[7..12].iter().all(|l| l.matches('│').count() >= 3), "the divider stays");
}

#[test]
fn compact_player_for_short_terminals() {
    let now = Instant::now();
    let mut app = short_song(now);
    let text = show(&mut app, now, 100, 14);
    assert!(text[1].starts_with("│  ▶ Instant Crush — Daft Punk  ") && text[1].trim_end_matches('│').trim_end().ends_with("1:12 / 2:24  vol 95%  [r] [z]"), "{}", text[1]);
    assert!(text[2].contains("━╸─") && !text[2].contains('['), "{}", text[2]);
    assert!(text[3].starts_with("├─ 1 Queue") && !text[3].contains('┴'), "{}", text[3]);
    assert!(text.iter().all(|l| text_width(l) == 100 && !l.contains("Volume:")));
    // Width limits drop the volume, then the flags.
    let text = show(&mut app, now, 44, 14);
    assert!(text[1].contains("1:12 / 2:24  [r] [z]") && !text[1].contains("vol"), "{}", text[1]);
    // The state shows in the glyph; the playing row is still on the table.
    for (state, glyph) in [(PlayState::Paused, "‖"), (PlayState::Loading, "…"), (PlayState::Stopped, "■")] {
        let mut st = app.status.clone().unwrap();
        st.state = state;
        app.set_status(st, now);
        let text = show(&mut app, now, 100, 14);
        assert!(text[1].starts_with(&format!("│  {glyph} Instant Crush")), "{state:?}: {}", text[1]);
    }
    // Nothing playing.
    let mut st = app.status.clone().unwrap();
    st.song = None;
    app.set_status(st, now);
    let text = show(&mut app, now, 100, 14);
    assert!(text[1].contains("Nothing playing") && text[1].contains("--:-- / --:--"), "{}", text[1]);
}

#[test]
fn narrow_but_tall_folds_the_status_into_the_player() {
    let now = Instant::now();
    let mut app = short_song(now);
    let text = show(&mut app, now, 60, 22);
    assert!(text.iter().all(|l| !l.contains("Volume:") && !l.contains('┬') && !l.contains('┴')));
    assert!(text[1].contains("Instant Crush") && text[2].contains("Daft Punk") && text[3].contains("Random Access"));
    assert!(text[4].contains("▶ 1:12 / 2:24  vol 95%  [r] [z]") && text[5].contains("━╸─") && text[6].contains("uzumaki · ? help"), "{}", text[4]);
}

#[test]
fn tiny_terminals_never_panic() {
    let now = Instant::now();
    let mut app = short_song(now);
    for screen in Screen::ALL {
        app.screen = screen;
        for (w, h) in [(30, 8), (1, 1), (2, 2), (3, 5), (10, 3), (5, 30), (30, 1), (200, 4), (69, 17), (70, 18), (40, 40)] {
            app.resize(h);
            let _ = draw(&app, now, w, h);
        }
    }
    let mut rig = Rig::new("tiny");
    for key_ in ['2', '4', '3'] {
        rig.press(&[ch(key_)]);
        for (w, h) in [(30, 8), (8, 8), (12, 6), (4, 4)] {
            rig.frame(w, h);
        }
    }
    rig.press(&[ch('?')]);
    rig.frame(30, 8);
    let text = show(&mut short_song(now), now, 30, 8);
    assert_eq!(text.len(), 8);
    assert!(text[7].starts_with("╰─ Space play/pause"));
}

// ------------------------------------------------------------------------------------- sorting

use super::sort::{Key, Sort, View};

fn sort_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("uzu-sort-tui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("uzumaki").join("uzu.json")
}

fn songs_pane(name: &str) -> Rig {
    let mut rig = playlists_loaded(name);
    rig.press(&[ch('l')]);
    rig
}

/// The play-order part of the last `PlayCollection`.
fn played(rig: &Rig) -> (Option<usize>, Option<Vec<usize>>) {
    match rig.last() {
        Request::PlayCollection { index, order, .. } => (index, order),
        other => panic!("{other:?}"),
    }
}

/// Titles on the song rows of a 100x24 frame of screen 2/4 (the rows under the header).
fn song_rows(text: &[String]) -> Vec<String> {
    text[11..23].iter().map(|l| l.split('│').nth(2).unwrap_or("").trim().to_string()).collect()
}

#[test]
fn the_popup_keys_sort_reverse_restore_and_close() {
    let mut rig = songs_pane("sort-keys");
    let sort = |r: &Rig| r.app.sort_of(View::Songs);
    assert_eq!(sort(&rig), None);
    rig.press(&[ch(',')]);
    assert_eq!(rig.app.sorting, Some(View::Songs));
    rig.press(&[ch('t')]);
    assert_eq!(rig.app.sorting, None);
    assert_eq!(sort(&rig), Some(Sort { key: Key::Title, desc: false }));
    rig.press(&[ch(','), ch('T')]);
    assert_eq!(sort(&rig), Some(Sort { key: Key::Title, desc: true }));
    rig.press(&[ch(','), ch('d')]);
    assert_eq!(sort(&rig), Some(Sort { key: Key::Duration, desc: false }));
    // `,,` is the original order.
    rig.press(&[ch(','), ch(',')]);
    assert_eq!(sort(&rig), None);
    // Esc and keys the popup doesn't list close it and change nothing (and are not commands).
    rig.press(&[ch(','), ch('a')]);
    let artist = sort(&rig);
    assert_eq!(artist, Some(Sort { key: Key::Artist, desc: false }));
    let sent = rig.requests().len();
    rig.press(&[ch(','), key(KeyCode::Esc)]);
    assert!(rig.app.sorting.is_none() && sort(&rig) == artist && rig.app.screen == Screen::Playlists);
    for c in ['x', 'n', 'q', 'c', '3'] {
        rig.press(&[ch(','), ch(c)]);
        assert!(rig.app.sorting.is_none() && sort(&rig) == artist, "{c}");
    }
    assert!(!rig.app.quit && rig.app.screen == Screen::Playlists);
    assert_eq!(rig.requests().len(), sent, "the key after `,` is not a command");
    // On the left pane the popup is the playlists' own.
    rig.press(&[ch('h'), ch(','), ch('n')]);
    assert_eq!(rig.app.sort_of(View::Playlists), Some(Sort { key: Key::Name, desc: false }));
    assert_eq!(sort(&rig), artist);
}

#[test]
fn original_order_can_be_reversed_and_restored() {
    let mut rig = songs_pane("sort-newest");
    let sort = |r: &Rig| r.app.sort_of(View::Songs);
    rig.press(&[ch(','), ch('O')]);
    assert_eq!(sort(&rig), Some(Sort { key: Key::Original, desc: true }));
    assert!(rig.frame(100, 24).iter().any(|l| l.contains("· newest first")));
    rig.press(&[ch(','), ch('o')]);
    assert_eq!(sort(&rig), None);
    rig.press(&[ch(','), ch('O'), ch(','), ch(',')]);
    assert_eq!(sort(&rig), None);
    // Enter on row 1 plays the last song first, in the reversed order.
    rig.press(&[ch(','), ch('O'), ch('g'), key(KeyCode::Enter)]);
    assert_eq!(played(&rig), (Some(0), Some(vec![5, 4, 3, 2, 1, 0])));
    rig.press(&[ch('P')]);
    assert_eq!(played(&rig), (None, Some(vec![5, 4, 3, 2, 1, 0])));
    rig.press(&[ch(','), ch('o'), ch('g'), key(KeyCode::Enter)]);
    assert_eq!(played(&rig), (Some(0), None));
}

#[test]
fn the_queue_is_not_sortable() {
    let mut rig = Rig::new("sort-queue");
    rig.press(&[ch(',')]);
    assert!(rig.app.sorting.is_none());
    assert_eq!(rig.app.message(rig.now), Some("The queue plays in order; sort a playlist instead"));
    // The next key is an ordinary one.
    rig.press(&[ch('j')]);
    assert_eq!(rig.app.cursor, 1);
    // A filter being typed takes `,` as text.
    rig.press(&[ch('/'), ch(',')]);
    assert_eq!(rig.app.filter, ",");
}

#[test]
fn sorting_the_list_keeps_the_selection_and_the_numbers() {
    let mut rig = playlists_loaded("sort-left");
    rig.press(&[ch('j'), ch('j')]); // Road Trip
    rig.press(&[ch(','), ch('N')]);
    let text = rig.frame(100, 24);
    assert!(text[8].starts_with("│ Playlists (4) · by name ↓"), "{}", text[8]);
    let names: Vec<&str> = ["Road Trip Two", "Road Trip", "Favourite Songs", "Anima"].to_vec();
    for (i, n) in names.iter().enumerate() {
        assert!(text[11 + i].contains(n), "{}", text[11 + i]);
    }
    // The numbers are the original ones, and the selection is still Road Trip.
    assert!(text[11].trim_start_matches('│').trim_start().starts_with('4'), "{}", text[11]);
    assert!(text[12].trim_start_matches('│').trim_start().starts_with('3'), "{}", text[12]);
    assert_eq!(rig.app.browse(Screen::Playlists).selected().unwrap().id, "p.road");
    rig.press(&[ch('j')]);
    assert_eq!(rig.app.browse(Screen::Playlists).selected().unwrap().id, "p.fav");
    rig.press(&[ch(','), ch('n'), ch('g')]);
    assert_eq!(rig.app.browse(Screen::Playlists).selected().unwrap().id, "p.anima");
    let text = rig.frame(100, 24);
    assert!(text[8].starts_with("│ Playlists (4) · by name ↑") && text[11].contains("Anima") && text[12].contains("Favourite"), "{}", text[8]);
    // Song count: all have six, so the order stays the original one.
    rig.press(&[ch(','), ch('c')]);
    assert_eq!(rig.frame(100, 24)[8].trim_end_matches(|c| c == ' ' || c == '│'), "│ Playlists (4) · by song count ↑".to_string());
    rig.press(&[ch(','), ch(',')]);
    assert_eq!(rig.frame(100, 24)[8].trim_end_matches(|c| c == ' ' || c == '│'), "│ Playlists (4)");
}

#[test]
fn the_songs_pane_sorts_filters_and_shows_original_numbers() {
    let mut rig = songs_pane("sort-songs");
    rig.press(&[ch(','), ch('D')]);
    let text = rig.frame(100, 24);
    assert!(text[8].starts_with("│ Favourite Songs · by duration ↓"), "{}", text[8]);
    // Durations 200 + 7 i: song 6 (index 5) is the longest. Numbers stay 1-6 as in `uzu playlist`.
    let nums: Vec<&str> = (11..17).map(|y| text[y].split('│').nth(2).unwrap().split_whitespace().find(|t| t.chars().all(|c| c.is_ascii_digit())).unwrap()).collect();
    assert_eq!(nums, ["6", "5", "4", "3", "2", "1"], "{:?}", &text[11..17]);
    // Filter first, then sort: "lose" leaves songs 1 and 5, longest first.
    rig.press(&[ch('/'), ch('l'), ch('o'), ch('s'), ch('e'), key(KeyCode::Enter)]);
    let text = rig.frame(100, 24);
    let right = |y: usize| text[y].split('│').nth(2).unwrap_or("").to_string();
    assert!(right(11).contains("Lose Yourself") && right(11).contains(" 5 ") && right(12).contains("Lose Yourself") && right(12).contains(" 1 ") && right(13).trim().is_empty(), "{:?}", &text[11..14]);
    assert!(text[8].contains("/lose"), "{}", text[8]);
    // The context shows the filter after the sort.
    assert!(text[8].contains("by duration ↓"), "{}", text[8]);
}

#[test]
fn the_selected_song_stays_selected_when_the_sort_changes() {
    let mut rig = songs_pane("sort-follow");
    rig.press(&[ch('j'), ch('j')]); // song 3 ("Around the World")
    rig.press(&[ch(','), ch('T')]);
    rig.press(&[key(KeyCode::Enter)]);
    // Original index 2 is at position 5 of the title-descending order, so it is index 5 of `order`.
    assert_eq!(played(&rig), (Some(5), Some(vec![3, 0, 4, 1, 5, 2])));
}

#[test]
fn enter_and_p_on_a_sorted_pane_send_the_visible_order() {
    let mut rig = songs_pane("sort-play");
    // Title descending: Midnight City (3), Lose Yourself (0, 4), Instant Crush (1, 5), Around (2).
    rig.press(&[ch(','), ch('T'), ch('g'), ch('j')]);
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Playlist, id: "p.fav".into(), index: Some(1), order: Some(vec![3, 0, 4, 1, 5, 2]) });
    rig.press(&[ch('P')]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Playlist, id: "p.fav".into(), index: None, order: Some(vec![3, 0, 4, 1, 5, 2]) });
    assert_eq!(rig.app.message(rig.now), Some("Playing: Favourite Songs"));
    // From the left pane too, and with a filter the whole sorted list still plays.
    rig.press(&[ch('h'), ch('P')]);
    assert_eq!(played(&rig), (None, Some(vec![3, 0, 4, 1, 5, 2])));
    // Back to the original order: nothing extra is sent.
    rig.press(&[ch('l'), ch(','), ch(','), ch('g'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Playlist, id: "p.fav".into(), index: Some(0), order: None });
    rig.press(&[ch('P')]);
    assert_eq!(played(&rig), (None, None));
    // Sorting the list of playlists changes no song order.
    rig.press(&[ch('h'), ch(','), ch('N'), ch('P')]);
    assert_eq!(played(&rig), (None, None));
}

#[test]
fn the_songs_choice_is_shared_by_albums_and_playlists() {
    let mut rig = songs_pane("sort-shared");
    rig.press(&[ch(','), ch('D'), ch('4')]);
    rig.advance(300);
    rig.press(&[ch('P')]);
    assert_eq!(rig.last(), Request::PlayCollection { kind: Kind::Album, id: "l.1".into(), index: None, order: Some(vec![5, 4, 3, 2, 1, 0]) });
    // The albums list has its own keys.
    rig.advance(3000); // the "Playing" message is gone
    rig.press(&[ch(','), ch('Y')]);
    assert_eq!(rig.app.sort_of(View::Albums), Some(Sort { key: Key::Year, desc: true }));
    let text = rig.frame(100, 24);
    assert!(text[8].starts_with("│ Albums (2) · by year ↓"), "{}", text[8]);
}

#[test]
fn a_sorted_search_picks_the_original_result() {
    let mut rig = Rig::new("sort-search");
    rig.press(&[ch('3'), ch('d'), ch('a'), key(KeyCode::Enter)]);
    rig.press(&[ch(','), ch('D')]);
    assert_eq!(rig.app.sorting, None);
    let text = rig.frame(100, 24);
    assert!(text[8].starts_with("│ Search: da (6 results) · by duration ↓"), "{}", text[8]);
    // Result 6 is the longest and shows first, still numbered 6.
    assert!(text[12].contains(" 6 ") && text[12].contains("Instant Crush"), "{}", text[12]);
    assert!(text[17].contains(" 1 ") && text[17].contains("Lose Yourself"), "{}", text[17]);
    // The cursor follows the result it was on (result 1), the keys walk the shown order.
    rig.press(&[ch('g'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Pick { indices: vec![5], action: PickAction::Play });
    rig.press(&[ch('j'), ch('a')]);
    assert_eq!(rig.last(), Request::Pick { indices: vec![4], action: PickAction::Add });
    assert_eq!(rig.app.message(rig.now), Some("Added: Lose Yourself to Dance"));
    rig.press(&[ch('N')]);
    assert_eq!(rig.last(), Request::Pick { indices: vec![4], action: PickAction::Next });
    rig.press(&[ch('G'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Pick { indices: vec![0], action: PickAction::Play });
    // `,` is text while typing a query, and a sort needs no results to be set.
    rig.press(&[ch('/'), ch(',')]);
    assert_eq!(rig.app.search.input, ",");
    assert!(rig.app.sorting.is_none());
}

#[test]
fn choices_are_remembered_between_runs() {
    let path = sort_file("remember");
    let mut rig = songs_pane("sort-remember");
    rig.app.use_sort_file(path.clone());
    rig.press(&[ch(','), ch('D'), ch('h'), ch(','), ch('N'), ch('3'), ch('d'), ch('a'), key(KeyCode::Enter), ch(','), ch('a')]);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"songs\"") && text.contains("\"duration\""), "{text}");
    // A new run starts with them: lists, songs and search come up sorted, the first row selected.
    let mut next = Rig::new("sort-remember-2");
    next.app.use_sort_file(path.clone());
    assert_eq!(next.app.sort_of(View::Playlists), Some(Sort { key: Key::Name, desc: true }));
    assert_eq!(next.app.sort_of(View::Songs), Some(Sort { key: Key::Duration, desc: true }));
    assert_eq!(next.app.sort_of(View::Search), Some(Sort { key: Key::Artist, desc: false }));
    assert_eq!(next.app.sort_of(View::Albums), None);
    next.press(&[ch('2')]);
    next.advance(300);
    let text = next.frame(100, 24);
    assert!(text[8].starts_with("│ Playlists (4) · by name ↓") && text[11].contains("Road Trip Two"), "{}", text[11]);
    assert_eq!(next.app.browse(Screen::Playlists).selected().unwrap().id, "p.road2");
    next.press(&[ch('l'), key(KeyCode::Enter)]);
    assert_eq!(played(&next), (Some(0), Some(vec![5, 4, 3, 2, 1, 0])));
    // `,,` clears the remembered choice too.
    next.press(&[ch(','), ch(',')]);
    assert_eq!(crate::tui::sort::load(&path).songs, None);
    assert_eq!(crate::tui::sort::load(&path).playlists, Some(Sort { key: Key::Name, desc: true }));
    // Unwritable places and a missing file are ignored.
    let mut app = App::new(next.now, Box::new(Inline::new(Box::new(NoApp))));
    app.use_sort_file(path.join("nope").join("uzu.json"));
    app.handle_key(ch(','), &mut NoApp, next.now);
    let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
}

#[test]
fn sort_popup_and_sorted_frames() {
    let mut rig = songs_pane("sort-frames");
    rig.press(&[ch(',')]);
    let text = rig.frame(100, 24);
    // A rounded box at the bottom right of the body, just above the bottom border.
    let top = text.iter().position(|l| l.contains("Sort songs")).unwrap();
    assert_eq!(top, 15, "{top}");
    assert!(text[top].contains('╭') && text[top].contains("╮│"), "{}", text[top]);
    assert_eq!(text[22].trim_end().chars().rev().nth(1), Some('╯'), "{}", text[22]);
    for (row, want) in [(top + 1, "o/O  original · O: newest first"), (top + 2, "t/T"), (top + 3, "a/A"), (top + 4, "l/L"), (top + 5, "d/D")] {
        assert!(text[row].contains(want), "{}", text[row]);
    }
    assert!(text[top + 6].contains("a/z up · A/Z down"), "{}", text[top + 6]);
    assert!(text.iter().all(|l| text_width(l) == 100));
    // Another view, another title and keys.
    rig.press(&[key(KeyCode::Esc), ch('h'), ch(',')]);
    let text = rig.frame(100, 24);
    assert!(text.iter().any(|l| l.contains("Sort playlists")) && text.iter().any(|l| l.contains("n/N")) && text.iter().any(|l| l.contains("c/C")));
    assert!(!text.iter().any(|l| l.contains("t/T")));
    rig.press(&[key(KeyCode::Esc), ch('4'), ch(',')]);
    rig.advance(300);
    let text = rig.frame(100, 24);
    assert!(text.iter().any(|l| l.contains("Sort albums")) && text.iter().any(|l| l.contains("y/Y")));
    // The popup is drawn over the table and is only a popup: it needs the key to go away.
    rig.press(&[key(KeyCode::Esc)]);
    assert!(!rig.frame(100, 24).iter().any(|l| l.contains("Sort ")));
    // Hints and help mention it.
    let text = rig.frame(120, 24);
    assert!(text[23].contains(", sort"), "{}", text[23]);
    rig.press(&[ch('?')]);
    let text = rig.frame(100, 30).join("\n");
    assert!(text.contains("sort the focused pane"), "{text}");
}

#[test]
fn frame_lines_are_not_dim_but_secondary_text_is() {
    let mut rig = songs_pane("frame-style");
    rig.press(&[ch(',')]);
    let text = rig.frame(100, 24);
    let buf = draw(&rig.app, rig.now, 100, 24);
    let dim = |x: u16, y: u16| buf[(x, y)].style().add_modifier.contains(Modifier::DIM);
    // Outer border, the player/status divider, the rule under the tabs, the pane divider, the popup border.
    assert!(!dim(0, 0) && !dim(0, 5) && !dim(99, 5) && !dim(1, 0));
    assert!(!dim(text[0].chars().position(|c| c == '┬').unwrap() as u16, 3));
    assert!(!dim(5, 9));
    assert!(!dim(text[11].chars().position(|c| c == '│').unwrap() as u16, 11));
    let popup = text[15].chars().position(|c| c == '╭').unwrap_or(0);
    assert!(text[15].contains("Sort songs") && popup > 0 && !dim(popup as u16, 15) && !dim(popup as u16, 22));
    // Secondary text stays dim: the album line, the help hint, the key hints.
    assert!(dim(3, 4) && dim(3, 6) && dim(5, 23));
}

#[test]
fn sorted_songs_frame_by_duration_descending() {
    let mut rig = songs_pane("sort-frame-d");
    rig.press(&[ch(','), ch('D')]);
    let text = rig.frame(100, 24);
    assert!(text[8].starts_with("│ Favourite Songs · by duration ↓"), "{}", text[8]);
    assert!(text[11].contains("Instant Crush") && text[11].contains("3:55"), "{}", text[11]);
    assert!(text[16].contains("Lose Yourself") && text[16].contains("3:20"), "{}", text[16]);
    assert!(song_rows(&text).len() == 12);
    // The selection (song 1) sits on the last row, inverted.
    let buf = draw(&rig.app, rig.now, 100, 24);
    assert!(buf[(60, 16)].style().add_modifier.contains(Modifier::REVERSED));
}

// -------------------------------------------------------------------------------------- cover

/// Runs jobs inline and counts the covers it was asked to encode.
struct CountCovers {
    covers: Arc<Mutex<usize>>,
    inner: Inline,
}

impl super::jobs::Jobs for CountCovers {
    fn submit(&mut self, job: super::jobs::Job) {
        if let super::jobs::Job::Cover { .. } = job {
            *self.covers.lock().unwrap() += 1;
        }
        self.inner.submit(job);
    }

    fn poll(&mut self) -> Vec<super::jobs::Done> {
        self.inner.poll()
    }
}

/// The fixture with the cover shown (halfblocks at 10×20 px cells: 12×6 cells) and `path` as
/// the status's cover. Returns the app and the count of covers encoded.
fn with_cover(now: Instant, caps: super::cover::Caps, path: Option<String>) -> (App, Arc<Mutex<usize>>) {
    let mut app = fixture(now);
    let covers = Arc::new(Mutex::new(0));
    let fixture_app = std::mem::replace(&mut app, App::new(now, Box::new(CountCovers { covers: covers.clone(), inner: Inline::new(Box::new(NoApp)) })));
    app.set_queue(fixture_app.queue.clone(), now);
    app.cover = super::cover::Cover::with_caps(caps);
    set_cover(&mut app, path, now);
    (app, covers)
}

fn set_cover(app: &mut App, path: Option<String>, now: Instant) {
    let mut st = fixture(now).status.unwrap();
    st.song = app.status.as_ref().and_then(|s| s.song.clone()).or(st.song);
    st.cover = path;
    app.set_status(st, now);
    app.sync_cover();
    app.drain(now);
}

/// The normal player's cover slot at 10×20 px cells.
const NORMAL: ratatui::layout::Size = ratatui::layout::Size::new(12, 6);

fn blocks() -> super::cover::Caps {
    super::cover::Caps::halfblocks((10, 20))
}

/// A cell of the halfblock picture: the image's colours (a space or a half block).
fn picture(buf: &Buffer, x: u16, y: u16) -> bool {
    matches!(buf[(x, y)].bg, Color::Rgb(..))
}

/// The 12×6 cover is drawn at columns 3..15, rows 1..=6.
fn cover_drawn(buf: &Buffer) -> bool {
    (1..=6).all(|y| (3..15).all(|x| picture(buf, x, y)))
}

/// Any picture cell in the player rows.
fn any_picture(buf: &Buffer) -> bool {
    (1..=6u16).any(|y| (0..buf.area.width).any(|x| picture(buf, x, y)))
}

/// Row text from column `x` on.
fn from(row: &str, x: usize) -> String {
    row.chars().skip(x).collect()
}

#[test]
fn cover_sits_left_of_the_song_lines() {
    let now = Instant::now();
    let (app, covers) = with_cover(now, blocks(), Some(super::cover::test_jpeg("frame")));
    let buf = draw(&app, now, 140, 20);
    let text = lines(&buf);
    for l in &text[..8] {
        println!("{l}");
    }
    assert_eq!(*covers.lock().unwrap(), 1);
    assert!(cover_drawn(&buf));
    assert!(!picture(&buf, 2, 3) && !picture(&buf, 15, 3) && !picture(&buf, 3, 7), "nothing outside the slot");
    assert!(from(&text[2], 17).starts_with("Instant Crush") && text[2].contains("│ Status:  ▶ Playing"), "{}", text[2]);
    assert!(from(&text[3], 17).starts_with("Daft Punk") && from(&text[6], 17).starts_with("uzumaki · ? help"));
    // Red on the left, blue on the right, as in the picture.
    let bg = |x: u16| buf[(x, 3)].bg;
    assert!(matches!(bg(3), Color::Rgb(r, _, b) if r > 150 && b < 100), "{:?}", bg(3));
    assert!(matches!(bg(14), Color::Rgb(r, _, b) if r < 100 && b > 150), "{:?}", bg(14));
    assert!(text.iter().all(|l| text_width(l) == 140));
}

#[test]
fn cover_slot_keeps_its_place_while_the_file_comes() {
    let now = Instant::now();
    let (mut app, covers) = with_cover(now, blocks(), None);
    let text = lines(&draw(&app, now, 140, 20));
    assert_eq!(*covers.lock().unwrap(), 0, "nothing to encode yet");
    assert!(text[2].starts_with(&format!("│  {}  Instant Crush", " ".repeat(12))), "{}", text[2]);
    assert_eq!(text[4].chars().position(|c| c == '♪'), Some(3 + 6), "{}", text[4]);
    // Nothing playing: the player as without a cover.
    let mut st = app.status.clone().unwrap();
    st.state = PlayState::Stopped;
    app.set_status(st, now);
    let text = lines(&draw(&app, now, 140, 20));
    assert!(!text.iter().any(|l| l.contains('♪')));
    assert!(text[2].starts_with("│  Instant Crush"), "{}", text[2]);
}

#[test]
fn cover_gives_way_on_narrow_and_short_terminals() {
    let now = Instant::now();
    let (app, _) = with_cover(now, blocks(), Some(super::cover::test_jpeg("narrow")));
    let shown = |w: u16, h: u16| any_picture(&draw(&app, now, w, h));
    // With the status column the song lines need 12 + 2 + 24 columns: from 77 wide.
    assert!(shown(77, 20) && !shown(76, 20));
    // Without it (under 70 wide) from 44.
    assert!(shown(69, 20) && shown(44, 20) && !shown(43, 20));
    // The two-row compact player has no room.
    assert!(shown(140, 18) && !shown(140, 17));
    let text = lines(&draw(&app, now, 76, 20));
    assert!(text[2].starts_with("│  Instant Crush"), "today's layout: {}", text[2]);
}

#[test]
fn help_box_over_the_cover_leaves_it_out() {
    let now = Instant::now();
    let (mut app, _) = with_cover(now, blocks(), Some(super::cover::test_jpeg("help")));
    app.help = true;
    // Wide: the box is in the middle, the cover stays.
    assert!(cover_drawn(&draw(&app, now, 140, 30)));
    // Narrow: the box reaches into the player, so the cover's first column is blank too.
    let buf = draw(&app, now, 60, 20);
    for y in 1..=6 {
        assert!(!picture(&buf, 3, y), "row {y}");
    }
    app.help = false;
    assert!(cover_drawn(&draw(&app, now, 60, 20)));
}

#[test]
fn i_hides_and_shows_the_cover_and_hiding_deletes_the_kitty_image() {
    let now = Instant::now();
    let kitty = super::cover::Caps { proto: ratatui_image::picker::ProtocolType::Kitty, font: (10, 20), tmux: false };
    let (mut app, covers) = with_cover(now, kitty, Some(super::cover::test_jpeg("kitty")));
    assert_eq!(*covers.lock().unwrap(), 1);
    assert!(app.cover.image("c", NORMAL).is_some());
    assert_eq!(app.cover.take_deletes(), "", "nothing replaced yet");
    app.handle_key(ch('i'), &mut NoApp, now);
    let gone = app.cover.take_deletes();
    assert!(gone.starts_with("\x1b_Ga=d,d=I,i=") && gone.ends_with(",q=2\x1b\\"), "{gone:?}");
    let text = lines(&draw(&app, now, 140, 20));
    assert!(text[2].starts_with("│  Instant Crush"), "today's layout: {}", text[2]);
    assert!(text[8].contains("Cover: off"), "{}", text[8]);
    app.handle_key(ch('i'), &mut NoApp, now);
    assert!(app.cover.image("c", NORMAL).is_some(), "encoded again");
    assert_eq!(*covers.lock().unwrap(), 2);
    let text = lines(&draw(&app, now, 140, 20));
    assert!(!text[2].starts_with("│  Instant Crush"), "{}", text[2]);
}

#[test]
fn each_cover_is_encoded_once() {
    let now = Instant::now();
    let small = super::cover::test_jpeg("small");
    let (mut app, covers) = with_cover(now, blocks(), Some(small.clone()));
    for _ in 0..5 {
        set_cover(&mut app, Some(small.clone()), now);
    }
    assert_eq!(*covers.lock().unwrap(), 1, "polls with the same file encode nothing");
    // The big size lands: encoded once more, the small one showing until then.
    let big = super::cover::test_jpeg("big");
    set_cover(&mut app, Some(big.clone()), now);
    set_cover(&mut app, Some(big), now);
    assert_eq!(*covers.lock().unwrap(), 2);
    // A file that can't be read is not tried again on every poll.
    set_cover(&mut app, Some("/nonexistent/uzu.jpg".into()), now);
    set_cover(&mut app, Some("/nonexistent/uzu.jpg".into()), now);
    assert_eq!(*covers.lock().unwrap(), 3);
    // Another song drops the old picture at once, even before its own cover is there.
    let mut st = app.status.clone().unwrap();
    st.song = Some(s("x", "Other", "Someone", "Else", 100.0));
    st.cover = None;
    app.set_status(st, now);
    app.sync_cover();
    assert!(app.cover.image("c", NORMAL).is_none() && app.cover.image("x", NORMAL).is_none());
}

#[test]
fn a_loaded_playlist_shows_its_count_in_the_list() {
    use super::browse::Browse;
    let now = Instant::now();
    let mut b = Browse::new(Kind::Playlist);
    let list = |id: &str, count: Option<u32>| uzumaki_ctl::CollectionInfo { kind: Kind::Playlist, id: id.into(), title: id.into(), subtitle: String::new(), count, favorites: false, editable: true, year: None, pinned: None };
    b.set_items(vec![list("p.1", None), list("p.2", Some(7))], now);
    assert_eq!(b.selected().map(|c| c.count), Some(None));
    b.got("p.1".into(), Ok((list("p.1", None), vec![s("a", "A", "", "", 1.0), s("b", "B", "", "", 1.0)])));
    assert_eq!(b.selected().map(|c| c.count), Some(Some(2)));
}

// ------------------------------------------------------------------- showcase and solo (F)

use super::showcase::Layout;

#[test]
fn f_cycles_the_views_and_esc_digits_and_q_leave_them() {
    let mut rig = Rig::new("views");
    rig.app.resize_to(120, 30);
    assert_eq!(rig.app.layout, Layout::Normal);
    rig.press(&[ch('F')]);
    assert_eq!(rig.app.layout, Layout::Showcase);
    assert!(rig.app.redraw, "a new view clears the terminal");
    assert_eq!(rig.app.message(rig.now), Some("Showcase view · F next"));
    rig.press(&[ch('F')]);
    assert_eq!(rig.app.layout, Layout::Solo);
    rig.press(&[ch('F')]);
    assert_eq!(rig.app.layout, Layout::Normal);
    // Esc leaves a view instead of quitting; on the normal screen it still quits.
    rig.press(&[ch('F'), key(KeyCode::Esc)]);
    assert_eq!(rig.app.layout, Layout::Normal);
    rig.press(&[ch('F'), ch('F'), key(KeyCode::Esc)]);
    assert_eq!(rig.app.layout, Layout::Normal);
    assert!(!rig.app.quit);
    // From another screen F goes to the queue; a screen key goes back to normal.
    rig.press(&[ch('2'), ch('F')]);
    assert_eq!((rig.app.screen, rig.app.layout), (Screen::Queue, Layout::Showcase));
    rig.press(&[ch('2')]);
    assert_eq!((rig.app.screen, rig.app.layout), (Screen::Playlists, Layout::Normal));
    // q quits from any view.
    rig.press(&[ch('1'), ch('F'), ch('F'), ch('q')]);
    assert!(rig.app.quit);
}

#[test]
fn the_showcase_list_is_up_next_and_still_works() {
    let mut rig = Rig::new("showcase-keys");
    rig.app.resize_to(120, 30);
    rig.press(&[ch('F')]);
    assert!(!rig.app.visible().contains(&0), "the playing song is on the left, not in the list");
    assert_eq!(rig.app.cursor, 1, "the cursor moved off the hidden playing row");
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Jump { index: 0 });
    rig.press(&[ch('j'), ch('d')]);
    assert!(matches!(rig.last(), Request::Remove { .. }));
    rig.press(&[ch('o')]);
    assert_eq!(rig.app.cursor, rig.app.visible()[0], "o goes to the top of Up Next");
    // A click on a row of the right column selects it; one on the left panel does nothing.
    let g = rig.app.showcase().unwrap();
    let at = |column: u16, row: u16| MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE };
    rig.app.handle_mouse(at(g.queue.x + 5, g.queue.y + 3), &mut rig.client, rig.now);
    assert_eq!(rig.app.cursor, rig.app.visible()[2]);
    let before = rig.app.cursor;
    rig.app.handle_mouse(at(4, g.queue.y + 1), &mut rig.client, rig.now);
    assert_eq!(rig.app.cursor, before);
    // Back on the normal screen the playing row is listed again.
    rig.press(&[key(KeyCode::Esc)]);
    assert!(rig.app.visible().contains(&0));
}

#[test]
fn solo_ignores_the_list_keys() {
    let mut rig = Rig::new("solo-keys");
    rig.app.resize_to(120, 30);
    rig.press(&[ch('F'), ch('F')]);
    let before = rig.log.lock().unwrap().len();
    rig.press(&[ch('j'), key(KeyCode::Enter), ch('d'), ch('J'), ch('/'), ch(',')]);
    assert_eq!(rig.log.lock().unwrap().len(), before, "nothing sent for keys of a list that isn't shown");
    assert!(!rig.app.filtering && rig.app.sorting.is_none());
    rig.press(&[ch('n')]);
    assert!(rig.sent(&Request::Next), "playback keys still work");
    let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 50, row: 15, modifiers: KeyModifiers::NONE };
    rig.app.handle_mouse(click, &mut rig.client, rig.now);
    assert!(!rig.sent(&Request::Jump { index: 0 }));
}

/// The fixture with a halfblock cover, in `layout`, laid out for `w`×`h`.
fn in_view(layout: Layout, w: u16, h: u16, now: Instant) -> (App, Arc<Mutex<usize>>) {
    let (mut app, covers) = with_cover(now, blocks(), Some(super::cover::test_jpeg(&format!("view-{w}x{h}"))));
    app.layout = layout;
    app.resize_to(w, h);
    app.sync_cover();
    app.drain(now);
    (app, covers)
}

/// Every cell of `r` is part of the picture.
fn picture_fills(buf: &Buffer, r: ratatui::layout::Rect) -> bool {
    (r.y..r.y + r.height).all(|y| (r.x..r.x + r.width).all(|x| picture(buf, x, y)))
}

#[test]
fn showcase_frame_big_cover_song_and_up_next() {
    let now = Instant::now();
    let (app, _) = in_view(Layout::Showcase, 120, 30, now);
    let g = app.showcase().unwrap();
    let buf = draw(&app, now, 120, 30);
    let text = lines(&buf);
    for l in &text {
        println!("{l}");
    }
    let c = g.cover.unwrap();
    assert_eq!((c.width, c.height), (36, 18), "30 rows leave 18 for a square cover at 10x20 px");
    assert!(picture_fills(&buf, c), "the cover fills its slot");
    assert!(!picture(&buf, c.x + c.width, c.y) && !picture(&buf, c.x, c.y + c.height));
    let d = g.divider as usize;
    assert!(text[0].starts_with("╭─ Now Playing ─") && text[0].contains("┬─ Up Next · 7 songs · 29 min ─") && text[0].ends_with('╮'), "{}", text[0]);
    assert!(text[29].starts_with('╰') && text[29].contains('┴') && text[29].contains("Space play/pause · F view") && text[29].ends_with('╯'), "{}", text[29]);
    let t = g.text.y as usize;
    assert!(from(&text[t], 3).starts_with("Instant Crush") && from(&text[t + 1], 3).starts_with("Daft Punk") && from(&text[t + 2], 3).starts_with("Random Access Memories"));
    assert!(from(&text[t + 4], 3).starts_with("━") && text[t + 5].contains("▶ Playing") && text[t + 5].contains("1:12 / 3:59"), "{}", text[t + 5]);
    assert!(text[t + 6].contains("vol 82% · repeat all · shuffle on"), "{}", text[t + 6]);
    // The title is coral (playing), the settings dim, the bar's track plain.
    assert_eq!(buf[(3, t as u16)].fg, CORAL);
    assert!(buf[(3, t as u16 + 6)].modifier.contains(Modifier::DIM));
    assert!(!buf[(c.x + c.width - 1, t as u16 + 4)].modifier.contains(Modifier::DIM));
    // Up Next: the header, then song 1; the playing song isn't listed, the cursor is on 1.
    assert!(from(&text[1], d).contains("Title") && from(&text[2], d).contains("Группа крови"), "{}", text[2]);
    assert!(!text[1..29].iter().any(|l| from(l, d).contains("Instant Crush") || from(l, d).contains('▶')));
    assert!(buf[(g.queue.x, 2)].modifier.contains(Modifier::REVERSED));
    assert!(text.iter().all(|l| text_width(l) == 120));
}

#[test]
fn showcase_on_small_terminals_shrinks_then_falls_back_to_normal() {
    let now = Instant::now();
    let (app, _) = in_view(Layout::Showcase, 70, 18, now);
    let g = app.showcase().expect("the smallest showcase");
    assert_eq!(g.cover.unwrap().height, 6);
    let text = lines(&draw(&app, now, 70, 18));
    for l in &text {
        println!("{l}");
    }
    assert!(text[0].contains("Now Playing") && text[0].contains("Up Next"));
    // Narrower or shorter: the normal screen, and its list again shows the playing song.
    for (w, h) in [(69, 30), (120, 17)] {
        let (app, _) = in_view(Layout::Showcase, w, h, now);
        assert!(app.showcase().is_none());
        assert!(app.visible().contains(&0));
        let text = lines(&draw(&app, now, w, h));
        assert!(text.iter().any(|l| l.contains("1 Queue")), "{w}x{h}");
    }
}

#[test]
fn solo_is_the_cover_and_song_centred_without_a_frame() {
    let now = Instant::now();
    let (app, _) = in_view(Layout::Solo, 100, 34, now);
    let buf = draw(&app, now, 100, 34);
    let text = lines(&buf);
    for l in &text {
        println!("{l}");
    }
    let g = super::showcase::solo(100, 34, app.cover.caps());
    let c = g.cover.unwrap();
    assert_eq!(c.height, 34 - 9);
    assert!(picture_fills(&buf, c));
    let all = text.join("\n");
    for frame in ['╭', '╰', '│', '┤', '┬'] {
        assert!(!all.contains(frame), "no frame: {frame}");
    }
    assert!(!all.contains("help") && !all.contains("Up Next") && !all.contains("Queue"));
    let t = g.top as usize;
    let centre = |l: &str| {
        let lead = l.len() - l.trim_start().len();
        let trail = l.chars().count() - l.trim_end().chars().count();
        lead.abs_diff(trail) <= 1
    };
    assert!(text[t].trim() == "Instant Crush" && centre(&text[t]), "{:?}", text[t]);
    assert!(text[t + 1].trim() == "Daft Punk" && text[t + 2].trim() == "Random Access Memories");
    assert!(text[t + 4].trim_start().starts_with('━'));
    let time = text[t + 5].trim();
    assert!(time.starts_with("1:12") && time.ends_with("3:59") && time.contains('▶'), "{time:?}");
    assert_eq!(buf[(g.bar.x, g.bar.y + 1)].symbol(), "1");
    assert_eq!(buf[(g.bar.x + g.bar.width - 1, g.bar.y + 1)].symbol(), "9");
}

#[test]
fn solo_on_small_terminals_shrinks_then_drops_the_cover() {
    let now = Instant::now();
    let (app, _) = in_view(Layout::Solo, 40, 16, now);
    let buf = draw(&app, now, 40, 16);
    let g = super::showcase::solo(40, 16, app.cover.caps());
    assert!(g.cover.is_some_and(|c| c.height == 7 && picture_fills(&buf, c)));
    let (app, _) = in_view(Layout::Solo, 60, 10, now);
    let text = lines(&draw(&app, now, 60, 10));
    for l in &text {
        println!("{l}");
    }
    assert!(!(0..10).any(|y| (0..60).any(|x| picture(&draw(&app, now, 60, 10), x, y))), "too short for a cover");
    assert!(text.iter().any(|l| l.trim() == "Instant Crush"));
}

#[test]
fn views_never_panic_on_tiny_terminals() {
    let now = Instant::now();
    for layout in [Layout::Showcase, Layout::Solo] {
        for (w, h) in [(1, 1), (2, 2), (3, 1), (10, 3), (20, 5), (69, 17), (70, 18), (71, 19), (30, 40), (200, 6)] {
            let (mut app, _) = in_view(layout, w, h, now);
            draw(&app, now, w, h);
            app.help = true;
            draw(&app, now, w, h);
            app.cover.toggle();
            app.resize_to(w, h);
            draw(&app, now, w, h);
        }
    }
}

#[test]
fn nothing_playing_and_no_cover_look_deliberate_in_both_views() {
    let now = Instant::now();
    for layout in [Layout::Showcase, Layout::Solo] {
        // Stopped: an empty frame where the cover goes, "Nothing playing", an empty bar.
        let (mut app, _) = in_view(layout, 100, 30, now);
        let mut st = app.status.clone().unwrap();
        st.state = PlayState::Stopped;
        st.song = None;
        app.set_status(st, now);
        app.sync_cover();
        let text = lines(&draw(&app, now, 100, 30));
        let all = text.join("\n");
        println!("{all}");
        assert!(all.contains("Nothing playing") && all.contains('♪') && all.contains('╭') && all.contains("--:--"), "{layout:?}");
        assert!(all.contains("■"), "{layout:?}");
        assert!(!all.contains('━'), "an empty bar: {layout:?}");
        // A song without artwork (or still coming): the same frame, with the song.
        let (mut app, _) = with_cover(now, blocks(), None);
        app.layout = layout;
        app.resize_to(100, 30);
        let all = lines(&draw(&app, now, 100, 30)).join("\n");
        assert!(all.contains('♪') && all.contains("Instant Crush"), "{layout:?}");
        // Before the first status.
        let mut app = App::new(now, Box::new(Inline::new(Box::new(NoApp))));
        app.layout = layout;
        app.resize_to(100, 30);
        let all = lines(&draw(&app, now, 100, 30)).join("\n");
        assert!(all.contains("Connecting…"), "{layout:?}");
    }
}

#[test]
fn i_in_the_views_gives_a_text_only_layout() {
    let now = Instant::now();
    let (mut app, _) = in_view(Layout::Solo, 100, 30, now);
    app.handle_key(ch('i'), &mut NoApp, now);
    assert!(app.redraw);
    let buf = draw(&app, now, 100, 30);
    let text = lines(&buf);
    assert!(!(0..30).any(|y| (0..100).any(|x| picture(&buf, x, y))));
    assert!(!text.join("").contains('♪'), "no empty cover frame either");
    let g = super::showcase::solo(100, 30, None);
    assert!(text[g.top as usize].trim() == "Instant Crush" && g.top >= 10, "centred vertically: row {}", g.top);
    let (mut app, _) = in_view(Layout::Showcase, 100, 30, now);
    app.handle_key(ch('i'), &mut NoApp, now);
    let g = app.showcase().unwrap();
    assert!(g.cover.is_none());
    let text = lines(&draw(&app, now, 100, 30));
    assert!(from(&text[g.text.y as usize], 3).starts_with("Instant Crush"));
}

#[test]
fn cycling_views_encodes_each_size_once_and_a_new_song_deletes_them_all() {
    let now = Instant::now();
    let kitty = super::cover::Caps { proto: ratatui_image::picker::ProtocolType::Kitty, font: (10, 20), tmux: false };
    let (mut app, covers) = with_cover(now, kitty, Some(super::cover::test_jpeg("cycle")));
    app.resize_to(120, 30);
    assert_eq!(*covers.lock().unwrap(), 1, "the normal size");
    for _ in 0..3 {
        app.handle_key(ch('F'), &mut NoApp, now);
        app.tick(&mut NoApp, now);
    }
    assert_eq!(*covers.lock().unwrap(), 3, "showcase and solo sizes, once each");
    assert_eq!(app.cover.kept(), 3);
    for _ in 0..6 {
        app.handle_key(ch('F'), &mut NoApp, now);
        app.tick(&mut NoApp, now);
    }
    assert_eq!(*covers.lock().unwrap(), 3, "cycling again encodes nothing");
    assert_eq!(app.cover.take_deletes(), "", "nothing replaced");
    // A resize gives the big views new sizes; at most three are kept, the dropped one deleted.
    app.handle_key(ch('F'), &mut NoApp, now);
    app.resize_to(140, 40);
    app.sync_cover();
    app.drain(now);
    assert_eq!(*covers.lock().unwrap(), 4);
    assert_eq!(app.cover.kept(), 3);
    assert_eq!(app.cover.take_deletes().matches("a=d").count(), 1);
    // Another song: every kept picture goes.
    let mut st = app.status.clone().unwrap();
    st.song = Some(s("x", "Other", "Someone", "Else", 100.0));
    st.cover = None;
    app.set_status(st, now);
    app.sync_cover();
    assert_eq!(app.cover.kept(), 0);
    assert_eq!(app.cover.take_deletes().matches("a=d").count(), 3);
}

#[test]
fn while_a_big_size_encodes_the_smaller_one_shows() {
    let now = Instant::now();
    let (mut app, _) = with_cover(now, blocks(), Some(super::cover::test_jpeg("interim")));
    app.resize_to(120, 30);
    let big = ratatui::layout::Size::new(36, 18);
    assert!(app.cover.image("c", big).is_some_and(|p| p.size() == NORMAL), "the normal one, centred, until the big one lands");
    app.layout = Layout::Showcase;
    app.sync_cover();
    app.drain(now);
    assert!(app.cover.image("c", big).is_some_and(|p| p.size() == big));
    assert!(app.cover.image("c", NORMAL).is_some_and(|p| p.size() == NORMAL));
}

#[test]
fn help_lists_f() {
    let text = lines(&draw(&{
        let mut a = fixture(Instant::now());
        a.help = true;
        a
    }, Instant::now(), 100, 30))
    .join("\n");
    assert!(text.contains("view: normal, showcase, solo"));
    assert!(crate::cli::HELP.contains("F cycles"));
}

// ------------------------------------------------------------------------------- lyrics screen

mod lyrics_screen {
    use super::*;
    use crate::tests::fake_lyrics;
    use uzumaki_ctl::{LyricLine, LyricsStatus, Reply};

    /// What the scripted app says; shared with the test, which changes it between ticks.
    struct Script {
        status: Status,
        lyrics: Reply,
        /// Answer `Lyrics` like an app from before it existed.
        old_app: bool,
        asks: usize,
    }

    struct Wire(Arc<Mutex<Script>>);

    impl crate::run::Backend for Wire {
        fn request(&mut self, req: &Request) -> std::io::Result<Reply> {
            let mut s = self.0.lock().unwrap();
            Ok(match req {
                Request::Status => Reply::Status(s.status.clone()),
                Request::Lyrics if s.old_app => Reply::Error { message: "bad request: unknown variant `lyrics`".into() },
                Request::Lyrics => {
                    s.asks += 1;
                    s.lyrics.clone()
                }
                Request::Queue => Reply::Queue(Queue { current: s.status.song.clone(), ..Default::default() }),
                _ => Reply::Error { message: "not in the script".into() },
            })
        }
    }

    struct Lab {
        script: Arc<Mutex<Script>>,
        be: Wire,
        app: App,
        now: Instant,
    }

    impl Lab {
        /// "Instant Crush" (id `c`) playing at `position`, lyrics fetched on the first tick.
        fn new(position: f64) -> Lab {
            let mut lab = Lab::bare(position);
            lab.press(&[ch('5')]);
            lab
        }

        /// The same, on the queue screen (nothing asked yet).
        fn bare(position: f64) -> Lab {
            let now = Instant::now();
            let status = Status { state: PlayState::Playing, song: Some(s("c", "Instant Crush", "Daft Punk", "Random Access Memories", 239.0)), position, duration: 239.0, volume: 0.8, connected: true, signed_in: true, ..Default::default() };
            let script = Arc::new(Mutex::new(Script { status, lyrics: fake_lyrics("c"), old_app: false, asks: 0 }));
            let mut app = App::new(now, Box::new(Inline::new(Box::new(NoApp))));
            app.resize_to(100, 24);
            Lab { be: Wire(script.clone()), script, app, now }
        }

        /// Solo (`F F`) in `w`×`h` with a halfblock cover, after the first status.
        fn solo(position: f64, w: u16, h: u16) -> Lab {
            let mut lab = Lab::bare(position);
            lab.script.lock().unwrap().status.cover = Some(crate::tui::cover::test_jpeg(&format!("sololy-{w}x{h}")));
            lab.app.cover = crate::tui::cover::Cover::with_caps(blocks());
            lab.app.resize_to(w, h);
            lab.press(&[ch('F'), ch('F')]);
            assert!(lab.app.solo());
            lab
        }

        fn frame_at(&mut self, w: u16, h: u16) -> (Vec<String>, Buffer) {
            let buf = draw(&self.app, self.now, w, h);
            let text = lines(&buf);
            for l in &text {
                println!("{l}");
            }
            (text, buf)
        }

        fn press(&mut self, keys: &[KeyEvent]) {
            for k in keys {
                self.app.handle_key(*k, &mut self.be, self.now);
            }
            self.app.tick(&mut self.be, self.now);
        }

        /// Moves the fake clock on and ticks.
        fn advance(&mut self, ms: u64) -> bool {
            self.now += Duration::from_millis(ms);
            self.app.tick(&mut self.be, self.now)
        }

        fn asks(&self) -> usize {
            self.script.lock().unwrap().asks
        }

        fn frame(&mut self) -> (Vec<String>, Buffer) {
            let buf = draw(&self.app, self.now, 100, 24);
            let text = lines(&buf);
            for l in &text {
                println!("{l}");
            }
            (text, buf)
        }

        /// The row of the screen holding `needle`.
        fn row_of(text: &[String], needle: &str) -> usize {
            text.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row with {needle:?}"))
        }
    }

    fn column_of(text: &str, needle: &str) -> u16 {
        text[..text.find(needle).unwrap()].chars().count() as u16
    }

    #[test]
    fn five_opens_the_screen_asks_once_and_esc_goes_back() {
        let mut lab = Lab::new(12.5);
        assert_eq!(lab.app.screen, Screen::Lyrics);
        assert_eq!(lab.asks(), 1);
        lab.advance(500);
        lab.advance(500);
        assert_eq!(lab.asks(), 1, "the lyrics are asked for once per song, not polled");
        assert_eq!(lab.app.lyrics.status, LyricsStatus::Synced);
        lab.press(&[key(KeyCode::Esc)]);
        assert_eq!(lab.app.screen, Screen::Queue);
    }

    #[test]
    fn the_status_never_carries_lyrics() {
        // Status is polled twice a second: the lyrics (word timing is ~20 KB) have their own request.
        let text = serde_json::to_string(&Reply::Status(Status::default())).unwrap();
        assert!(!text.contains("lyric"), "{text}");
    }

    #[test]
    fn the_current_line_is_in_the_middle_and_sung_words_are_coral() {
        // 12.5 s: line 0 ("one two three four"), words "one", "two", "three" have started.
        let mut lab = Lab::new(12.5);
        let (text, buf) = lab.frame();
        let current = Lab::row_of(&text, "one two three four");
        let (top, bottom) = (10usize, 22usize); // the body: rows 10..=22 of a 24-row screen
        assert_eq!(current, top + (bottom - top) / 2, "centred vertically");
        let x = column_of(&text[current], "one");
        let width = "one two three four".chars().count() as u16;
        assert_eq!(x, 2 + (95 - width) / 2, "centred horizontally in columns 2..97: {}", text[current]);
        let style = |col: u16| buf[(col, current as u16)].style();
        for (k, ch) in "one two three four".chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let st = style(x + k as u16);
            let sung = k < "one two three".len();
            assert_eq!(st.fg == Some(CORAL), sung, "{ch:?} at {k}");
            assert!(st.add_modifier.contains(Modifier::BOLD), "the current line is bold");
        }
        // The next line is plain, and the second voice's line is right-aligned.
        let next = Lab::row_of(&text, "second line here");
        assert_eq!(next, current + 1);
        assert_ne!(style_of(&buf, text.as_slice(), "second line here").fg, Some(CORAL));
        let answer = Lab::row_of(&text, "answer");
        assert_eq!(column_of(&text[answer], "answer") + 6, 2 + 95, "right-aligned: {}", text[answer]);
        // The context line says what it shows.
        assert!(text[8].starts_with("│ Lyrics: Instant Crush — Daft Punk · word by word"), "{}", text[8]);
    }

    fn style_of(buf: &Buffer, text: &[String], needle: &str) -> ratatui::style::Style {
        let row = Lab::row_of(text, needle);
        buf[(column_of(&text[row], needle), row as u16)].style()
    }

    #[test]
    fn lines_before_the_current_one_are_dim() {
        let mut lab = Lab::new(15.0); // line 1 is being sung
        let (text, buf) = lab.frame();
        assert!(style_of(&buf, &text, "one two three four").add_modifier.contains(Modifier::DIM));
        let now = style_of(&buf, &text, "second line here");
        assert_eq!(now.fg, Some(CORAL), "a line without word timing is coral as a whole");
        assert!(!now.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn w_switches_between_words_and_lines() {
        let mut lab = Lab::new(11.5); // "one" has started, "two" has not... 11.5 > 11.0: "two" has too
        let (text, buf) = lab.frame();
        let x = column_of(&text[Lab::row_of(&text, "one two")], "one");
        let row = Lab::row_of(&text, "one two") as u16;
        assert_ne!(buf[(x + 8, row)].style().fg, Some(CORAL), "words: 'three' hasn't started");
        lab.press(&[ch('w')]);
        assert!(lab.app.message(lab.now).unwrap().contains("line by line"));
        let (text, buf) = lab.frame();
        let row = Lab::row_of(&text, "one two") as u16;
        assert_eq!(buf[(x + 8, row)].style().fg, Some(CORAL), "lines: the whole line is coral");
        lab.advance(2100); // the message goes; the context line says the mode
        let (text, _) = lab.frame();
        assert!(text[8].contains("Instant Crush — Daft Punk · line by line"), "{}", text[8]);
        lab.press(&[ch('w')]);
        assert!(lab.app.message(lab.now).unwrap().contains("word by word"));
    }

    #[test]
    fn scrolling_by_hand_stops_following_and_g_follows_again() {
        let mut lab = Lab::new(12.5);
        let followed = lab.frame().0;
        let at = Lab::row_of(&followed, "one two three four");
        lab.press(&[ch('j'), ch('j')]);
        let scrolled = lab.frame().0;
        assert_eq!(Lab::row_of(&scrolled, "one two three four"), at - 2, "two rows up");
        assert!(scrolled[8].contains("scrolled, g follows"), "{}", scrolled[8]);
        // The song moves on into the next line; the view stays where the hand put it.
        lab.script.lock().unwrap().status.position = 15.0;
        lab.advance(500);
        assert_eq!(Lab::row_of(&lab.frame().0, "one two three four"), at - 2);
        lab.press(&[ch('k')]);
        assert_eq!(Lab::row_of(&lab.frame().0, "one two three four"), at - 1);
        lab.press(&[ch('g')]);
        let back = lab.frame().0;
        assert_eq!(Lab::row_of(&back, "second line here"), at, "the current line (now the second) is in the middle again");
        // It can't be scrolled out of the text.
        lab.press(&[ch('G'), ch('j'), ch('j')]);
        let end = lab.frame().0;
        assert!(end.iter().any(|l| l.contains("answer")), "the last line stays in view");
        lab.press(&[key(KeyCode::Home)]);
        assert_eq!(lab.app.lyrics.manual, None);
    }

    #[test]
    fn the_wheel_scrolls_too() {
        let mut lab = Lab::new(12.5);
        let at = Lab::row_of(&lab.frame().0, "one two three four");
        lab.app.handle_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: 5, row: 12, modifiers: KeyModifiers::NONE }, &mut lab.be, lab.now);
        // Three rows down, but the view stops with the last line in the middle (only 3 lines here).
        assert_eq!(Lab::row_of(&lab.frame().0, "one two three four"), at - 2);
    }

    #[test]
    fn the_clock_never_steps_back_except_on_a_seek() {
        let mut lab = Lab::new(14.0);
        lab.advance(100); // the status at 14.0 + 0.1 shown
        let shown = lab.app.lyrics.clock(lab.app.position(lab.now));
        assert!((shown - 14.1).abs() < 1e-9, "{shown}");
        // A late status says 13.9 (the interpolation said 14.1 + 0.4): a jitter, not a seek.
        lab.script.lock().unwrap().status.position = 13.9;
        lab.now += Duration::from_millis(400);
        lab.app.tick(&mut lab.be, lab.now);
        let t = lab.app.lyrics.clock(lab.app.position(lab.now));
        assert!(t >= shown - 1e-9, "stepped back to {t}");
        assert_eq!(lab.app.lyrics.cursor(t).0, Some(1), "still on the second line");
        // A seek back 10 s is followed.
        lab.script.lock().unwrap().status.position = 4.0;
        lab.now += Duration::from_millis(500);
        lab.app.tick(&mut lab.be, lab.now);
        let t = lab.app.lyrics.clock(lab.app.position(lab.now));
        assert!(t < 5.0, "{t}");
        assert_eq!(lab.app.lyrics.cursor(t).0, None, "before the first line");
        let (text, _) = lab.frame();
        assert!(text.iter().any(|l| l.contains("one two three four")) && !text[10..].iter().any(|l| l.contains("Loading")), "the first line shows below the middle");
    }

    #[test]
    fn redraws_come_at_word_and_line_starts_not_on_a_timer() {
        let mut lab = Lab::new(10.0);
        let at = |lab: &Lab| lab.app.lyrics.clock(lab.app.position(lab.now));
        lab.app.lyrics.reopened();
        assert!(lab.app.lyrics.changed(at(&lab), lab.now), "the first frame");
        // Nothing starts for the next 900 ms: no redraw is due, however often it is asked.
        for _ in 0..8 {
            lab.now += Duration::from_millis(100);
            assert!(!lab.app.lyrics.changed(at(&lab), lab.now), "at {}", at(&lab));
        }
        // The loop sleeps until the word at 11.0 (about 100 ms away), not a fixed 100 ms.
        lab.now += Duration::from_millis(150); // 10.95
        let wait = lab.app.wait(lab.now);
        assert!(wait >= Duration::from_millis(40) && wait <= Duration::from_millis(60), "{wait:?}");
        lab.now += wait;
        assert!(at(&lab) >= 10.99, "{}", at(&lab));
        lab.now += Duration::from_millis(5);
        assert!(lab.app.lyrics.changed(at(&lab), lab.now), "'two' starts");
        // Right after, the same frame; and a word 20 ms later waits for the 100 ms gate.
        assert!(!lab.app.lyrics.changed(at(&lab), lab.now));
    }

    #[test]
    fn redraws_are_at_most_every_100_ms() {
        let mut l = super::super::lyrics::Lyrics::default();
        let word = |start: f64, text: &str| uzumaki_ctl::LyricWord { start, end: start + 0.02, text: text.into() };
        l.set("x".into(), LyricsStatus::Synced, vec![LyricLine { start: 0.0, end: 1.0, text: "a b c d".into(), words: vec![word(0.0, "a "), word(0.02, "b "), word(0.04, "c "), word(0.3, "d")], ..Default::default() }]);
        let start = Instant::now();
        assert!(l.changed(0.0, start));
        // 'b' and 'c' start within 100 ms: they wait for the gate, then show together.
        assert!(!l.changed(0.05, start + Duration::from_millis(50)));
        assert_eq!(l.wake_in(0.05, start + Duration::from_millis(50)), Some(Duration::from_millis(50)));
        assert!(l.changed(0.1, start + Duration::from_millis(100)));
        assert!(!l.changed(0.2, start + Duration::from_millis(200)), "nothing started since");
        assert_eq!(l.until_next(0.2).map(|d| (d * 1000.0).round()), Some(100.0));
    }

    #[test]
    fn a_song_change_asks_again_and_shows_loading_meanwhile() {
        let mut lab = Lab::new(12.5);
        assert_eq!(lab.asks(), 1);
        {
            let mut s = lab.script.lock().unwrap();
            s.status.song = Some(s_song("d"));
            s.status.position = 1.0;
            s.lyrics = Reply::Lyrics { song_id: "d".into(), status: LyricsStatus::Loading, lines: vec![] };
        }
        lab.advance(500); // the status says "d"; the screen asks for its lyrics
        assert_eq!(lab.asks(), 2);
        assert_eq!(lab.app.lyrics.status, LyricsStatus::Loading);
        assert!(lab.frame().0.iter().any(|l| l.contains("Loading lyrics…")));
        // Still loading: asked again after a second, not before.
        lab.advance(500);
        assert_eq!(lab.asks(), 2);
        lab.advance(600);
        assert_eq!(lab.asks(), 3);
        lab.script.lock().unwrap().lyrics = fake_lyrics("d");
        lab.advance(1100);
        assert_eq!(lab.app.lyrics.status, LyricsStatus::Synced);
        lab.advance(1100);
        assert_eq!(lab.asks(), 4, "settled: no more asks");
        lab.advance(3000);
        assert_eq!(lab.asks(), 4);
    }

    fn s_song(id: &str) -> SongInfo {
        s(id, "Midnight City", "M83", "Hurry Up, We're Dreaming", 244.0)
    }

    #[test]
    fn empty_states() {
        let mut lab = Lab::new(5.0);
        let say = |lab: &mut Lab, text: &str| {
            let (frame, _) = lab.frame();
            assert!(frame.iter().any(|l| l.contains(text)), "wanted {text:?}");
        };
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "c".into(), status: LyricsStatus::None, lines: vec![] };
        lab.script.lock().unwrap().status.song = Some(s_song("e"));
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "e".into(), status: LyricsStatus::None, lines: vec![] };
        lab.advance(500);
        say(&mut lab, "No lyrics");
        lab.script.lock().unwrap().status.song = Some(s_song("f"));
        lab.script.lock().unwrap().status.signed_in = false;
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "f".into(), status: LyricsStatus::None, lines: vec![] };
        lab.advance(500);
        say(&mut lab, "Sign in for lyrics");
        lab.script.lock().unwrap().status.state = PlayState::Stopped;
        lab.advance(500);
        say(&mut lab, "Nothing playing");
    }

    #[test]
    fn an_old_app_gets_a_hint_and_no_more_asks() {
        let mut lab = Lab::new(5.0);
        lab.script.lock().unwrap().old_app = true;
        lab.script.lock().unwrap().status.song = Some(s_song("g"));
        for _ in 0..5 {
            lab.advance(600);
        }
        assert!(lab.app.lyrics.unsupported);
        assert!(lab.frame().0.iter().any(|l| l.contains("Update Uzumaki for lyrics")));
        // It asked once; the other screens are as before.
        lab.press(&[ch('1')]);
        assert_eq!(lab.app.screen, Screen::Queue);
    }

    #[test]
    fn unsynced_text_is_plain_and_scrolls() {
        let mut lab = Lab::new(5.0);
        let many: Vec<LyricLine> = (0..40).map(|i| LyricLine { text: format!("verse line {i}"), ..Default::default() }).collect();
        lab.script.lock().unwrap().status.song = Some(s_song("h"));
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "h".into(), status: LyricsStatus::Plain, lines: many };
        lab.advance(500);
        let (text, buf) = lab.frame();
        assert!(text[10].contains("verse line 0"), "starts at the top: {}", text[10]);
        assert!(text[8].contains("not time-synced"), "{}", text[8]);
        let st = style_of(&buf, &text, "verse line 0");
        assert!(st.fg != Some(CORAL) && !st.add_modifier.contains(Modifier::BOLD) && !st.add_modifier.contains(Modifier::DIM), "no highlight");
        // Time passing changes nothing.
        lab.advance(5000);
        assert!(lab.frame().0[10].contains("verse line 0"));
        lab.press(&[ch('j'), ch('j'), ch('j')]);
        assert!(lab.frame().0[10].contains("verse line 3"));
        lab.press(&[ch('G')]);
        let end = lab.frame().0;
        assert!(end[22].contains("verse line 39"), "{}", end[22]);
        lab.press(&[ch('g')]);
        assert!(lab.frame().0[10].contains("verse line 0"));
        lab.press(&[key(KeyCode::PageDown)]);
        assert!(lab.frame().0[10].contains("verse line 12"));
    }

    #[test]
    fn long_lines_wrap_and_stay_inside_the_box() {
        let mut lab = Lab::new(5.0);
        let long = "this is a very long line of a song that will not fit on a narrow terminal at all ever".to_string();
        lab.script.lock().unwrap().status.song = Some(s_song("i"));
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "i".into(), status: LyricsStatus::Synced, lines: vec![LyricLine { start: 1.0, end: 9.0, text: long.clone(), ..Default::default() }, LyricLine { start: 9.0, end: 12.0, text: "short".into(), ..Default::default() }] };
        lab.app.resize_to(40, 24);
        lab.advance(500);
        let buf = draw(&lab.app, lab.now, 40, 24);
        let text = lines(&buf);
        for l in &text {
            println!("{l}");
        }
        let shown: Vec<&String> = text[10..23].iter().filter(|l| l.contains("this") || l.contains("line") || l.contains("narrow") || l.contains("ever")).collect();
        assert!(shown.len() >= 3, "wrapped over rows: {shown:?}");
        assert!(text.iter().all(|l| text_width(l) <= 40), "no row is wider than the screen");
        let joined: String = text[10..23].iter().map(|l| l.trim_matches(|c| c == '│' || c == ' ').to_string()).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("this is a very long line"), "{joined}");
    }

    #[test]
    fn help_and_hints_list_the_keys() {
        let mut lab = Lab::new(5.0);
        let (text, _) = lab.frame();
        assert!(text[23].contains("w words/lines") && text[23].contains("j/k scroll"), "{}", text[23]);
        lab.press(&[ch('?')]);
        let (text, _) = lab.frame();
        let help = text.join("\n");
        assert!(help.contains("Keys: Lyrics") && help.contains("word by word") && help.contains("scroll by hand") && help.contains("follow the song again"), "{help}");
        assert!(help.contains("1 2 3 4 5"), "{help}");
    }

    #[test]
    fn a_tiny_terminal_never_panics() {
        let mut lab = Lab::new(12.5);
        for (w, h) in [(1, 1), (5, 3), (20, 6), (30, 12), (200, 60)] {
            lab.app.resize_to(w, h);
            let _ = draw(&lab.app, lab.now, w, h);
            lab.press(&[ch('j'), ch('G'), ch('g')]);
        }
    }

    // ------------------------------------------------------------------- lyrics in solo (`l`)

    fn rect_has_picture(buf: &Buffer, r: ratatui::layout::Rect) -> bool {
        picture_fills(buf, r)
    }

    #[test]
    fn solo_without_l_asks_for_nothing_and_looks_as_before() {
        let mut lab = Lab::solo(12.5, 120, 34);
        lab.advance(500);
        assert_eq!(lab.asks(), 0);
        let (_, buf) = lab.frame_at(120, 34);
        let g = super::super::showcase::solo(120, 34, lab.app.cover.caps());
        assert_eq!(lab.app.solo_geometry(), Some(g));
        assert!(rect_has_picture(&buf, g.cover.unwrap()));
    }

    #[test]
    fn l_puts_the_lyrics_right_of_the_cover_in_a_wide_terminal() {
        let mut lab = Lab::solo(12.5, 120, 34);
        let before = lab.app.solo_geometry().unwrap().cover.unwrap();
        lab.press(&[ch('l')]);
        assert_eq!(lab.asks(), 1);
        assert!(lab.app.redraw, "the screen is cleared for the new layout");
        let (text, buf) = lab.frame_at(120, 34);
        let g = lab.app.solo_geometry().unwrap();
        let ly = g.lyrics.expect("lyrics rect");
        let c = g.cover.unwrap();
        assert!(c.x + c.width <= ly.x && c.width < before.width, "cover {c:?} left of lyrics {ly:?}");
        assert!(rect_has_picture(&buf, c), "the cover is re-encoded for its new slot");
        // Song lines on the left, the current line centred in the right-hand rectangle.
        let title = Lab::row_of(&text, "Instant Crush");
        assert!(column_of(&text[title], "Instant Crush") < ly.x);
        let current = Lab::row_of(&text, "one two three four");
        assert_eq!(current as u16, ly.y + ly.height / 2, "{}", text[current]);
        let x = column_of(&text[current], "one");
        let width = "one two three four".chars().count() as u16;
        assert_eq!(x, ly.x + (ly.width - width) / 2);
        // Sung words are coral: the same renderer as screen 5.
        for (k, ch) in "one two three".chars().enumerate().filter(|(_, c)| *c != ' ') {
            let st = buf[(x + k as u16, current as u16)].style();
            assert_eq!(st.fg, Some(CORAL), "{ch:?}");
        }
        assert_ne!(buf[(x + "one two three ".len() as u16, current as u16)].style().fg, Some(CORAL), "'four' not yet");
        assert!(text.iter().all(|l| !l.contains('│')), "still no frame");
    }

    #[test]
    fn l_puts_the_lyrics_under_the_cover_in_a_narrow_terminal() {
        let mut lab = Lab::solo(12.5, 60, 40);
        let plain = lab.app.solo_geometry().unwrap().cover.unwrap();
        lab.press(&[ch('l')]);
        let (text, buf) = lab.frame_at(60, 40);
        let g = lab.app.solo_geometry().unwrap();
        let ly = g.lyrics.unwrap();
        let c = g.cover.unwrap();
        assert!(c.height < plain.height && c.height <= 40 / 3, "{c:?} vs {plain:?}");
        assert!(ly.y > g.bar.y + 1 && ly.width == 56, "{ly:?}");
        assert!(rect_has_picture(&buf, c));
        let current = Lab::row_of(&text, "one two three four");
        assert!(current as u16 >= ly.y && (current as u16) < ly.y + ly.height);
        assert_eq!(current as u16, ly.y + ly.height / 2);
        assert!(Lab::row_of(&text, "Instant Crush") < current);
        assert!(text[g.bar.y as usize + 1].contains("3:59"), "the time stays: {}", text[g.bar.y as usize + 1]);
    }

    #[test]
    fn l_again_takes_them_away_and_the_choice_lasts_the_session() {
        let mut lab = Lab::solo(12.5, 120, 34);
        let plain = lab.app.solo_geometry().unwrap();
        lab.press(&[ch('l')]);
        assert!(lab.app.solo_geometry().unwrap().lyrics.is_some());
        lab.app.redraw = false;
        lab.press(&[ch('l')]);
        assert_eq!(lab.app.solo_geometry(), Some(plain));
        assert!(lab.app.redraw, "the cover grows back: clear the screen");
        assert!(!lab.frame_at(120, 34).0.iter().any(|l| l.contains("one two")));
        // On again, out of solo and back in: still on.
        lab.press(&[ch('l')]);
        lab.press(&[key(KeyCode::Esc)]);
        assert_eq!(lab.app.layout, Layout::Normal);
        lab.press(&[ch('F'), ch('F')]);
        assert!(lab.app.solo_lyrics && lab.app.solo_lyrics_shown());
        assert!(lab.frame_at(120, 34).0.iter().any(|l| l.contains("one two three four")));
        assert_eq!(lab.asks(), 1, "the song's lyrics are kept, not asked again");
    }

    #[test]
    fn l_does_nothing_outside_solo() {
        let mut lab = Lab::bare(12.5);
        lab.press(&[ch('l')]);
        assert!(!lab.app.solo_lyrics);
        lab.press(&[ch('F'), ch('l')]);
        assert!(!lab.app.solo_lyrics, "not in showcase either");
    }

    #[test]
    fn w_switches_words_and_lines_in_solo() {
        let mut lab = Lab::solo(12.5, 120, 34);
        lab.press(&[ch('w')]);
        assert!(lab.app.lyrics.words(), "w means nothing until the lyrics are on");
        lab.press(&[ch('l'), ch('w')]);
        assert!(!lab.app.lyrics.words());
        let (text, buf) = lab.frame_at(120, 34);
        let row = Lab::row_of(&text, "one two three four");
        let x = column_of(&text[row], "one");
        // Line by line: the whole current line is coral.
        assert_eq!(buf[(x + "one two three ".len() as u16, row as u16)].style().fg, Some(CORAL));
    }

    #[test]
    fn no_lyrics_leaves_solo_as_it_was_with_a_dim_hint() {
        let mut lab = Lab::solo(12.5, 120, 34);
        let plain = lab.app.solo_geometry().unwrap();
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "c".into(), status: LyricsStatus::None, lines: vec![] };
        lab.press(&[ch('l')]);
        lab.advance(3000); // the "Lyrics: on" message is gone
        assert_eq!(lab.app.solo_geometry(), Some(plain));
        let (text, buf) = lab.frame_at(120, 34);
        assert!(text[33].trim() == "No lyrics", "{:?}", text[33]);
        let x = column_of(&text[33], "No lyrics");
        assert!(buf[(x, 33)].style().add_modifier.contains(Modifier::DIM), "dim");
        // Not signed in.
        lab.script.lock().unwrap().status.signed_in = false;
        lab.advance(600);
        assert!(lab.frame_at(120, 34).0[33].contains("Sign in for lyrics"));
    }

    /// The first verses of song 6830888979 (BXZX & Brz, "Рви моё сердце"), which Apple has only
    /// unsynced: the app's reply, as it came off the socket (every line at 0.0).
    const RVI: [&str; 19] = [
        "Постой", "Нервы сдают беды с головой", "Крики мои это твой покой", "Рви моё сердце когтями", "Кровь под ногами", "Выход такой простой",
        "Я ещё здесь но какой ценой", "Крики мои это твой покой", "Рви моё сердце когтями", "Боль между нами", "Без неё мир пустой", "Бьёт холодный взгляд",
        "Меня накроет первобытный страх", "Ставлю точки в ряд", "А ты ломаешь пальцы на руках", "Мой срок минус день", "Научи меня молчать", "Ты стрелок я мишень",
        "Мотаешь время вспять",
    ];

    /// The wire reply for song `id` with the first `n` lines of [`RVI`] (repeated past 19).
    fn plain_reply(id: &str, n: usize) -> Reply {
        let lines: Vec<String> = (0..n).map(|i| format!(r#"{{"start":0.0,"end":0.0,"text":"{}","second_voice":false}}"#, RVI[i % RVI.len()])).collect();
        let json = format!(r#"{{"reply":"lyrics","song_id":"{id}","status":"plain","lines":[{}]}}"#, lines.join(","));
        serde_json::from_str(&json).unwrap()
    }

    /// Screen 5 shows every line of the live plain reply, from the top, with no highlight and a
    /// small label, and j/k scroll it.
    #[test]
    fn the_live_plain_reply_shows_on_screen_5_and_scrolls() {
        let mut lab = Lab::bare(127.5);
        lab.script.lock().unwrap().status.state = PlayState::Paused;
        lab.script.lock().unwrap().lyrics = plain_reply("c", 58);
        lab.press(&[ch('5')]);
        let (text, buf) = lab.frame();
        assert!(!text.iter().any(|l| l.contains("No lyrics")));
        assert!(text[8].contains("not time-synced"), "{}", text[8]);
        assert!(text[10].contains("Постой") && text[11].contains("Нервы сдают"), "{}", text[10]);
        for (i, line) in RVI.iter().enumerate().skip(1).take(10) {
            let st = style_of(&buf, &text, line);
            assert!(st.fg != Some(CORAL) && !st.add_modifier.contains(Modifier::BOLD) && !st.add_modifier.contains(Modifier::DIM), "line {i} has no highlight");
        }
        lab.press(&[ch('j'), ch('j')]);
        assert!(lab.frame().0[10].contains("Крики мои"));
        lab.press(&[ch('k')]);
        assert!(lab.frame().0[10].contains("Нервы сдают"));
        lab.press(&[ch('G')]);
        assert!(lab.frame().0[22].contains(RVI[57 % 19]));
    }

    /// An app that said "none" (its own fetch failed) is asked again now and then, so a later
    /// plain answer shows up without changing songs.
    #[test]
    fn a_none_answer_is_asked_again_and_a_later_plain_one_shows() {
        let mut lab = Lab::bare(5.0);
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "c".into(), status: LyricsStatus::None, lines: vec![] };
        lab.press(&[ch('5')]);
        assert!(lab.frame().0.iter().any(|l| l.contains("No lyrics")));
        let asks = lab.asks();
        lab.advance(5000);
        assert_eq!(lab.asks(), asks, "not at once");
        lab.script.lock().unwrap().lyrics = plain_reply("c", 5);
        lab.advance(6000);
        assert_eq!(lab.asks(), asks + 1);
        assert!(lab.frame().0[10].contains("Постой"));
        // Lyrics that came are not asked for again.
        lab.advance(30_000);
        assert_eq!(lab.asks(), asks + 1);
    }

    /// Plain lyrics that fit show in solo, centred, without highlight and with the label on the
    /// last row (longer ones: `long_plain_lyrics_show_in_solo_and_scroll`).
    #[test]
    fn plain_lyrics_that_fit_show_in_solo_centred() {
        let mut lab = Lab::solo(12.5, 120, 34);
        lab.script.lock().unwrap().lyrics = plain_reply("c", 6);
        lab.press(&[ch('l')]);
        lab.advance(3000);
        let g = lab.app.solo_geometry().unwrap();
        assert!(g.lyrics.is_some(), "laid out with lyrics");
        let (text, buf) = lab.frame_at(120, 34);
        let r = g.lyrics.unwrap();
        let first = Lab::row_of(&text, "Постой");
        assert!(first as u16 > r.y + 5 && Lab::row_of(&text, "Выход такой") == first + 5, "centred, every line: {first}");
        let st = style_of(&buf, &text, "Постой");
        assert!(st.fg != Some(CORAL) && !st.add_modifier.contains(Modifier::BOLD) && !st.add_modifier.contains(Modifier::DIM));
        assert_eq!(text[33].trim(), "not time-synced");
        assert!(!text.iter().any(|l| l.contains("No lyrics")));
        // Time passing moves nothing.
        lab.advance(20_000);
        assert_eq!(Lab::row_of(&lab.frame_at(120, 34).0, "Постой"), first);
    }

    #[test]
    fn an_old_app_leaves_solo_alone() {
        let mut lab = Lab::solo(12.5, 120, 34);
        let plain = lab.app.solo_geometry().unwrap();
        lab.script.lock().unwrap().old_app = true;
        lab.press(&[ch('l')]);
        lab.advance(3000);
        assert_eq!(lab.app.solo_geometry(), Some(plain));
        assert!(lab.frame_at(120, 34).0[33].contains("Update Uzumaki for lyrics"));
    }

    #[test]
    fn loading_shows_the_hint_then_the_layout_changes_and_the_cover_follows() {
        let mut lab = Lab::solo(12.5, 120, 34);
        let plain = lab.app.solo_geometry().unwrap();
        lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "c".into(), status: LyricsStatus::Loading, lines: vec![] };
        lab.press(&[ch('l')]);
        lab.advance(3000);
        assert_eq!(lab.app.solo_geometry(), Some(plain));
        assert!(lab.frame_at(120, 34).0[33].contains("Loading lyrics…"));
        lab.script.lock().unwrap().lyrics = fake_lyrics("c");
        lab.app.redraw = false;
        lab.advance(1000);
        let g = lab.app.solo_geometry().unwrap();
        assert!(g.lyrics.is_some() && g.cover != plain.cover);
        assert!(lab.app.redraw, "the cover moved: the screen is cleared");
        let (_, buf) = lab.frame_at(120, 34);
        assert!(rect_has_picture(&buf, g.cover.unwrap()), "encoded for the new rectangle");
        assert!(!(plain.cover.unwrap().x..plain.cover.unwrap().x + 4).all(|x| picture(&buf, x, plain.cover.unwrap().y)) || g.cover.unwrap().x <= plain.cover.unwrap().x + 4);
    }

    #[test]
    fn a_new_song_keeps_the_lyrics_layout_while_its_lyrics_load() {
        let mut lab = Lab::solo(12.5, 120, 34);
        lab.press(&[ch('l')]);
        let with = lab.app.solo_geometry().unwrap();
        {
            let mut sc = lab.script.lock().unwrap();
            sc.status.song = Some(s("d", "Other Song", "Daft Punk", "Random Access Memories", 200.0));
            sc.lyrics = Reply::Lyrics { song_id: "d".into(), status: LyricsStatus::Loading, lines: vec![] };
        }
        lab.advance(500);
        assert_eq!(lab.app.solo_geometry(), Some(with), "no jumping cover");
        assert!(lab.frame_at(120, 34).0.iter().any(|l| l.contains("Loading lyrics…")));
        lab.script.lock().unwrap().lyrics = fake_lyrics("d");
        lab.advance(1000);
        assert_eq!(lab.app.lyrics.song.as_deref(), Some("d"));
        assert!(lab.frame_at(120, 34).0.iter().any(|l| l.contains("one two three four")));
    }

    #[test]
    fn the_line_moves_with_the_clock_in_solo() {
        let mut lab = Lab::solo(12.5, 120, 34);
        lab.press(&[ch('l')]);
        assert_eq!(lab.app.lyrics.cursor(lab.app.lyrics.clock(lab.app.position(lab.now))).0, Some(0));
        lab.script.lock().unwrap().status.position = 14.5; // the app reports the second line
        lab.advance(600);
        let (text, _) = lab.frame_at(120, 34);
        let ly = lab.app.solo_geometry().unwrap().lyrics.unwrap();
        assert_eq!(Lab::row_of(&text, "second line here") as u16, ly.y + ly.height / 2);
        assert!(lab.app.wait(lab.now) <= Duration::from_secs(4), "the loop wakes for the next line");
    }

    #[test]
    fn solo_lyrics_geometry_fits_every_size() {
        use super::super::showcase::solo_lyrics;
        let caps = Some(blocks());
        for (w, h) in [(0, 0), (1, 1), (5, 3), (20, 6), (40, 10), (60, 14), (79, 30), (80, 10), (80, 9), (100, 24), (200, 60), (60, 90)] {
            if let Some(g) = solo_lyrics(w, h, caps) {
                let ly = g.lyrics.unwrap();
                assert!(ly.x + ly.width <= w && ly.y + ly.height <= h, "{w}x{h}: {ly:?}");
                if let Some(c) = g.cover {
                    assert!(c.x + c.width <= w && c.y + c.height <= h);
                    assert!(c.x + c.width <= ly.x || c.y + c.height <= ly.y, "{w}x{h}: cover {c:?} overlaps {ly:?}");
                }
                assert!(g.bar.y + 1 < h, "{w}x{h}: the time row is on screen");
            }
        }
        assert!(solo_lyrics(120, 34, caps).is_some_and(|g| g.lyrics.unwrap().x > 40), "wide: beside");
        assert!(solo_lyrics(60, 40, caps).is_some_and(|g| g.lyrics.unwrap().x == 2), "narrow: under");
        // Beside whenever a 34-column cover column and 32 lyrics columns fit: square windows too.
        for (w, h) in [(110, 44), (120, 48), (100, 40), (90, 60), (70, 40)] {
            let g = solo_lyrics(w, h, caps).unwrap();
            let (c, ly) = (g.cover.unwrap(), g.lyrics.unwrap());
            assert!(c.x + c.width <= ly.x && ly.width >= 32 && ly.y == 1 && ly.height == h - 2, "{w}x{h}: {c:?} {ly:?}");
        }
        assert!(solo_lyrics(69, 40, caps).is_some_and(|g| g.lyrics.unwrap().x == 2), "69 columns: under");
        assert!(solo_lyrics(110, 44, None).is_some_and(|g| g.cover.is_none() && g.lyrics.unwrap().x > 40), "no cover: text and lyrics side by side");
        assert!(solo_lyrics(60, 12, caps).is_none(), "no room under the song");
        let mut lab = Lab::solo(12.5, 100, 24);
        lab.press(&[ch('l')]);
        for (w, h) in [(1, 1), (5, 3), (20, 6), (30, 12), (60, 14), (200, 60), (60, 90)] {
            lab.app.resize_to(w, h);
            lab.advance(10);
            let _ = draw(&lab.app, lab.now, w, h);
        }
    }

    /// A roughly square window (about 1065×950 px: 105-120 columns by 40-48 rows): solo, `l`, lyrics
    /// still loading at first, then synced. They sit right of the cover at every such size.
    #[test]
    fn l_puts_the_lyrics_right_of_the_cover_in_a_roughly_square_window() {
        for (w, h) in [(100u16, 40u16), (105, 40), (110, 44), (120, 48), (120, 40)] {
            let mut lab = Lab::solo(12.5, w, h);
            lab.script.lock().unwrap().lyrics = Reply::Lyrics { song_id: "c".into(), status: LyricsStatus::Loading, lines: vec![] };
            lab.press(&[ch('l')]);
            assert!(!lab.app.solo_lyrics_shown());
            lab.script.lock().unwrap().lyrics = fake_lyrics("c");
            lab.advance(1000);
            let (text, buf) = lab.frame_at(w, h);
            let g = lab.app.solo_geometry().unwrap();
            let (c, ly) = (g.cover.unwrap(), g.lyrics.expect("lyrics rect"));
            assert!(c.x + c.width <= ly.x && ly.width >= 32, "{w}x{h}: cover {c:?} left of lyrics {ly:?}");
            assert!(rect_has_picture(&buf, c));
            let current = Lab::row_of(&text, "one two three four");
            assert!(column_of(&text[current], "one") >= ly.x, "{w}x{h}");
        }
    }

    /// Plain lyrics taller than the box used to leave solo alone (only a dim hint on the last
    /// row, so it looked as if `l` did nothing). They show now, from the top, and `j`/`k` scroll.
    #[test]
    fn long_plain_lyrics_show_in_solo_and_scroll() {
        let mut lab = Lab::solo(12.5, 110, 44);
        let plain = lab.app.solo_geometry().unwrap();
        lab.script.lock().unwrap().lyrics = plain_reply("c", 80);
        lab.press(&[ch('l')]);
        lab.advance(3000);
        let g = lab.app.solo_geometry().unwrap();
        assert!(g.lyrics.is_some() && g.cover != plain.cover, "laid out with lyrics");
        let text = lab.frame_at(110, 44).0;
        assert_eq!(text[43].trim(), "not time-synced · j/k scroll");
        let first = Lab::row_of(&text, "Постой");
        assert_eq!(first as u16, g.lyrics.unwrap().y, "from the top");
        lab.press(&[ch('j'), ch('j'), ch('j')]);
        let text = lab.frame_at(110, 44).0;
        assert!(text.iter().any(|l| l.contains("Постой")));
        let scrolled = lab.app.lyrics.manual;
        assert_eq!(scrolled, Some(3));
        lab.press(&[ch('k')]);
        assert_eq!(lab.app.lyrics.manual, Some(2));
        lab.press(&[ch('G')]);
        assert!(lab.app.lyrics.manual > Some(3));
        // Synced lyrics ignore j/k in solo (they follow the song).
        let mut lab = Lab::solo(12.5, 110, 44);
        lab.press(&[ch('l')]);
        lab.press(&[ch('j')]);
        assert_eq!(lab.app.lyrics.manual, None);
    }

    #[test]
    fn the_toggle_messages_say_what_changed() {
        let mut lab = Lab::solo(12.5, 110, 44);
        lab.press(&[ch('l')]);
        assert_eq!(lab.app.message(lab.now), Some("Lyrics shown"));
        lab.press(&[ch('l')]);
        assert_eq!(lab.app.message(lab.now), Some("Lyrics hidden (l to show)"));
    }

    #[test]
    fn help_and_uzu_help_list_l() {
        let mut lab = Lab::solo(12.5, 120, 34);
        lab.press(&[ch('?')]);
        let help = lab.frame_at(120, 34).0.join("\n");
        assert!(help.contains("in solo: show / hide lyrics"), "{help}");
        assert!(crate::cli::HELP.contains("In solo, l shows the lyrics"));
    }

}

// ------------------------------------------------------------------------ the next lap (repeat all)

/// The fixture's queue plus a next lap of three songs.
fn lapped(now: Instant) -> App {
    let mut app = fixture(now);
    let mut q = app.queue.clone();
    q.next_lap = vec![s("n1", "Lap One", "Aa", "Album A", 100.0), s("n2", "Lap Two", "Bb", "Album B", 200.0), s("n3", "Lap Three", "Cc", "Album C", 300.0)];
    app.set_queue(q, now);
    app.resize(30);
    app
}

#[test]
fn the_next_lap_follows_a_repeat_divider_dimmed_and_numbered_on() {
    let now = Instant::now();
    let app = lapped(now);
    let text = lines(&draw(&app, now, 140, 30));
    let at = text.iter().position(|l| l.contains("↻ Repeat")).expect("a Repeat divider");
    assert!(text[at - 1].contains("Lose Yourself to Dance"), "the rest of this lap comes first: {}", text[at - 1]);
    assert!(text[at + 1].contains("Lap One") && text[at + 1].contains(" 8 "), "{}", text[at + 1]);
    assert!(text[at + 3].contains("Lap Three") && text[at + 3].contains(" 10 "), "{}", text[at + 3]);
    // The divider and the lap's songs are dim; the songs before the divider are not.
    let buf = draw(&app, now, 140, 30);
    let dim_at = |y: usize, needle: &str| {
        let x = text[y].find(needle).map(|b| text_width(&text[y][..b])).unwrap();
        buf[(x as u16, y as u16)].modifier.contains(Modifier::DIM)
    };
    assert!(dim_at(at, "↻ Repeat") && dim_at(at + 1, "Lap One") && dim_at(at + 3, "Lap Three"));
    assert!(!dim_at(at - 1, "Lose Yourself"));
    // The player's own count and length stay about this lap's queue.
    assert!(text.iter().any(|l| l.contains("Queue:   8 songs")), "{:?}", text.iter().find(|l| l.contains("Queue:")));
}

#[test]
fn the_cursor_skips_the_divider_and_a_lap_song_can_be_played_but_not_edited() {
    let mut rig = Rig::new("lap");
    let now = rig.now;
    let mut q = rig.app.queue.clone();
    let n = q.up_next.len();
    q.next_lap = vec![s("n1", "Lap One", "Aa", "A", 100.0), s("n2", "Lap Two", "Bb", "B", 200.0)];
    rig.app.set_queue(q, now);
    rig.press(&[ch('G')]);
    assert_eq!(rig.app.cursor, n + 2 + 1, "G lands on the last lap song (playing row + {n} + divider + 2)");
    rig.press(&[ch('k')]);
    rig.press(&[ch('k')]);
    assert_eq!(rig.app.cursor, n, "k from the first lap song steps over the divider onto the last up-next song");
    rig.press(&[ch('j')]);
    assert_eq!(rig.app.cursor, n + 2, "and j steps back over it");
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Jump { index: n });
    let sent = rig.log.lock().unwrap().len();
    rig.press(&[ch('d'), ch('J'), ch('K')]);
    assert_eq!(rig.log.lock().unwrap().len(), sent, "nothing is sent for edits of the next lap");
    assert_eq!(rig.app.message(rig.now), Some("The next lap can't be edited"));
    // A click on the divider does nothing; a filter hides it.
    rig.press(&[ch('/'), ch('L'), ch('a'), ch('p')]);
    let rows = rig.app.rows();
    assert!(rig.app.visible().iter().all(|&i| rows[i].kind != RowKind::Repeat));
}

// ------------------------------------------------------------------------ a radio station

/// A station playing: one song queued by hand, three the station already played.
fn on_station(rig: &mut Rig) {
    let now = rig.now;
    let mut q = Queue { current: rig.app.queue.current.clone(), queued: 1, source: "Stazione di Thomas".into(), ..Default::default() };
    q.up_next = vec![s("u", "Queued By Hand", "Uu", "U", 100.0)];
    q.station = Some("Stazione di Thomas".into());
    q.station_played = vec![s("p1", "Played One", "Aa", "A", 100.0), s("p2", "Played Two", "Bb", "B", 200.0), s("p3", "Played Three", "Cc", "C", 300.0)];
    rig.app.set_queue(q, now);
    rig.app.resize(30);
}

#[test]
fn a_station_shows_its_header_and_the_songs_it_played_after_the_queued_ones() {
    let mut rig = Rig::new("station");
    on_station(&mut rig);
    let now = rig.now;
    let buf = draw(&rig.app, now, 120, 30);
    let text = lines(&buf);
    let at = text.iter().position(|l| l.contains("Stazione di Thomas \u{2014} songs are picked as you listen")).expect("the header");
    assert!(text[at].contains('◉'));
    assert!(text[at - 1].contains("Queued By Hand"), "queued songs stay above: {}", text[at - 1]);
    assert!(text[at + 1].contains("Played One") && text[at + 1].contains(" 2 "), "numbered on from Up Next: {}", text[at + 1]);
    assert!(text[at + 3].contains("Played Three") && text[at + 3].contains(" 4 "), "{}", text[at + 3]);
    let x = text[at + 1].find("Played One").map(|b| text_width(&text[at + 1][..b])).unwrap();
    assert!(buf[(x as u16, (at + 1) as u16)].modifier.contains(Modifier::DIM), "history is dim");
    // No "Nothing up next" while the station has a header, and the song count is the queue's.
    assert!(!text.iter().any(|l| l.contains("Nothing up next")));
}

#[test]
fn the_cursor_skips_the_station_header_and_enter_plays_a_played_song() {
    let mut rig = Rig::new("station-keys");
    on_station(&mut rig);
    rig.press(&[ch('G')]);
    assert_eq!(rig.app.cursor, 5, "playing row, queued song, header, then the three played songs");
    rig.press(&[ch('k'), ch('k')]);
    assert_eq!(rig.app.cursor, 3, "the last click of k stands on the first played song");
    rig.press(&[ch('k')]);
    assert_eq!(rig.app.cursor, 1, "and one more steps over the header onto the queued song");
    rig.press(&[ch('j'), ch('j')]);
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Jump { index: 2 }, "the second played song is entry 3");
    rig.press(&[ch('d'), ch('J')]);
    assert_eq!(rig.app.message(rig.now), Some("Songs the station played can't be edited"));
    // The queued song is still editable; a filter hides the header.
    rig.press(&[ch('/'), ch('P'), ch('l')]);
    let rows = rig.app.rows();
    assert!(rig.app.visible().iter().all(|&i| rows[i].kind != RowKind::Station));
}

#[test]
fn the_plain_queue_names_the_station_instead_of_saying_nothing_is_up_next() {
    let q = Queue { station: Some("Stazione di Thomas".into()), ..Default::default() };
    let text = crate::run::queue_lines(&q, None, false, crate::run::Term { color: false, width: 80 }).join("\n");
    assert!(text.contains("Stazione di Thomas \u{2014} songs are picked as you listen"), "{text}");
    assert!(!text.contains("Nothing up next"));
}

#[test]
fn general_history_is_above_current_initially_hidden_and_cursor_survives_updates() {
    let now = Instant::now();
    let mut app = App::new(now, Box::new(Inline::new(Box::new(NoApp))));
    app.resize_to(100, 24);
    let q = Queue {
        history: (0..30).map(|i| s(&format!("h{i}"), "Played", "Artist", "Album", 120.0)).collect(),
        current: Some(s("now", "Current", "Artist", "Album", 120.0)),
        up_next: (0..30).map(|i| s(&format!("next{i}"), "Next", "Artist", "Album", 120.0)).collect(),
        ..Default::default()
    };
    app.set_queue(q.clone(), now);
    assert!(app.rows()[0].kind == RowKind::History);
    assert!(app.rows()[app.cursor].playing);
    assert_eq!(app.offset, 31);
    let mut q2 = q;
    q2.history.push(s("last", "Last", "Artist", "Album", 120.0));
    app.set_queue(q2, now);
    assert!(app.rows()[app.cursor].playing);
    assert_eq!(app.offset, 32);
}

#[test]
fn history_enter_uses_general_replay_without_changing_up_next_numbers() {
    let mut rig = Rig::new("general-history-enter");
    let mut q = rig.app.queue.clone();
    q.history = vec![s("old", "Old track", "Artist", "Album", 120.0)];
    rig.app.set_queue(q, rig.now);
    rig.press(&[key(KeyCode::Home), key(KeyCode::Home), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::History { index: 0 });
}

#[test]
fn short_history_queue_can_place_current_at_top_without_upcoming_tracks() {
    let now = Instant::now();
    let mut app = App::new(now, Box::new(Inline::new(Box::new(NoApp))));
    app.resize_to(100, 30);
    app.set_queue(Queue {
        history: vec![s("old", "Old", "Artist", "Album", 120.0)],
        current: Some(s("now", "Current", "Artist", "Album", 120.0)),
        ..Default::default()
    }, now);
    assert_eq!(app.offset, 2);
    assert!(app.rows()[app.cursor].playing);
}

// -------------------------------------------------------------- history as its own zone

/// Three songs played before this one (oldest first, as the app sends them), the playing song,
/// two in Up Next and, with repeat all, a next lap of the same two.
fn with_history(rig: &mut Rig) {
    let now = rig.now;
    let mut q = Queue { current: Some(s("c", "Instant Crush", "Daft Punk", "Random Access Memories", 239.0)), source: "Mix".into(), ..Default::default() };
    q.history = vec![s("h1", "Oldest Played", "Aa", "A", 100.0), s("h2", "Middle Played", "Bb", "B", 110.0), s("h3", "Just Played", "Cc", "C", 120.0)];
    q.up_next = vec![s("n1", "Next One", "Dd", "D", 130.0), s("n2", "Next Two", "Ee", "E", 140.0)];
    q.next_lap = vec![s("l1", "Lap One", "Ff", "F", 150.0), s("l2", "Lap Two", "Gg", "G", 160.0)];
    rig.app.set_queue(q, now);
}

#[test]
fn history_is_a_separate_zone_with_a_header_and_h_numbers_counting_up_from_the_playing_song() {
    let mut rig = Rig::new("history-zone");
    with_history(&mut rig);
    rig.app.resize_to(120, 20);
    rig.press(&[key(KeyCode::Home)]);
    let buf = draw(&rig.app, rig.now, 120, 20);
    let text = lines(&buf);
    let head = text.iter().position(|l| l.contains("⟲ History · 3 songs")).expect("the header");
    assert!(text[head + 1].contains("H3") && text[head + 1].contains("Oldest Played"), "{}", text[head + 1]);
    assert!(text[head + 2].contains("H2") && text[head + 2].contains("Middle Played"));
    assert!(text[head + 3].contains("H1") && text[head + 3].contains("Just Played"), "H1 is right above the playing song");
    assert!(text[head + 4].contains("Instant Crush") && text[head + 4].contains('▶'));
    assert!(text[head + 5].contains("Next One") && text[head + 5].contains(" 1 "), "Up Next numbers are unchanged: {}", text[head + 5]);
    assert!(text.iter().any(|l| l.contains("↻ Repeat")));
    // The history rows are dim, and the H labels line up on the right edge of the number column.
    let x = text[head + 1].find("Oldest Played").map(|b| text_width(&text[head + 1][..b])).unwrap();
    assert!(buf[(x as u16, (head + 1) as u16)].modifier.contains(Modifier::DIM));
    let right_edge = |row: usize, tag: &str| text_width(&text[row][..text[row].find(tag).unwrap() + tag.len()]);
    assert_eq!(right_edge(head + 1, "H3"), right_edge(head + 5, " 1 ") - 1, "H3 and 1 end in the same column");
}

#[test]
fn the_number_column_fits_h12_and_the_up_next_numbers_stay_right_aligned() {
    let mut rig = Rig::new("history-wide-num");
    let now = rig.now;
    let mut q = Queue { current: Some(s("c", "Now", "Aa", "A", 100.0)), ..Default::default() };
    q.history = (0..12).map(|i| s(&format!("h{i}"), &format!("Past {i}"), "Aa", "A", 100.0)).collect();
    q.up_next = vec![s("n1", "Next One", "Dd", "D", 130.0)];
    rig.app.set_queue(q, now);
    rig.app.resize_to(120, 40);
    rig.press(&[key(KeyCode::Home)]); // the queue opens with the history scrolled off
    assert_eq!(rig.app.num_width(), 3);
    let text = lines(&draw(&rig.app, now, 120, 40));
    let row = |t: &str| text.iter().find(|l| l.contains(t)).cloned().unwrap();
    let end = |l: &str, tag: &str| text_width(&l[..l.find(tag).unwrap() + tag.len()]);
    assert!(row("Past 0").contains("H12") && row("Past 11").contains(" H1 "), "{}", row("Past 11"));
    assert_eq!(end(&row("Past 0"), "H12"), end(&row("Next One"), " 1  ") - 2, "right-aligned in one column");
}

#[test]
fn enter_on_a_history_row_still_replays_that_entry_and_the_header_is_skipped() {
    let mut rig = Rig::new("history-enter-h");
    with_history(&mut rig);
    rig.app.resize_to(120, 20);
    rig.press(&[key(KeyCode::Home)]);
    let rows = rig.app.rows();
    assert!(rows[rig.app.cursor].kind == RowKind::Song && !rows[rig.app.cursor].playing, "Home first goes to Up Next 1");
    rig.press(&[ch('g')]);
    let rows = rig.app.rows();
    assert!(rows[0].kind == RowKind::History, "the header is the first row");
    assert!(rows[rig.app.cursor].kind == RowKind::Past(3), "g again: the top of the history, skipping the header");
    rig.press(&[ch('g')]);
    assert!(rig.app.rows()[rig.app.cursor].kind == RowKind::Song, "and once more back to Up Next 1");
    rig.press(&[ch('g')]);
    assert!(rig.app.rows()[rig.app.cursor].kind == RowKind::Past(3));
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::History { index: 0 });
    rig.press(&[ch('j'), ch('j'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::History { index: 2 }, "H1 is the newest entry");
    rig.press(&[ch('d')]);
    assert_eq!(rig.app.message(rig.now), Some("History songs can't be edited"));
}

#[test]
fn the_showcase_cursor_lands_on_the_first_up_next_song_not_on_the_history() {
    let mut rig = Rig::new("history-showcase");
    rig.app.resize_to(140, 30);
    with_history(&mut rig);
    let on = |rig: &Rig| rig.app.rows()[rig.app.cursor].song.id.clone();
    rig.press(&[ch('o')]);
    assert_eq!(on(&rig), "c", "on the normal screen o selects the playing row");
    rig.press(&[ch('F')]);
    assert!(rig.app.showcase().is_some());
    assert_eq!(on(&rig), "n1", "the playing row hides: the cursor moves to the first Up Next song");
    rig.press(&[ch('G')]);
    assert_eq!(on(&rig), "l2");
    rig.press(&[ch('o')]);
    assert_eq!(on(&rig), "n1", "o is the first Up Next song");
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Jump { index: 0 });
    // The history shows there too, as a zone of its own.
    let text = lines(&draw(&rig.app, rig.now, 140, 30));
    assert!(text.iter().any(|l| l.contains("⟲ History · 3 songs")));
    assert!(text.iter().any(|l| l.contains("H1") && l.contains("Just Played")));
    // With nothing up next, the playing row is hidden, so the first real song is a history one.
    let now = rig.now;
    rig.app.set_queue(Queue { current: rig.app.queue.current.clone(), history: rig.app.queue.history.clone(), ..Default::default() }, now);
    rig.press(&[ch('G'), ch('o')]);
    assert!(rig.app.rows()[rig.app.cursor].kind != RowKind::History, "never on the header");
}

fn big_history(rig: &mut Rig, hist: usize, up: usize) {
    let now = rig.now;
    let mut q = Queue { current: Some(s("c", "Playing Now", "Daft Punk", "RAM", 239.0)), ..Default::default() };
    q.history = (0..hist).map(|i| s(&format!("h{i}"), &format!("Past {i}"), "Aa", "A", 100.0)).collect();
    q.up_next = (0..up).map(|i| s(&format!("n{i}"), &format!("Next {i}"), "Dd", "D", 130.0)).collect();
    rig.app.set_queue(q, now);
}

/// The table's rows in the showcase: what follows the line with the column titles.
fn showcase_table(rig: &Rig, w: u16, h: u16) -> Vec<String> {
    let text = lines(&draw(&rig.app, rig.now, w, h));
    let head = text.iter().position(|l| l.contains("Title") && l.contains("Artist")).expect("the table header");
    text[head + 1..].to_vec()
}

#[test]
fn the_showcase_opens_with_the_history_divider_visible_when_the_history_fits() {
    let mut rig = Rig::new("showcase-top");
    // A fresh app in the showcase gets its first queue: the cursor opens on Up Next 1.
    let jobs = Inline::new(Box::new(Client::connect_to(&socket("tui-showcase-top")).unwrap()));
    rig.app = App::new(rig.now, Box::new(jobs));
    rig.app.resize_to(160, 50);
    rig.press(&[ch('F')]);
    assert!(rig.app.showcase().is_some());
    big_history(&mut rig, 12, 186);
    let t = showcase_table(&rig, 160, 50);
    assert!(t[0].contains("⟲ History · 12 songs"), "first table row: {}", t[0]);
    assert!(t[1].contains("H12") && t[12].contains("H1"), "{} / {}", t[1], t[12]);
    assert!(t[13].contains("▸ Up Next · 186 songs"), "{}", t[13]);
    assert!(t[14].contains("Next 0") && t[14].contains(" 1 "), "{}", t[14]);
    let on = rig.app.rows()[rig.app.cursor].song.id.clone();
    assert_eq!(on, "n0", "the cursor opens on Up Next 1");
    // Pressing o and moving away and back keeps the divider on screen too.
    rig.press(&[ch('j'), ch('o')]);
    assert!(showcase_table(&rig, 160, 50)[0].contains("⟲ History"));
}

#[test]
fn a_showcase_too_short_for_the_history_starts_at_the_up_next_divider() {
    let mut rig = Rig::new("showcase-short");
    big_history(&mut rig, 12, 186);
    rig.app.resize_to(140, 24);
    rig.press(&[ch('F')]);
    if rig.app.showcase().is_none() {
        return;
    }
    let t = showcase_table(&rig, 140, 24);
    assert!(t[0].contains("⟲ History") || t[0].contains("▸ Up Next"), "never a bare history row first: {}", t[0]);
}

#[test]
fn the_up_next_divider_shows_in_the_showcase_only_with_a_history() {
    let mut rig = Rig::new("upnext-divider");
    with_history(&mut rig);
    rig.app.resize_to(140, 40);
    rig.press(&[ch('F')]);
    let t = showcase_table(&rig, 140, 40);
    let h1 = t.iter().position(|l| l.contains("Just Played")).unwrap();
    assert!(t[h1 + 1].contains("▸ Up Next · 2 songs"), "{}", t[h1 + 1]);
    assert!(t[h1 + 2].contains("Next One"));
    // The cursor skips it; Enter on Up Next 1 still jumps to entry 0.
    rig.press(&[ch('k')]);
    assert!(!rig.app.rows()[rig.app.cursor].kind.is_divider());
    rig.press(&[ch('o'), key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Jump { index: 0 });
    // The normal view has the playing row between them: no divider.
    rig.press(&[ch('F')]);
    assert!(rig.app.showcase().is_none());
    assert!(!lines(&draw(&rig.app, rig.now, 140, 40)).iter().any(|l| l.contains("▸ Up Next")));
    // No history, no divider, in the showcase either.
    let mut rig = Rig::new("upnext-divider-none");
    big_history(&mut rig, 0, 5);
    rig.app.resize_to(140, 40);
    rig.press(&[ch('F')]);
    assert!(!showcase_table(&rig, 140, 40).iter().any(|l| l.contains("▸ Up Next") || l.contains("History")));
}

/// Prints the history screens (run with `--ignored --nocapture --test-threads=1`).
#[test]
#[ignore = "prints the screens for review"]
fn dump_the_history_screens() {
    let mut rig = Rig::new("history-dump");
    with_history(&mut rig);
    rig.app.resize_to(110, 26);
    rig.press(&[key(KeyCode::Home)]);
    for (name, w, h) in [("normal (table part)", 110u16, 26u16)] {
        eprintln!("--- {name} ---");
        for l in lines(&draw(&rig.app, rig.now, w, h)).iter().skip(8).take(15) {
            eprintln!("{l}");
        }
    }
    rig.app.resize_to(140, 30);
    rig.press(&[ch('F')]);
    eprintln!("--- showcase ---");
    for l in lines(&draw(&rig.app, rig.now, 140, 30)).iter().take(15) {
        eprintln!("{l}");
    }
    let mut rig = Rig::new("history-dump-big");
    big_history(&mut rig, 12, 186);
    rig.app.resize_to(160, 50);
    rig.press(&[ch('F')]);
    eprintln!("--- showcase, 12 history + 186 Up Next (table rows) ---");
    for l in showcase_table(&rig, 160, 50).iter().take(16) {
        eprintln!("{l}");
    }
}

// ------------------------------------------------------------------------------- uzu.toml

use crate::config::Config;

/// A fixture app with the config the text gives.
fn with_config(text: &str, now: Instant) -> App {
    let mut app = fixture(now);
    app.set_config(Config::parse(text), now);
    app
}

#[test]
fn no_config_and_an_empty_one_draw_exactly_what_the_defaults_draw() {
    let now = Instant::now();
    for (w, h) in [(140, 20), (100, 30), (60, 14), (40, 12), (80, 10)] {
        let mut plain = fixture(now);
        plain.resize(h);
        let mut empty = with_config("# nothing here\n", now);
        empty.resize(h);
        assert_eq!(lines(&draw(&plain, now, w, h)), lines(&draw(&empty, now, w, h)), "{w}x{h}");
        assert_eq!(draw(&plain, now, w, h), draw(&empty, now, w, h), "styles differ at {w}x{h}");
    }
}

#[test]
fn a_problem_in_the_file_shows_in_the_status_line_and_leaves_the_screen_whole() {
    let now = Instant::now();
    let mut app = with_config("[theme]\nacent = \"red\"\nbad line\n", now);
    app.resize(20);
    let text = lines(&draw(&app, now, 140, 20));
    assert!(text[8].starts_with("│ uzu.toml line 2: unknown key 'acent' in [theme]; did you mean 'accent'?"), "{}", text[8]);
    assert!(text[8].contains("(+1 more; 'uzu config' lists them)"), "{}", text[8]);
    // The frame is intact: every row as wide as the terminal, borders where they belong.
    assert!(text.iter().all(|l| text_width(l) == 140));
    assert!(text[8].ends_with('│') && text[0].starts_with('╭') && text[19].starts_with('╰'));
    // A narrow screen cuts it instead of breaking the box.
    let narrow = lines(&draw(&app, now, 50, 20));
    assert!(narrow.iter().all(|l| text_width(l) == 50), "{narrow:?}");
    // It stays a while, then the usual line is back.
    let later = now + Duration::from_secs(9);
    assert!(lines(&draw(&app, later, 140, 20))[8].starts_with("│ Playing from: Anima"));
}

#[test]
fn no_problems_means_no_message() {
    let now = Instant::now();
    let mut app = with_config("[format]\nstatus_idle = \"-\"\n", now);
    app.resize(20);
    assert!(lines(&draw(&app, now, 140, 20))[8].starts_with("│ Playing from: Anima"));
}

/// The foreground colours used on a frame, as a set.
fn colours(buf: &Buffer) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Color::Reset = buf[(x, y)].fg {
                continue;
            }
            out.insert(format!("{:?}", buf[(x, y)].fg));
        }
    }
    out
}

#[test]
fn the_default_theme_uses_coral_green_cyan_and_magenta() {
    let now = Instant::now();
    let mut app = fixture(now);
    app.resize(20);
    let got = colours(&draw(&app, now, 140, 20));
    assert_eq!(got, ["Cyan", "Green", "Magenta", "Rgb(255, 122, 138)"].map(String::from).into_iter().collect());
}

#[test]
fn a_theme_in_the_file_recolours_the_roles() {
    let now = Instant::now();
    let mut app = with_config("[theme]\naccent = \"#112233\"\nnumber = \"red\"\nalbum = \"blue\"\ntime = 99\n", now);
    app.resize(20);
    let buf = draw(&app, now, 140, 20);
    assert_eq!(colours(&buf), ["Blue", "Indexed(99)", "Red", "Rgb(17, 34, 51)"].map(String::from).into_iter().collect());
    // The playing row's title carries the accent, the # column the number colour.
    let text = lines(&buf);
    let row = text.iter().position(|l| l.contains("Instant Crush") && l.contains('▶')).unwrap();
    let x = text[row].find("Instant Crush").map(|b| text[row][..b].chars().count()).unwrap();
    assert_eq!(buf[(x as u16, row as u16)].fg, Color::Rgb(17, 34, 51));
}

#[test]
fn mono_and_no_color_draw_no_colour_at_all() {
    let now = Instant::now();
    for app in [with_config("[theme]\npalette = \"mono\"\n", now), {
        let mut a = fixture(now);
        a.set_config(Config::parse("").with_no_color(true), now);
        a
    }] {
        let mut app = app;
        app.resize(20);
        let buf = draw(&app, now, 140, 20);
        assert!(colours(&buf).is_empty(), "{:?}", colours(&buf));
        // Bold and the selection still show: it is colour that goes, not emphasis.
        let bold = (0..20u16).any(|y| (0..140u16).any(|x| buf[(x, y)].modifier.contains(Modifier::BOLD)));
        assert!(bold);
    }
}

#[test]
fn the_paused_state_and_every_screen_follow_the_theme() {
    let now = Instant::now();
    let mut app = with_config("[theme]\npaused = \"light-red\"\naccent = \"green\"\n", now);
    app.set_status(Status { state: PlayState::Paused, song: app.queue.current.clone(), position: 72.0, duration: 239.0, volume: 0.82, connected: true, ..Default::default() }, now);
    app.resize(20);
    let buf = draw(&app, now, 140, 20);
    assert!(colours(&buf).contains("LightRed"), "{:?}", colours(&buf));
    // The accent also colours the tabs and the help's key names.
    app.help = true;
    let buf = draw(&app, now, 140, 24);
    assert!(colours(&buf).contains("Green") && !colours(&buf).contains("Rgb(255, 122, 138)"), "{:?}", colours(&buf));
    // One theme per frame: a frame with the default theme on the same thread is coral again.
    let mut plain = fixture(now);
    plain.resize(20);
    assert!(colours(&draw(&plain, now, 140, 20)).contains("Rgb(255, 122, 138)"));
}

// ------------------------------------------------------------------- uzu.toml: [keys]

fn rig_with(name: &str, config: &str) -> Rig {
    let mut rig = Rig::new(name);
    rig.app.set_config(Config::parse(config), rig.now);
    rig
}

#[test]
fn remapped_keys_drive_the_same_actions() {
    use uzumaki_ctl::VolumeTo;
    let mut rig = rig_with("keys-remap", "[keys]\nnext = [\"]\", \"ctrl+n\"]\nvolume_up = \"u\"\nplay_pause = \"enter\"\nselect = \"x\"\n");
    rig.press(&[ch(']')]);
    assert_eq!(rig.last(), Request::Next);
    rig.log.lock().unwrap().clear();
    rig.press(&[KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)]);
    assert_eq!(rig.last(), Request::Next);
    rig.press(&[ch('u')]);
    assert_eq!(rig.last(), Request::Volume { to: VolumeTo::Change(0.05) });
    // The old keys do nothing: n, + and = (volume_up's two defaults).
    rig.log.lock().unwrap().clear();
    rig.press(&[ch('n'), ch('+'), ch('=')]);
    assert!(rig.log.lock().unwrap().is_empty(), "{:?}", rig.log.lock().unwrap());
    // Enter is play/pause now, and x plays the selected song.
    rig.press(&[key(KeyCode::Enter)]);
    assert_eq!(rig.last(), Request::Toggle);
    rig.press(&[ch('j'), ch('x')]);
    assert_eq!(rig.last(), Request::Jump { index: 0 });
}

#[test]
fn an_unbound_quit_key_does_not_quit_but_ctrl_c_still_does() {
    let mut rig = rig_with("keys-quit", "[keys]\nquit = []\n");
    rig.press(&[ch('q')]);
    assert!(!rig.app.quit);
    rig.press(&[KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)]);
    assert!(rig.app.quit);
    // Esc on the queue still quits (its own action, `back`).
    let mut rig = rig_with("keys-quit2", "[keys]\nquit = []\n");
    rig.press(&[key(KeyCode::Esc)]);
    assert!(rig.app.quit);
}

#[test]
fn screens_and_lists_follow_the_keymap() {
    let mut rig = rig_with("keys-screens", "[keys]\ngoto_search = \"s\"\nshuffle = \"S\"\ndown = [\"ctrl+j\"]\n");
    assert!(rig.app.config.warnings.is_empty());
    rig.press(&[ch('3')]);
    assert_eq!(rig.app.screen, Screen::Queue, "3 is not bound to Search any more");
    rig.press(&[ch('s')]);
    assert_eq!(rig.app.screen, Screen::Search);
    // Search opens in its box: the first Esc leaves the box, the second goes back.
    rig.press(&[key(KeyCode::Esc), key(KeyCode::Esc)]);
    assert_eq!(rig.app.screen, Screen::Queue);
    rig.press(&[KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)]);
    assert_eq!(rig.app.cursor, 1);
    rig.press(&[ch('j'), key(KeyCode::Down)]);
    assert_eq!(rig.app.cursor, 1, "j and Down were replaced by ctrl+j");
}

#[test]
fn typing_takes_every_key_whatever_the_keymap_says() {
    let mut rig = rig_with("keys-typing", "[keys]\nquit = []\nfilter = \"F4\"\nnext = \"x\"\n");
    // `/` is no longer the filter key (F4 is).
    rig.press(&[ch('/')]);
    assert!(!rig.app.filtering);
    rig.press(&[key(KeyCode::F(4))]);
    assert!(rig.app.filtering);
    rig.press(&[ch('q')]);
    assert_eq!(rig.app.filter, "q");
    let mut rig = rig_with("keys-typing2", "[keys]\nfilter = \"v\"\nnext = \"x\"\nquit = \"z\"\n");
    rig.press(&[ch('v')]);
    assert!(rig.app.filtering);
    // Inside the filter every key is text, even the ones bound or unbound.
    rig.press(&[ch('x'), ch('z'), ch('n'), ch('q')]);
    assert_eq!(rig.app.filter, "xznq");
    assert!(!rig.app.quit);
    rig.press(&[key(KeyCode::Esc)]);
    assert!(!rig.app.filtering);
    // The search box too.
    let mut rig = rig_with("keys-search", "[keys]\ngoto_search = \"s\"\nfilter = \"v\"\nquit = \"z\"\n");
    rig.press(&[ch('s')]);
    assert!(rig.app.search.typing, "a search with no results opens in the box");
    rig.press(&[ch('z'), ch('s'), ch('q')]);
    assert_eq!(rig.app.search.input, "zsq");
    assert!(!rig.app.quit);
}

#[test]
fn the_help_and_the_hints_show_the_remapped_keys() {
    let now = Instant::now();
    let mut app = with_config("[keys]\nnext = \"ctrl+n\"\nquit = []\n", now);
    app.resize(30);
    let text = lines(&draw(&app, now, 140, 30));
    let hints = text.last().unwrap();
    assert!(hints.contains("ctrl+n/p next/prev") && hints.contains("– quit"), "{hints}");
    app.help = true;
    let help = lines(&draw(&app, now, 140, 40));
    assert!(help.iter().any(|l| l.contains("ctrl+n  p") && l.contains("next / previous song")), "{help:#?}");
    assert!(help.iter().any(|l| l.contains("–  Ctrl+C") && l.contains("quit")), "{help:#?}");
    // With the default keymap both are as they were.
    let mut plain = fixture(now);
    plain.resize(30);
    assert!(lines(&draw(&plain, now, 140, 30)).last().unwrap().contains("n/p next/prev"));
}

#[test]
fn a_keymap_works_in_every_view_and_a_popup_still_takes_any_key() {
    let mut rig = rig_with("keys-popups", "[keys]\nhelp = \"h\"\nsort = \"S\"\n");
    rig.press(&[ch('?')]);
    assert!(!rig.app.help, "? is not the help key any more");
    rig.press(&[ch('h')]);
    assert!(rig.app.help);
    // Any key closes the help, bound or not.
    rig.press(&[ch('z')]);
    assert!(!rig.app.help);
    // The sort popup reads its own letters (n, a, y, …) untranslated.
    rig.press(&[ch('2')]);
    rig.press(&[ch('S')]);
    assert!(rig.app.sorting.is_some());
}
