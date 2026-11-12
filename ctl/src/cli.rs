//! The command line -> a [`Command`]. Usage mistakes come back as `Err(message)` (exit 2).

use uzumaki_ctl::{Kind, PickAction, RepeatMode, SeekTo, VolumeTo};

use crate::args::{parse_seek, parse_song_number, parse_volume, split_song_number};

#[derive(Debug, PartialEq)]
pub enum Command {
    Status,
    Play,
    /// A bare `play <n>`: entry n (0-based) of the list `uzu` printed last.
    PlayNumber(usize),
    /// A playlist or album by number or name, from `song` (0-based).
    PlayCollection { kind: Kind, words: Vec<String>, song: Option<usize> },
    Pause,
    Toggle,
    Next,
    Prev,
    Seek(SeekTo),
    Vol(Option<VolumeTo>),
    Shuffle(Option<bool>),
    Repeat(Option<RepeatMode>),
    Playlists,
    Playlist(String),
    Albums(String),
    Album(String),
    Queue { all: bool },
    /// 0-based index into up next.
    Jump(usize),
    History(usize),
    /// Up-next song by title words.
    JumpTitle(String),
    /// 0-based indexes into up next.
    Move { from: usize, to: usize },
    Remove(Vec<usize>),
    Search(String),
    Pick { indices: Vec<usize>, action: PickAction },
    Watch,
    /// Plain `uzu` in a terminal: the interactive queue screen (set by `main`, never parsed).
    Screen,
    Show,
    Start,
    Quit,
    Restart,
    /// How the screens' cover is detected in this terminal (no app needed).
    CoverInfo,
    Help,
    /// `uzu --version`: the version, no app needed.
    Version,
}

#[derive(Debug, PartialEq)]
pub struct Invocation {
    pub cmd: Command,
    pub quiet: bool,
}

fn none(name: &str, params: &[String]) -> Result<(), String> {
    match params.first() {
        Some(p) => Err(format!("'{name}' takes no arguments (got '{p}')")),
        None => Ok(()),
    }
}

fn one<'a>(name: &str, params: &'a [String], what: &str) -> Result<&'a str, String> {
    match params {
        [p] => Ok(p),
        [] => Err(format!("'{name}' needs {what}")),
        _ => Err(format!("'{name}' takes just one argument")),
    }
}

fn target(name: &str, params: &[String], what: &str) -> Result<String, String> {
    if params.is_empty() { Err(format!("'{name}' needs {what}")) } else { Ok(params.join(" ")) }
}

fn song_numbers(name: &str, params: &[String]) -> Result<Vec<usize>, String> {
    if params.is_empty() {
        return Err(format!("'{name}' needs result numbers from 'uzu search'"));
    }
    params.iter().map(|p| parse_song_number(p)).collect()
}

fn play(params: &[String]) -> Result<Command, String> {
    let (kind, words, explicit) = match params.split_first() {
        Some((a, rest)) if a == "-a" || a == "album" => (Kind::Album, rest, true),
        Some((a, rest)) if a == "-p" || a == "playlist" => (Kind::Playlist, rest, true),
        _ => (Kind::Playlist, params, false),
    };
    if words.is_empty() {
        return match (explicit, params.first()) {
            (false, _) => Ok(Command::Play),
            (true, Some(a)) => Err(format!("'play {a}' needs {} (number or name)", if kind == Kind::Album { "an album" } else { "a playlist" })),
            (true, None) => unreachable!(),
        };
    }
    // One bare number follows the list printed last; `play 3 5` and `play mix 5` don't.
    if let ([n], false) = (words, explicit) {
        if n.bytes().all(|b| b.is_ascii_digit()) {
            return Ok(Command::PlayNumber(parse_song_number(n)?));
        }
    }
    let (words, song) = split_song_number(words)?;
    Ok(Command::PlayCollection { kind, words, song })
}

