//! Runs a [`Command`] against a [`Backend`] (the app's socket, or the disk cache) and writes
//! what the user sees to `out`. Nothing here reads globals: width, colour and timing come in
//! through [`Env`], so tests drive it with a fake server.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use uzumaki_ctl::{Client, CollectionInfo, Kind, PickAction, PlayState, Queue, Reply, Request, SongInfo, Status};

use crate::args::{self, Found};
use crate::cache;
use crate::cli::{Command, Invocation};
use crate::fmt::{self, bold, dim, plain, render};
use crate::last::{self, Last};
use crate::list;

pub const NOT_RUNNING: &str = "uzumaki isn't running (start it with: uzu start)";

#[derive(Clone, Copy)]
pub struct Term {
    pub width: usize,
    pub color: bool,
}

impl Term {
    /// The real terminal: colour on a TTY unless `NO_COLOR`; width from the ioctl, `$COLUMNS`, or 80.
    pub fn detect() -> Self {
        let color = unsafe { libc::isatty(1) } == 1 && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let from_ioctl = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0;
        let width = if from_ioctl {
            ws.ws_col as usize
        } else {
            std::env::var("COLUMNS").ok().and_then(|c| c.trim().parse().ok()).filter(|w| *w > 0).unwrap_or(80)
        };
        Term { width, color }
    }
}

pub struct Timing {
    /// Between `Status` polls while waiting for a song change.
    pub poll: Duration,
    /// Longest wait for a song change.
    pub wait: Duration,
    /// Between redraws of `uzu watch`.
    pub watch: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { poll: Duration::from_millis(100), wait: Duration::from_secs(2), watch: Duration::from_millis(250) }
    }
}

pub struct Env<'a> {
    pub term: Term,
    pub timing: Timing,
    /// Set by the SIGINT handler; ends `uzu watch`.
    pub stop: &'a AtomicBool,
    /// Where to record the last numbered list (`None`: don't).
    pub last: Option<PathBuf>,
}

impl Env<'_> {
    fn record(&self, last: Last) {
        if let Some(path) = &self.last {
            last::save(path, &last);
        }
    }
}

/// A failed command: what to print on stderr (verbatim; empty prints nothing) and the exit code.
#[derive(Debug)]
pub struct Fail {
    pub code: i32,
    pub msg: String,
}

impl Fail {
    pub fn err(msg: impl std::fmt::Display) -> Self {
        Fail { code: 1, msg: format!("uzu: {msg}") }
    }
    pub fn verbatim(msg: &str) -> Self {
        Fail { code: 1, msg: msg.to_string() }
    }
    pub fn usage(msg: impl std::fmt::Display) -> Self {
        Fail { code: 2, msg: format!("uzu: {msg}\nTry 'uzu help'.") }
    }
}

// ------------------------------------------------------------------------------------ backends

pub trait Backend {
    fn request(&mut self, req: &Request) -> io::Result<Reply>;
    /// Answering from the disk cache because the app isn't running.
    fn offline(&self) -> bool {
        false
    }
}

impl Backend for Client {
    fn request(&mut self, req: &Request) -> io::Result<Reply> {
        Client::request(self, req)
    }
}

/// Answers the library commands from the app's cache directory.
pub struct Offline {
    pub dir: PathBuf,
    /// The app's settings file, for its pinned playlists (`cache::default_settings`).
    pub settings: Option<PathBuf>,
}

impl Backend for Offline {
    fn request(&mut self, req: &Request) -> io::Result<Reply> {
        let reply = match req {
            Request::Playlists => cache::collections(&self.dir, Kind::Playlist).map(|mut items| {
                let pins = self.settings.as_deref().map(cache::pinned_playlists).unwrap_or_default();
                cache::apply_pins(&mut items, &pins);
                Reply::Collections { items }
            }),
            Request::Albums => cache::collections(&self.dir, Kind::Album).map(|items| Reply::Collections { items }),
            Request::Tracks { kind, id } => cache::tracks(&self.dir, *kind, id).map(|(collection, songs)| Reply::Tracks { collection, songs }),
            _ => Err(NOT_RUNNING.to_string()),
        };
        Ok(reply.unwrap_or_else(|message| Reply::Error { message }))
    }