/// `move 3 1`, `del 2 4`: 1-based up-next numbers -> 0-based.
fn up_next_numbers(name: &str, params: &[String], want: Option<usize>) -> Result<Vec<usize>, String> {
    match want {
        Some(n) if params.len() != n => return Err(format!("'{name}' needs {n} up-next numbers from 'uzu queue'")),
        None if params.is_empty() => return Err(format!("'{name}' needs up-next numbers from 'uzu queue'")),
        _ => {}
    }
    params.iter().map(|p| parse_song_number(p)).collect()
}

pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let quiet = args.iter().any(|a| a == "-q" || a == "--quiet");
    let rest: Vec<String> = args.iter().filter(|a| *a != "-q" && *a != "--quiet").cloned().collect();
    let Some((name, p)) = rest.split_first() else { return Ok(Invocation { cmd: Command::Status, quiet }) };
    let name = name.as_str();
    let cmd = match name {
        "status" => none(name, p).map(|_| Command::Status)?,
        "play" => play(p)?,
        "pause" | "stop" => none(name, p).map(|_| Command::Pause)?,
        // `toggle` alone is play/pause; `toggle shuffle` / `toggle repeat` are what people try next.
        "toggle" => match p {
            [] => Command::Toggle,
            [what] if what == "shuffle" => Command::Shuffle(None),
            [what] if what == "repeat" => Command::Repeat(None),
            [what, ..] => return Err(format!("'toggle' is play/pause; it can also take 'shuffle' or 'repeat' (got '{what}')")),
        },
        "next" => none(name, p).map(|_| Command::Next)?,
        "prev" => none(name, p).map(|_| Command::Prev)?,
        "seek" => Command::Seek(parse_seek(one(name, p, "a position (30, 1:30, +10, -10, 50%)")?)?),
        "vol" | "volume" => Command::Vol(if p.is_empty() { None } else { Some(parse_volume(one(name, p, "a level")?)?) }),
        "shuffle" => Command::Shuffle(match p {
            [] => None,
            [a] if a == "on" => Some(true),
            [a] if a == "off" => Some(false),
            _ => return Err("'shuffle' takes on or off".into()),
        }),
        "repeat" => Command::Repeat(match p {
            [] => None,
            [a] if a == "off" => Some(RepeatMode::Off),
            [a] if a == "all" => Some(RepeatMode::All),
            [a] if a == "one" => Some(RepeatMode::One),
            _ => return Err("'repeat' takes off, all or one".into()),
        }),
        "playlists" => none(name, p).map(|_| Command::Playlists)?,
        "playlist" => Command::Playlist(target(name, p, "a playlist (number or name)")?),
        "albums" => Command::Albums(p.join(" ")),
        "album" => Command::Album(target(name, p, "an album (number or name)")?),
        "queue" => match p {
            [] => Command::Queue { all: false },
            [a] if a == "-a" => Command::Queue { all: true },
            _ => return Err("'queue' takes only -a".into()),
        },
        "history" => Command::History(parse_song_number(one(name, p, "a history number from uzu queue")?)?),
        "jump" => match p {
            [] => return Err(format!("'{name}' needs an up-next number from 'uzu queue', or part of a title")),
            [n] if n.bytes().all(|b| b.is_ascii_digit()) => Command::Jump(parse_song_number(n)?),
            _ => Command::JumpTitle(p.join(" ")),
        },
        "move" | "mv" => match up_next_numbers(name, p, Some(2))?[..] {
            [from, to] => Command::Move { from, to },
            _ => unreachable!(),
        },
        "del" | "rm" => Command::Remove(up_next_numbers(name, p, None)?),
        "search" => Command::Search(target(name, p, "some words to search for")?),
        "pick" => Command::Pick { indices: song_numbers(name, p)?, action: PickAction::Play },
        "insert" => Command::Pick { indices: song_numbers(name, p)?, action: PickAction::Next },
        "add" => Command::Pick { indices: song_numbers(name, p)?, action: PickAction::Add },
        "watch" => none(name, p).map(|_| Command::Watch)?,
        "show" => none(name, p).map(|_| Command::Show)?,
        "start" => none(name, p).map(|_| Command::Start)?,
        "quit" => none(name, p).map(|_| Command::Quit)?,
        "restart" => none(name, p).map(|_| Command::Restart)?,
        "cover-info" => none(name, p).map(|_| Command::CoverInfo)?,
        "help" | "-h" | "--help" => Command::Help,
        "version" | "-V" | "--version" => none(name, p).map(|_| Command::Version)?,
        other => return Err(format!("unknown command '{other}'")),
    };
    Ok(Invocation { cmd, quiet })
}

pub const HELP: &str = "\
usage: uzu [-q] [command]

Plain 'uzu' in a terminal opens the interactive screens: 1 Queue, 2 Playlists, 3 Search, 4 Albums,
5 Lyrics ('?' lists the keys). Lyrics: the current line in the middle, sung words in coral; w
switches word by word / line by line, j/k scroll by hand, g follows the song again. Needs a
recent Uzumaki (an older one answers 'update Uzumaki for lyrics'). The player shows the song's cover with the terminal's graphics (kitty,
Ghostty, WezTerm, iTerm2, sixel), else in coloured blocks; 'i' hides or shows it.
UZUMAKI_COVER=off starts it hidden; halfblocks, kitty, sixel or iterm2 forces how it is drawn.
F cycles the queue screen's views: normal, showcase (a big cover beside Up Next) and solo
(only the cover and the song, centred); Esc goes back. In solo, l shows the lyrics (beside
the cover whenever there is room (about 70 columns), under it in a narrower one; w words/lines; unsynced ones sit still, j/k scroll). UZUMAKI_VIEW=showcase|solo starts in one.
'uzu cover-info' shows what the terminal answered and which way was picked.
In a pipe it prints the status. If Uzumaki isn't running, uzu starts it hidden (to the tray).

  status                      what plays now (the default when piped)
  toggle                      play/pause (also: play, pause)
  next                        next song
  prev                        previous song (restarts it after 3 s)
  play <n>                    play entry n of the list uzu printed last (a search
                              result, a playlist, an album, a song of one, or the queue)
  play <playlist> [n]         play a playlist from song n (a random one with shuffle on)
  play -p|-a <x> [n]          force a playlist (also: play playlist <x>) or an album (play album <x>)
  seek <pos>                  30  1:30  +10  -10  +1:00  50%
  vol [<level>]               60  +5  -5
  shuffle [on|off]            no argument: turn it on/off (also: toggle shuffle)
  repeat [off|all|one]        no argument: off -> all -> one (also: toggle repeat)
  playlists                   playlist <playlist>
  albums [<filter>]           album <album>
  history <n>                 replay a History entry (oldest first)
  queue [-a]                  jump <n|title>  move <from> <to>  del <n...>  (n: numbers from queue)
  search <words>              pick <n...>  insert <n...>  add <n...>
  watch                       the interactive screen in a terminal (same as plain uzu); piped: live status
  show                        show the window
  start                       start Uzumaki in the background (as a systemd user service when it can)
  quit                        quit Uzumaki (does nothing if it isn't running)
  restart                     quit and start again (the queue starts empty)
  cover-info                  how the cover is drawn in this terminal (for bug reports)
  --version                   uzu's version

<playlist> and <album> are a number from the list, or part of the name.
A lone number in 'play <n>' follows the last list printed by playlists, albums, playlist,
album, search or queue (recorded next to the control socket, as <socket>.last).
-q: don't print the status after a command.";

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Command, String> {
        let args: Vec<String> = s.split_whitespace().map(String::from).collect();
        parse(&args).map(|i| i.cmd)
    }

    #[test]
    fn commands() {
        assert_eq!(p(""), Ok(Command::Status));
        assert_eq!(p("stop"), Ok(Command::Pause));
        assert_eq!(p("play"), Ok(Command::Play));
        assert_eq!(p("play 3"), Ok(Command::PlayNumber(2)));
        assert_eq!(p("play 3 5"), Ok(Command::PlayCollection { kind: Kind::Playlist, words: vec!["3".into()], song: Some(4) }));
        assert_eq!(p("play mix 5"), Ok(Command::PlayCollection { kind: Kind::Playlist, words: vec!["mix".into()], song: Some(4) }));
        assert_eq!(p("play mix"), Ok(Command::PlayCollection { kind: Kind::Playlist, words: vec!["mix".into()], song: None }));
        assert_eq!(p("play -p 3"), Ok(Command::PlayCollection { kind: Kind::Playlist, words: vec!["3".into()], song: None }));
        assert_eq!(p("play playlist 3 2"), Ok(Command::PlayCollection { kind: Kind::Playlist, words: vec!["3".into()], song: Some(1) }));
        assert_eq!(p("play album 3"), Ok(Command::PlayCollection { kind: Kind::Album, words: vec!["3".into()], song: None }));
        assert_eq!(p("jump numb"), Ok(Command::JumpTitle("numb".into())));
        assert_eq!(p("jump instant crush"), Ok(Command::JumpTitle("instant crush".into())));
        assert_eq!(p("move 3 1"), Ok(Command::Move { from: 2, to: 0 }));
        assert_eq!(p("del 2 4 2"), Ok(Command::Remove(vec![1, 3, 1])));
        assert_eq!(p("rm 1"), Ok(Command::Remove(vec![0])));
        assert_eq!(p("play my road trip 5"), Ok(Command::PlayCollection { kind: Kind::Playlist, words: vec!["my".into(), "road".into(), "trip".into()], song: Some(4) }));
        assert_eq!(p("play -a 532"), Ok(Command::PlayCollection { kind: Kind::Album, words: vec!["532".into()], song: None }));
        assert_eq!(p("vol -5"), Ok(Command::Vol(Some(VolumeTo::Change(-0.05)))));
        assert_eq!(p("jump 1"), Ok(Command::Jump(0)));
        assert_eq!(p("add 2 3"), Ok(Command::Pick { indices: vec![1, 2], action: PickAction::Add }));
        assert_eq!(p("shuffle"), Ok(Command::Shuffle(None)));
        assert_eq!(p("toggle"), Ok(Command::Toggle));
        assert_eq!(p("toggle shuffle"), Ok(Command::Shuffle(None)));
        assert_eq!(p("toggle repeat"), Ok(Command::Repeat(None)));
        assert!(p("toggle volume").is_err());
        assert_eq!(p("repeat one"), Ok(Command::Repeat(Some(RepeatMode::One))));
        assert_eq!(p("albums"), Ok(Command::Albums(String::new())));
        assert_eq!(p("quit"), Ok(Command::Quit));
        assert_eq!(p("restart"), Ok(Command::Restart));
        assert!(p("quit now").is_err() && p("restart now").is_err());
        assert_eq!(p("--version"), Ok(Command::Version));
        assert_eq!(p("-V"), Ok(Command::Version));
    }

    #[test]
    fn usage_errors() {
        for bad in ["frobnicate", "seek", "seek x", "play -a", "play -p", "play album", "play x 0", "jump", "move", "move 1", "move 1 2 3", "move a 1", "move 0 1", "del", "del x", "del 0", "jump 0", "pick", "pick a", "shuffle maybe", "repeat twice", "next now", "queue -b"] {
            assert!(p(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn quiet_flag_anywhere() {
        let args = vec!["-q".to_string(), "next".to_string()];
        assert_eq!(parse(&args), Ok(Invocation { cmd: Command::Next, quiet: true }));
        let args = vec!["pause".to_string(), "--quiet".to_string()];
        assert!(parse(&args).unwrap().quiet);
    }
}