    fn offline(&self) -> bool {
        true
    }
}

// -------------------------------------------------------------------------------------- helpers

fn put(out: &mut dyn Write, line: &str) -> Result<(), Fail> {
    // A closed pipe (`uzu queue | head`) is not worth a message.
    writeln!(out, "{line}").map_err(|_| Fail { code: 1, msg: String::new() })
}

fn put_all(out: &mut dyn Write, lines: &[String]) -> Result<(), Fail> {
    lines.iter().try_for_each(|l| put(out, l))
}

fn call(be: &mut dyn Backend, req: Request) -> Result<Reply, Fail> {
    match be.request(&req) {
        Ok(Reply::Error { message }) => Err(Fail::err(message)),
        Ok(reply) => Ok(reply),
        Err(e) => Err(Fail::err(format!("lost the connection to Uzumaki ({e})"))),
    }
}

fn unexpected() -> Fail {
    Fail::err("unexpected reply from Uzumaki (is it a different version?)")
}

fn status(be: &mut dyn Backend) -> Result<Status, Fail> {
    match call(be, Request::Status)? {
        Reply::Status(s) => Ok(s),
        _ => Err(unexpected()),
    }
}

fn collections(be: &mut dyn Backend, kind: Kind) -> Result<Vec<CollectionInfo>, Fail> {
    match call(be, if kind == Kind::Album { Request::Albums } else { Request::Playlists })? {
        // The app sends pinned playlists first already; this keeps the printed numbers right
        // whatever order a reply comes in.
        Reply::Collections { mut items } => {
            uzumaki_ctl::pinned_first(&mut items);
            Ok(items)
        }
        _ => Err(unexpected()),
    }
}

fn noun(kind: Kind) -> &'static str {
    if kind == Kind::Album { "album" } else { "playlist" }
}

fn print_status(out: &mut dyn Write, env: &Env, st: &Status) -> Result<(), Fail> {
    put_all(out, &fmt::status_block(st, env.term.width, env.term.color))
}

/// Id of the song to mark with `▶`; the status is a nicety, so failures just mean no mark.
fn playing_id(be: &mut dyn Backend) -> Option<String> {
    let st = status(be).ok()?;
    if st.state == PlayState::Stopped { None } else { st.song.map(|s| s.id) }
}

fn offline_note(be: &dyn Backend, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    if be.offline() {
        put(out, &render(&[dim("(Uzumaki isn't running — from the cache)")], env.term.width, env.term.color))?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------- playback commands

/// Sends `req`, optionally waits for the song to change (MusicKit switches asynchronously), and
/// prints the status unless `quiet`.
fn act(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, req: Request, wait: bool, quiet: bool) -> Result<(), Fail> {
    let before = if wait { Some(status(be)?) } else { None };
    let mut st = match call(be, req.clone())? {
        Reply::Status(s) => s,
        _ => return Err(unexpected()),
    };
    if let Some(before) = before {
        // `prev` after 3 s only restarts the song: nothing to wait for.
        let restarts = req == Request::Prev && before.position > 3.0;
        if !restarts {
            st = settle(be, env, st, before.song.as_ref().map(|s| s.id.as_str()))?;
        }
    }
    if quiet { Ok(()) } else { print_status(out, env, &st) }
}

/// Polls until the song is no longer `before_id`: for at least `timing.wait`, and for at least as
/// many polls as that holds at the `timing.poll` pace, so a stalled machine still gets its polls
/// in (a stall used to use up the whole wait before the app was asked twice).
fn settle(be: &mut dyn Backend, env: &Env, mut st: Status, before_id: Option<&str>) -> Result<Status, Fail> {
    let started = Instant::now();
    let min_polls = env.timing.wait.as_nanos() / env.timing.poll.as_nanos().max(1);
    let mut polls = 0;
    while st.song.as_ref().map(|s| s.id.as_str()) == before_id && (started.elapsed() < env.timing.wait || polls < min_polls) {
        std::thread::sleep(env.timing.poll);
        st = status(be)?;
        polls += 1;
    }
    Ok(st)
}

fn volume(be: &mut dyn Backend, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    let st = status(be)?;
    put(out, &render(&fmt::volume_spans(&st)[..2], env.term.width, env.term.color))
}

/// How long `uzu airplay <name>` waits for the speaker to play.
const AIRPLAY_WAIT: Duration = Duration::from_secs(20);

/// `uzu airplay`: the speakers (the app looks for them for a moment), `off`, or a speaker by
/// name; the last waits until it plays, or says that it didn't.
fn airplay(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, to: uzumaki_ctl::AirPlayTo, quiet: bool) -> Result<(), Fail> {
    use uzumaki_ctl::AirPlayTo;
    let line = |now: &Option<uzumaki_ctl::AirPlayNow>| match now {
        Some(n) if n.connecting => format!("AirPlay: connecting to {}", n.name),
        Some(n) => format!("AirPlay: playing on {}", n.name),
        None => "AirPlay: off (playing on this computer)".to_string(),
    };
    match to {
        AirPlayTo::List => {
            let Reply::AirPlay { current, devices } = call(be, Request::AirPlay { to: AirPlayTo::List })? else { return Err(unexpected()) };
            put(out, &line(&current))?;
            if devices.is_empty() {
                return put(out, "No AirPlay speakers found on this network.");
            }
            let width = devices.iter().map(|d| d.name.chars().count()).max().unwrap_or(0);
            for d in &devices {
                let mark = if current.as_ref().is_some_and(|c| c.name.eq_ignore_ascii_case(&d.name)) { "\u{25b6}" } else { " " };
                let kind = if d.pair { format!("{}, stereo pair", d.kind) } else { d.kind.clone() };
                put(out, &format!("{mark} {:<width$}  {kind}", d.name))?;
            }
            Ok(())
        }
        AirPlayTo::Off => {
            let Reply::Status(st) = call(be, Request::AirPlay { to: AirPlayTo::Off })? else { return Err(unexpected()) };
            if quiet { Ok(()) } else { put(out, &line(&st.airplay)) }
        }
        AirPlayTo::Name(name) => {
            let Reply::Status(mut st) = call(be, Request::AirPlay { to: AirPlayTo::Name(name.clone()) })? else { return Err(unexpected()) };
            let started = Instant::now();
            while st.airplay.as_ref().is_some_and(|a| a.connecting) && started.elapsed() < AIRPLAY_WAIT {
                std::thread::sleep(env.timing.poll);
                st = status(be)?;
            }
            match &st.airplay {
                Some(a) if !a.connecting => {
                    if quiet { Ok(()) } else { put(out, &line(&st.airplay)) }
                }
                Some(_) => Err(Fail::err(format!("still connecting to '{name}' (see: uzu airplay)"))),
                None => Err(Fail::err(format!("couldn't play on '{name}': the sound stays on this computer (speakers: uzu airplay)"))),
            }
        }
    }
}

// ------------------------------------------------------------------------------- library commands

fn find(be: &mut dyn Backend, env: &Env, kind: Kind, query: &str) -> Result<CollectionInfo, Fail> {
    let items = collections(be, kind)?;
    match args::resolve(&items, query) {
        Found::One(i) => Ok(items[i].clone()),
        Found::None => Err(Fail::err(format!("no {} matches '{query}' (see: uzu {}s)", noun(kind), noun(kind)))),
        Found::Many(hits) => {
            let shown: Vec<_> = hits.iter().map(|&i| (i + 1, &items[i])).collect();
            let lines = list::rows(&list::collection_rows(&shown), env.term.width, false);
            Err(Fail::err(format!("'{query}' matches several {}s, pick one by number:\n{}", noun(kind), lines.join("\n"))))
        }
    }
}

fn list_collections(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, kind: Kind, filter: &str) -> Result<(), Fail> {
    let items = collections(be, kind)?;
    env.record(Last::Collections { kind, ids: items.iter().map(|c| c.id.clone()).collect() });
    offline_note(be, env, out)?;
    let shown: Vec<_> = args::filter(&items, filter).into_iter().map(|i| (i + 1, &items[i])).collect();
    if shown.is_empty() {
        let what = if filter.is_empty() { format!("no {}s", noun(kind)) } else { format!("no {}s match '{filter}'", noun(kind)) };
        return put(out, &render(&[dim(what)], env.term.width, env.term.color));
    }
    put_all(out, &list::rows(&list::collection_rows(&shown), env.term.width, env.term.color))
}

fn show_tracks(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, kind: Kind, query: &str) -> Result<(), Fail> {
    let found = find(be, env, kind, query)?;
    let (collection, songs) = match call(be, Request::Tracks { kind, id: found.id.clone() })? {
        Reply::Tracks { collection, songs } => (collection, songs),
        _ => return Err(unexpected()),
    };
    env.record(Last::Songs { kind, id: found.id });
    let playing = playing_id(be);
    offline_note(be, env, out)?;
    let Term { width, color } = env.term;
    put(out, &list::tracks_header(&collection, &songs, width, color))?;
    if songs.is_empty() {
        return put(out, &render(&[dim("(no songs)")], width, color));
    }
    put_all(out, &list::rows(&list::song_rows(&songs, playing.as_deref()), width, color))
}

const QUEUE_SHOWN: usize = 20;

fn now_playing_line(song: &SongInfo, st: Option<&Status>, term: Term) -> String {
    let mut spans = vec![plain("  ")];
    spans.extend(fmt::song_line(song));
    let time = match st {
        Some(st) if st.state != PlayState::Stopped && st.duration > 0.0 => format!("{}/{}", fmt::fmt_time(st.position), fmt::fmt_time(st.duration)),
        _ if song.duration > 0.0 => fmt::fmt_time(song.duration),
        _ => String::new(),
    };
    if !time.is_empty() {
        spans.push(dim(format!("   {time}")));
    }
    render(&spans, term.width, term.color)
}

/// Section heading lines and numbered rows for `uzu queue`.
pub(crate) fn queue_lines(q: &Queue, st: Option<&Status>, all: bool, term: Term) -> Vec<String> {
    let Term { width, color } = term;
    let mut lines = Vec::new();
    if !q.history.is_empty() {
        lines.push(render(&[bold("◷ History (replay: uzu history <n>)")], width, color));
        lines.extend(list::rows(&list::song_rows(&q.history, None), width, color));
    }
    lines.push(render(&[bold("Now playing")], width, color));
    match &q.current {
        Some(song) => lines.push(now_playing_line(song, st, term)),
        None => lines.push(render(&[dim("  nothing")], width, color)),
    }
    let limit = if all { q.up_next.len() } else { QUEUE_SHOWN };
    let shown = &q.up_next[..q.up_next.len().min(limit)];
    let rows = list::rows(&list::song_rows(shown, None), width, color);
    let queued = q.queued.min(shown.len());
    if queued > 0 {
        lines.push(String::new());
        lines.push(render(&[bold("Queued")], width, color));
        lines.extend(rows[..queued].iter().cloned());
    }
    if queued < shown.len() {
        lines.push(String::new());
        let heading = if q.source.is_empty() { "Continue playing".to_string() } else { format!("Continue playing from {}", q.source) };
        lines.push(render(&[bold(heading)], width, color));
        lines.extend(rows[queued..].iter().cloned());
    }
    if let Some(name) = &q.station {
        // Apple picks a station's songs as it goes: nothing to list. (The queue screen shows
        // the songs it already played.)
        lines.push(String::new());
        lines.push(render(&[dim(format!("{name} \u{2014} songs are picked as you listen"))], width, color));
    } else if shown.is_empty() {
        lines.push(String::new());
        lines.push(render(&[dim("Nothing up next")], width, color));
    }
    if q.up_next.len() > shown.len() {
        lines.push(render(&[dim(format!("… and {} more (uzu queue -a)", q.up_next.len() - shown.len()))], width, color));
    }
    lines
}

fn fetch_queue(be: &mut dyn Backend) -> Result<Queue, Fail> {
    match call(be, Request::Queue)? {
        Reply::Queue(q) => Ok(q),
        _ => Err(unexpected()),
    }
}

fn show_queue(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, all: bool) -> Result<(), Fail> {
    let q = fetch_queue(be)?;
    print_queue(be, env, out, &q, all)
}

/// Prints the queue as `uzu queue` does and records the numbers shown.
fn print_queue(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, q: &Queue, all: bool) -> Result<(), Fail> {
    let shown = if all { q.up_next.len() } else { q.up_next.len().min(QUEUE_SHOWN) };
    env.record(Last::Queue { ids: q.up_next[..shown].iter().map(|s| s.id.clone()).collect() });
    let st = status(be).ok();
    put_all(out, &queue_lines(q, st.as_ref(), all, env.term))
}

/// `jump`, `move` and `del` change the queue; the answer is the new queue (unless `quiet`).
fn change_queue(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, req: Request, quiet: bool) -> Result<(), Fail> {
    match call(be, req)? {
        Reply::Queue(_) if quiet => Ok(()),
        Reply::Queue(q) => print_queue(be, env, out, &q, false),
        _ => Err(unexpected()),
    }
}

/// `jump <title>`: the first up-next song whose title matches (exact, prefix, substring).
fn jump_title(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, words: &str, quiet: bool) -> Result<(), Fail> {
    let q = fetch_queue(be)?;
    let titles: Vec<&str> = q.up_next.iter().map(|s| s.title.as_str()).collect();
    match args::first_title(&titles, words) {
        Some(index) => act(be, env, out, Request::Jump { index }, true, quiet),
        None => Err(Fail::err(format!("no song in Up Next matches '{words}' (see: uzu queue -a)"))),
    }
}

/// `play <n>`: entry n of the list printed last; with no record, playlist n.
fn play_number(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, i: usize, quiet: bool) -> Result<(), Fail> {
    let last = env.last.as_deref().and_then(last::load);
    let too_far = |len: usize, what: &str| Fail::err(format!("the last list has only {len} {what}"));
    match last {
        None => {
            let c = find(be, env, Kind::Playlist, &(i + 1).to_string())?;
            act(be, env, out, Request::PlayCollection { kind: Kind::Playlist, id: c.id, index: None, order: None }, true, quiet)
        }
        Some(Last::Collections { kind, ids }) => match ids.get(i) {
            Some(id) => act(be, env, out, Request::PlayCollection { kind, id: id.clone(), index: None, order: None }, true, quiet),
            None => Err(too_far(ids.len(), &format!("{}s", noun(kind)))),
        },
        Some(Last::Songs { kind, id }) => act(be, env, out, Request::PlayCollection { kind, id, index: Some(i), order: None }, true, quiet),
        Some(Last::Search) => act(be, env, out, Request::Pick { indices: vec![i], action: PickAction::Play }, true, quiet),
        Some(Last::Queue { ids }) => {
            let Some(id) = ids.get(i) else { return Err(too_far(ids.len(), "songs")) };
            // The same song may appear twice: take the same occurrence as printed.
            let nth = ids[..i].iter().filter(|x| *x == id).count();
            let q = fetch_queue(be)?;
            let index = q.up_next.iter().enumerate().filter(|(_, s)| &s.id == id).nth(nth).map(|(k, _)| k);
            match index {
                Some(index) => act(be, env, out, Request::Jump { index }, true, quiet),
                None => Err(Fail::err("that song is no longer in the queue")),
            }
        }
    }
}

fn search(be: &mut dyn Backend, env: &Env, out: &mut dyn Write, query: &str) -> Result<(), Fail> {
    let songs = match call(be, Request::Search { query: query.to_string() })? {
        Reply::Songs { songs } => songs,
        _ => return Err(unexpected()),
    };
    env.record(Last::Search);
    let Term { width, color } = env.term;
    if songs.is_empty() {
        return put(out, &render(&[dim(format!("no songs found for '{query}'"))], width, color));
    }
    put_all(out, &list::rows(&list::song_rows(&songs, None), width, color))?;
    put(out, &render(&[dim("uzu pick <n>  ·  uzu insert <n>  ·  uzu add <n>")], width, color))
}

// ------------------------------------------------------------------------------------------ watch

fn watch(be: &mut dyn Backend, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    const HIDE: &str = "\x1b[?25l";
    const SHOW: &str = "\x1b[?25h";
    let _ = write!(out, "{HIDE}");
    let result = watch_loop(be, env, out);
    let _ = write!(out, "{SHOW}");
    let _ = out.flush();
    result
}

fn watch_loop(be: &mut dyn Backend, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    let mut drawn = 0;
    while !env.stop.load(Ordering::Relaxed) {
        let st = match be.request(&Request::Status) {
            Ok(Reply::Status(st)) => st,
            Ok(Reply::Error { message }) => return Err(Fail::err(message)),
            Ok(_) => return Err(unexpected()),
            Err(_) => return Err(Fail::err("Uzumaki went away")),
        };
        let lines = fmt::status_block(&st, env.term.width, env.term.color);
        write!(out, "{}", fmt::watch_frame(drawn, &lines)).and_then(|_| out.flush()).map_err(|_| Fail { code: 1, msg: String::new() })?;
        drawn = lines.len();
        sleep_unless_stopped(env);
    }
    Ok(())
}

fn sleep_unless_stopped(env: &Env) {
    let until = Instant::now() + env.timing.watch;
    while Instant::now() < until && !env.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(20).min(env.timing.watch));
    }
}

// --------------------------------------------------------------------------------------- dispatch

pub fn run(inv: &Invocation, be: &mut dyn Backend, env: &Env, out: &mut dyn Write) -> Result<(), Fail> {
    let q = inv.quiet;
    match &inv.cmd {
        Command::Status => {
            let st = status(be)?;
            print_status(out, env, &st)
        }
        Command::Play => act(be, env, out, Request::Play, false, q),
        Command::PlayNumber(i) => play_number(be, env, out, *i, q),
        Command::PlayCollection { kind, words, song } => {
            let c = find(be, env, *kind, &words.join(" "))?;
            act(be, env, out, Request::PlayCollection { kind: *kind, id: c.id, index: *song, order: None }, true, q)
        }
        Command::Pause => act(be, env, out, Request::Pause, false, q),
        Command::Toggle => act(be, env, out, Request::Toggle, false, q),
        Command::Next => act(be, env, out, Request::Next, true, q),
        Command::Prev => act(be, env, out, Request::Prev, true, q),
        Command::Seek(to) => act(be, env, out, Request::Seek { to: *to }, false, q),
        Command::Vol(Some(to)) => act(be, env, out, Request::Volume { to: *to }, false, q),
        Command::Vol(None) => volume(be, env, out),
        Command::AirPlay(to) => airplay(be, env, out, to.clone(), q),
        Command::Shuffle(on) => act(be, env, out, Request::Shuffle { on: *on }, false, q),
        Command::Repeat(mode) => act(be, env, out, Request::Repeat { mode: *mode }, false, q),
        Command::Playlists => list_collections(be, env, out, Kind::Playlist, ""),
        Command::Albums(filter) => list_collections(be, env, out, Kind::Album, filter),
        Command::Playlist(query) => show_tracks(be, env, out, Kind::Playlist, query),
        Command::Album(query) => show_tracks(be, env, out, Kind::Album, query),
        Command::History(index) => act(be, env, out, Request::History { index: *index }, true, q),
        Command::Queue { all } => show_queue(be, env, out, *all),
        Command::Jump(index) => act(be, env, out, Request::Jump { index: *index }, true, q),
        Command::JumpTitle(words) => jump_title(be, env, out, words, q),
        Command::Move { from, to } => change_queue(be, env, out, Request::Move { from: *from, to: *to }, q),
        Command::Remove(indices) => change_queue(be, env, out, Request::Remove { indices: indices.clone() }, q),
        Command::Search(query) => search(be, env, out, query),
        Command::Pick { indices, action } => act(be, env, out, Request::Pick { indices: indices.clone(), action: *action }, *action == PickAction::Play, q),
        Command::Watch => watch(be, env, out),
        Command::Show => call(be, Request::Show).map(|_| ()),
        Command::Start | Command::Quit | Command::Restart | Command::Help | Command::Version | Command::Screen | Command::CoverInfo => Ok(()), // handled by main
    }
}
