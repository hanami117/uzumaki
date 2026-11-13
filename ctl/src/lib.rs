//! The control protocol between a running Uzumaki and `uzu`, its mpc-style command line.
//!
//! The app plays the part of mpd: it owns the one playback bridge, the library and the queue.
//! It listens on a Unix stream socket ([`socket_path`]). `uzu` connects, writes one [`Request`]
//! per line as JSON, and reads one [`Reply`] per line back. A connection may carry any number of
//! requests; they are answered in order.
//!
//! Both sides use this crate, so they can't drift apart: the app calls [`serve`] with a handler,
//! `uzu` uses [`Client`].

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The build number: a whole number that goes up with every package built from the same version
/// (a DMG or AppImage rebuilt without code changes gets the next one). Its one source is the
/// `BUILD` file at the top of the repo; the packaging scripts read it too. 0.1.0 is build 1.
pub const BUILD: u32 = parse_build(include_str!("../../BUILD"));

/// `BUILD`'s number, at compile time: digits, optional spaces and newlines around them, above 0.
/// Anything else stops the build.
const fn parse_build(s: &str) -> u32 {
    let b = s.as_bytes();
    let (mut i, mut n, mut digits) = (0, 0u32, 0);
    while i < b.len() {
        match b[i] {
            b'0'..=b'9' if digits == 0 || i == 0 || b[i - 1].is_ascii_digit() => {
                n = match n.checked_mul(10) {
                    Some(m) => m + (b[i] - b'0') as u32,
                    None => panic!("BUILD is too large"),
                };
                digits += 1;
            }
            b' ' | b'\t' | b'\n' | b'\r' => {}
            _ => panic!("BUILD must hold one whole number"),
        }
        i += 1;
    }
    assert!(digits > 0 && n > 0, "BUILD must hold a number above 0");
    n
}

/// The version and build as `--version` prints them: `0.1.0 (build 1)`.
pub fn version_line() -> String {
    format!("{} (build {BUILD}){}", env!("CARGO_PKG_VERSION"), dev_suffix())
}

/// What `build.rs` found in git: the branch, short hash, whether the tree has changes, and whether
/// HEAD is exactly the release tag for this version and build.
#[derive(Debug, PartialEq)]
pub struct GitFacts<'a> {
    pub branch: &'a str,
    pub hash: &'a str,
    pub dirty: bool,
    pub at_release_tag: bool,
}

/// The text a dev build adds after the version, for display only (never for update checks, the
/// User-Agent or any comparison). Empty without git facts (tarball, AUR source build) and for a
/// clean checkout of the release tag. Otherwise ` · main 20c1fe9`, with ` dirty` if the tree has
/// changes.
pub fn git_suffix(facts: Option<GitFacts>) -> String {
    match facts {
        Some(f) if !(f.at_release_tag && !f.dirty) => {
            format!(" \u{b7} {} {}{}", f.branch, f.hash, if f.dirty { " dirty" } else { "" })
        }
        _ => String::new(),
    }
}

/// `build.rs`'s `UZUMAKI_GIT` (tab separated, or empty), as facts.
fn parse_facts(s: &str) -> Option<GitFacts<'_>> {
    let mut p = s.split('\t');
    let (branch, hash, dirty, tag) = (p.next()?, p.next()?, p.next()?, p.next()?);
    (!branch.is_empty() && !hash.is_empty()).then_some(GitFacts { branch, hash, dirty: dirty == "1", at_release_tag: tag == "1" })
}

/// [`git_suffix`] for this build.
pub fn dev_suffix() -> String {
    git_suffix(parse_facts(env!("UZUMAKI_GIT")))
}

/// `$UZUMAKI_CTL_SOCKET`, else `<runtime dir>/uzumaki.sock` (see [`runtime_dir`]).
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("UZUMAKI_CTL_SOCKET").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    runtime_dir().join("uzumaki.sock")
}

/// Linux: `$XDG_RUNTIME_DIR`, else `/tmp/uzumaki-<uid>`.
#[cfg(not(target_os = "macos"))]
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("uzumaki-{}", uid())))
}

/// macOS: `uzumaki-<uid>` in the per-user temp folder (`/var/folders/…/T/`), asked from the
/// system instead of read from `$TMPDIR`: an ssh login doesn't set `$TMPDIR` (it comes with the
/// GUI login), and the app started from the Dock and `uzu` over ssh must find the same socket.
#[cfg(target_os = "macos")]
fn runtime_dir() -> PathBuf {
    darwin_user_temp_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("uzumaki-{}", uid()))
}

#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; 1024];
    // SAFETY: `buf` is writable for `buf.len()` bytes; confstr writes at most that many,
    // NUL included, and returns the size it needs (NUL included), or 0 on failure.
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    if n <= 1 || n > buf.len() {
        return None;
    }
    buf.truncate(n - 1);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

/// Where `uzu` records the last numbered list it printed: the socket path plus `.last`, so an
/// instance with its own `UZUMAKI_CTL_SOCKET` keeps its own record.
pub fn last_path() -> PathBuf {
    let mut p = socket_path().into_os_string();
    p.push(".last");
    PathBuf::from(p)
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and can't fail.
    unsafe { libc::getuid() }
}

// ------------------------------------------------------------------------------------ requests

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// What plays now. Replies `Status`.
    Status,
    /// Resume. Replies `Status`.
    Play,
    /// Replies `Status`.
    Pause,
    /// Play/pause. Replies `Status`.
    Toggle,
    /// Replies `Status` (the song may change a moment later: MusicKit skips on its own).
    Next,
    /// Restarts the song after 3 s, else the previous one, like the player bar. Replies `Status`.
    Prev,
    /// Replies `Status`.
    Seek { to: SeekTo },
    /// Replies `Status`.
    Volume { to: VolumeTo },
    /// `None` toggles. Replies `Status`.
    Shuffle { on: Option<bool> },
    /// `None` cycles Off → All → One → Off, like the player bar. Replies `Status`.
    Repeat { mode: Option<RepeatMode> },
    /// The library's playlists in sidebar order: those pinned in the app first (`CollectionInfo::pinned`,
    /// newer apps), then Favourite Songs, then the rest.
    /// Replies `Collections`.
    Playlists,
    /// The library's albums, as the app has them. Replies `Collections`.
    Albums,
    /// An album's or playlist's songs: from the app's memory, the disk cache, or Apple (which
    /// can take a few seconds for long playlists). Replies `Tracks`.
    Tracks { kind: Kind, id: String },
    /// Plays an album or playlist from song `index` (0-based; with shuffle on, the rest follows
    /// shuffled). `None`: from a random song when shuffle is on, else from the first playable
    /// one. Replies `Status`.
    ///
    /// `order`: the collection's song indices (as in the `Tracks` reply) in the order to play
    /// them, for a list the user sorted. `index` is then a position within `order`, not a song
    /// number. An index out of range is an error ("this list changed since it was loaded").
    /// Absent (old clients) or `None`: the collection's own order.
    PlayCollection {
        kind: Kind,
        id: String,
        index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<Vec<usize>>,
    },
    /// Now playing and Up Next. Replies `Queue`.
    Queue,
    /// Plays entry `index` (0-based) of `Queue::up_next`, then of `Queue::next_lap`, then of
    /// `Queue::station_played`, now. Replies `Status`.
    Jump { index: usize },
    /// Replay chronological session history entry (0-based). Replies Status.
    History { index: usize },
    /// Moves entry `from` of `Queue::up_next` so it sits at index `to` afterwards (0-based).
    /// Replies `Queue`.
    Move { from: usize, to: usize },
    /// Removes entries of `Queue::up_next` (0-based, any order, repeats ignored). Replies `Queue`.
    Remove { indices: Vec<usize> },
    /// Searches Apple Music's catalog for songs. The app remembers the results for `Pick`.
    /// Replies `Songs`.
    Search { query: String },
    /// Acts on songs of the last `Search` (0-based, in the order given). Replies `Status`.
    Pick { indices: Vec<usize>, action: PickAction },
    /// The lyrics of the song playing now, from the app's memory (the app fetches them on every
    /// song change; only when they are missing does it ask Apple, once, and answers `loading`
    /// until they land). Replies `Lyrics`. Not part of `Status`: that is polled twice a second
    /// and word timing is ~20 KB. An app older than this answers "bad request".
    Lyrics,
    /// Shows the app's window. Replies `Status`.
    Show,
    /// Quits the app (as the tray's Quit does). Replies `Status`, then the app goes away.
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeekTo {
    /// Seconds from the start.
    Absolute(f64),
    /// Seconds from the current position (negative goes back).
    Relative(f64),
    /// 0..100 of the song.
    Percent(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeTo {
    /// 0..1
    Set(f64),
    /// Added to the current level (0..1 scale), clamped.
    Change(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickAction {
    /// Plays the picked songs now, as a list in the order given.
    Play,
    /// Plays them right after the current song.
    Next,
    /// Adds them to the end of the queue.
    Add,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Album,
    Playlist,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

// ------------------------------------------------------------------------------------- replies

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Status(Status),
    Collections { items: Vec<CollectionInfo> },
    Tracks { collection: CollectionInfo, songs: Vec<SongInfo> },
    Queue(Queue),
    Songs { songs: Vec<SongInfo> },
    /// The playing song's lyrics. `song_id` is `SongInfo::id` of that song ("" when none plays),
    /// so a client can tell which song a reply belongs to. `lines` is empty unless `status` is
    /// `plain` or `synced`.
    Lyrics {
        song_id: String,
        status: LyricsStatus,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lines: Vec<LyricLine>,
    },
    Error { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsStatus {
    /// No lyrics: nothing playing, a song without any, signed out, or Apple has none.
    #[default]
    None,
    /// On their way.
    Loading,
    /// Text only: the lines carry no times.
    Plain,
    /// Timed: lines have `start`/`end`, and `words` when Apple times each word.
    Synced,
}

/// One line of lyrics. Times are seconds from the start of the song (0 when `plain`).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct LyricLine {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Sung by the second voice (shown right-aligned).
    #[serde(default)]
    pub second_voice: bool,
    /// Per-word timing; absent for line-timed and plain lyrics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<LyricWord>,
}

/// A word with its own timing; `text` keeps the space that follows it.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct LyricWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayState {
    /// Nothing loaded, or the queue ran out.
    #[default]
    Stopped,
    /// Starting, buffering or seeking.
    Loading,
    Playing,
    Paused,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Status {
    pub state: PlayState,
    pub song: Option<SongInfo>,
    /// Seconds, as of the reply.
    pub position: f64,
    /// Seconds (0 when unknown).
    pub duration: f64,
    /// 0..1
    pub volume: f64,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// Name of the list playback started from ("Continue playing from …"); "" when none.
    pub source: String,
    /// 0-based place of the current song in that list, in the list's own order (with shuffle
    /// on too, like mpc). `None` while a queued (Play Next / Add to Queue) song plays, or when
    /// nothing does.
    pub index: Option<usize>,
    /// Songs in that list.
    pub count: usize,
    /// Play Next / Add to Queue songs still waiting.
    pub queued: usize,
    /// The playback bridge (Electron) runs and the page is ready.
    pub connected: bool,
    pub signed_in: bool,
    /// The app's last error, if any (e.g. the bridge failed to start).
    pub error: Option<String>,
    /// The current song's cover as a file on this machine (the app's artwork disk cache, or a
    /// local song's thumbnail), once it is there. `None` while it downloads, for songs without
    /// artwork, and from apps older than this field. A path is enough: the socket is local, so
    /// `uzu` always runs where the app does (over ssh only the terminal is elsewhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct SongInfo {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds (0 when unknown).
    pub duration: f64,
    pub explicit: bool,
    /// A local file (played in-process, not by Apple Music).
    pub local: bool,
    /// False when Apple can't stream it (pulled from the catalog); it can't be played.
    pub playable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollectionInfo {
    pub kind: Kind,
    pub id: String,
    pub title: String,
    /// Artist for albums, curator for playlists (may be empty).
    pub subtitle: String,
    /// Song count when known.
    pub count: Option<u32>,
    /// Apple's Favourite Songs playlist.
    pub favorites: bool,
    /// The user's own playlist (songs can be added).
    pub editable: bool,
    /// Release year, for albums.
    pub year: Option<String>,
    /// Playlists pinned in the app's sidebar: the place among the pinned playlists (0 = top).
    /// Absent from older apps (then nothing counts as pinned) and ignored by older clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<u32>,
}

/// Puts the pinned collections first, in pin order; the rest keep their order (stable).
pub fn pinned_first(items: &mut [CollectionInfo]) {
    items.sort_by_key(|c| c.pinned.unwrap_or(u32::MAX));
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Queue {
    pub current: Option<SongInfo>,
    /// What plays after the current song, in order. The first `queued` entries are songs the
    /// user added (Play Next / Add to Queue); the rest continue `source`.
    pub up_next: Vec<SongInfo>,
    pub queued: usize,
    pub source: String,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// Repeat all only: the whole next lap, exactly as it will play, shown after `up_next`.
    /// Numbered on from it (entry `up_next.len() + k` is the k-th song here; only `Jump` takes
    /// those numbers). Empty from an older app, and always with repeat off or one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_lap: Vec<SongInfo>,
    /// The radio station playing, by name. Apple picks a station's songs as it goes, so `up_next`
    /// holds only what the user queued by hand. `None` from an older app, and off a station.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    /// Songs the station already played in this session, most recent first (at most 50). Numbered
    /// on after `up_next` and `next_lap` (entry `up_next.len() + next_lap.len() + k`); `Jump` plays
    /// it on its own, then the station picks up again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub station_played: Vec<SongInfo>,
    /// Confirmed session history, oldest first, excluding current.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<SongInfo>,
}

// ------------------------------------------------------------------------------------ transport

/// How long a client waits for one reply. Fetching a 1000-song playlist from Apple takes a few
/// seconds; this only guards against a hung app.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// A connection to the running app.
pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    /// Connects to [`socket_path`]. Fails (NotFound / ConnectionRefused) when Uzumaki isn't running.
    pub fn connect() -> io::Result<Self> {
        Self::connect_to(&socket_path())
    }

    pub fn connect_to(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { writer: stream, reader })
    }

    /// Sends one request and waits for its reply. An app-side failure comes back as
    /// `Ok(Reply::Error { .. })`; `Err` means the connection itself failed.
    pub fn request(&mut self, req: &Request) -> io::Result<Reply> {
        let mut line = serde_json::to_string(req).map_err(io::Error::other)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        let mut answer = String::new();
        if self.reader.read_line(&mut answer)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the app closed the connection"));
        }
        serde_json::from_str(&answer).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// The listening socket. Dropping it removes the socket file (the accept thread ends with the
/// process).
pub struct Server {
    path: PathBuf,
}

impl Server {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Listens on `path` and answers every request with `handler`, which runs on a per-connection
/// thread (it may block). A stale socket file from a crashed run is replaced; one that still
/// answers means another app owns it (`AddrInUse`).
pub fn serve(path: &Path, handler: impl Fn(Request) -> Reply + Send + Sync + 'static) -> io::Result<Server> {
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("{} is in use", path.display())));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let handler = Arc::new(handler);
    std::thread::Builder::new().name("uzumaki-ctl".into()).spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let handler = handler.clone();
            let _ = std::thread::Builder::new().name("uzumaki-ctl-conn".into()).spawn(move || connection(stream, &*handler));
        }
    })?;
    Ok(Server { path: path.to_path_buf() })
}

fn connection(stream: UnixStream, handler: &(dyn Fn(Request) -> Reply + Send + Sync)) {
    let Ok(mut writer) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handler(req),
            Err(e) => Reply::Error { message: format!("bad request: {e}") },
        };
        let Ok(mut out) = serde_json::to_string(&reply) else { return };
        out.push('\n');
        if writer.write_all(out.as_bytes()).and_then(|_| writer.flush()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An old app sends no `next_lap` (and an old `uzu` ignores it): both directions keep working.
    #[test]
    fn the_next_lap_is_an_optional_field_of_the_queue() {
        let old = r#"{"current":null,"up_next":[],"queued":0,"source":"","shuffle":false,"repeat":"all"}"#;
        let q: Queue = serde_json::from_str(old).unwrap();
        assert!(q.next_lap.is_empty());
        // Nothing to say: the field isn't written, so an old `uzu` sees the queue it knows.
        assert!(!serde_json::to_string(&q).unwrap().contains("next_lap"));
        let song = |id: &str| SongInfo { id: id.into(), title: id.into(), ..Default::default() };
        let q = Queue { next_lap: vec![song("a"), song("b")], ..Default::default() };
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(serde_json::from_str::<Queue>(&json).unwrap(), q);
        // An old reader's struct (no such field) takes the same JSON: unknown fields are ignored.
        #[derive(Deserialize)]
        struct OldQueue {
            up_next: Vec<SongInfo>,
            queued: usize,
        }
        let o: OldQueue = serde_json::from_str(&json).unwrap();
        assert_eq!((o.up_next.len(), o.queued), (0, 0));
    }

    /// An old app sends no station fields; an old `uzu` ignores them.
    #[test]
    fn the_station_is_an_optional_part_of_the_queue() {
        let old = r#"{"current":null,"up_next":[],"queued":0,"source":"","shuffle":false,"repeat":"off"}"#;
        let q: Queue = serde_json::from_str(old).unwrap();
        assert_eq!((q.station.clone(), q.station_played.len()), (None, 0));
        let json = serde_json::to_string(&q).unwrap();
        assert!(!json.contains("station"), "nothing to say, so an old uzu sees the queue it knows");
        let song = |id: &str| SongInfo { id: id.into(), title: id.into(), ..Default::default() };
        let q = Queue { station: Some("Stazione di Thomas".into()), station_played: vec![song("a"), song("b")], ..Default::default() };
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(serde_json::from_str::<Queue>(&json).unwrap(), q);
        #[derive(Deserialize)]
        struct OldQueue {
            queued: usize,
        }
        assert_eq!(serde_json::from_str::<OldQueue>(&json).unwrap().queued, 0);
    }

    fn temp_socket(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("uzumaki-ctl-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("ctl.sock")
    }

    /// The Dock-started app and `uzu` over ssh agree on the folder even when `$TMPDIR` is unset,
    /// and the socket path fits a macOS `sun_path` (104 bytes).
    #[test]
    #[cfg(target_os = "macos")]
    fn macos_socket_folder_comes_from_the_system() {
        let temp = darwin_user_temp_dir().expect("confstr(_CS_DARWIN_USER_TEMP_DIR)");
        assert!(temp.is_dir(), "{} is a folder", temp.display());
        let dir = runtime_dir();
        assert_eq!(dir, temp.join(format!("uzumaki-{}", uid())));
        assert!(dir.join("uzumaki.sock.last").as_os_str().len() < 104);
        assert_ne!(uid(), 0, "tests don't run as root");
    }

    #[test]
    fn wire_format_is_readable() {
        let req = Request::Seek { to: SeekTo::Relative(-10.0) };
        assert_eq!(serde_json::to_string(&req).unwrap(), r#"{"cmd":"seek","to":{"relative":-10.0}}"#);
        let req = Request::PlayCollection { kind: Kind::Playlist, id: "p.x".into(), index: Some(2), order: None };
        assert_eq!(serde_json::to_string(&req).unwrap(), r#"{"cmd":"play_collection","kind":"playlist","id":"p.x","index":2}"#);
        // An old client's message has no `order`; a new one's round-trips.
        assert_eq!(serde_json::from_str::<Request>(r#"{"cmd":"play_collection","kind":"playlist","id":"p.x","index":2}"#).unwrap(), req);
        let ordered = Request::PlayCollection { kind: Kind::Album, id: "l.1".into(), index: None, order: Some(vec![2, 0, 1]) };
        let text = serde_json::to_string(&ordered).unwrap();
        assert_eq!(text, r#"{"cmd":"play_collection","kind":"album","id":"l.1","index":null,"order":[2,0,1]}"#);
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), ordered);
        assert_eq!(serde_json::to_string(&Request::Status).unwrap(), r#"{"cmd":"status"}"#);
        let back: Request = serde_json::from_str(r#"{"cmd":"shuffle","on":null}"#).unwrap();
        assert_eq!(back, Request::Shuffle { on: None });
        let reply = Reply::Status(Status { volume: 0.5, ..Default::default() });
        let text = serde_json::to_string(&reply).unwrap();
        assert!(text.starts_with(r#"{"reply":"status","state":"stopped""#), "{text}");
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), reply);
    }

    /// `cover` is new: an old app's status (no field) still reads, a status without a cover
    /// doesn't send it, and fields this build doesn't know (a newer app's) are ignored.
    #[test]
    fn status_cover_is_optional_both_ways() {
        let old = r#"{"reply":"status","state":"playing","song":null,"position":1.0,"duration":2.0,"volume":0.5,"shuffle":false,"repeat":"off","source":"","index":null,"count":0,"queued":0,"connected":true,"signed_in":false,"error":null}"#;
        let Reply::Status(st) = serde_json::from_str::<Reply>(old).unwrap() else { panic!("not a status") };
        assert_eq!(st.cover, None);
        assert!(!serde_json::to_string(&Reply::Status(st.clone())).unwrap().contains("cover"));
        let with = Reply::Status(Status { cover: Some("/home/x/.cache/uzumaki/art/00ff".into()), ..st });
        let text = serde_json::to_string(&with).unwrap();
        assert!(text.contains(r#""cover":"/home/x/.cache/uzumaki/art/00ff""#), "{text}");
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), with);
        let newer = old.replace(r#""error":null"#, r#""error":null,"something_new":[1,2]"#);
        assert!(matches!(serde_json::from_str::<Reply>(&newer), Ok(Reply::Status(_))));
    }

    /// `pinned` is new: an old app's playlist (no field) reads as unpinned, an unpinned one
    /// doesn't send it, a pinned one round-trips, and an old uzu (which doesn't know the field)
    /// still reads a new app's list.
    #[test]
    fn collection_pinned_is_optional_both_ways() {
        let old = r#"{"reply":"collections","items":[{"kind":"playlist","id":"p.1","title":"Mix","subtitle":"","count":3,"favorites":false,"editable":true,"year":null}]}"#;
        let Reply::Collections { items } = serde_json::from_str::<Reply>(old).unwrap() else { panic!("not collections") };
        assert_eq!(items[0].pinned, None);
        assert_eq!(serde_json::to_string(&Reply::Collections { items: items.clone() }).unwrap(), old, "unpinned: same bytes as before");
        let mut pinned = items[0].clone();
        pinned.pinned = Some(2);
        let text = serde_json::to_string(&Reply::Collections { items: vec![pinned.clone()] }).unwrap();
        assert!(text.ends_with(r#""year":null,"pinned":2}]}"#), "{text}");
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), Reply::Collections { items: vec![pinned] });

        /// The old uzu's `CollectionInfo`, as it was before `pinned`.
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct OldInfo {
            kind: Kind,
            id: String,
            title: String,
            subtitle: String,
            count: Option<u32>,
            favorites: bool,
            editable: bool,
            year: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(tag = "reply", rename_all = "snake_case")]
        enum OldReply {
            Collections { items: Vec<OldInfo> },
        }
        let OldReply::Collections { items } = serde_json::from_str::<OldReply>(&text).unwrap();
        assert_eq!(items[0].id, "p.1");
    }

    #[test]
    fn pinned_first_is_stable() {
        let c = |id: &str, pinned: Option<u32>| CollectionInfo { kind: Kind::Playlist, id: id.into(), title: id.into(), subtitle: String::new(), count: None, favorites: false, editable: true, year: None, pinned };
        let mut items = vec![c("a", None), c("b", Some(1)), c("c", None), c("d", Some(0))];
        pinned_first(&mut items);
        assert_eq!(items.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["d", "b", "a", "c"]);
    }

    /// `Lyrics` is new. A new uzu's request reads as itself, an old app can't read it (the server
    /// answers "bad request", which uzu shows as "update Uzumaki"), the reply drops empty `words`
    /// and `lines`, and replies read both ways.
    #[test]
    fn lyrics_wire_format_both_ways() {
        assert_eq!(serde_json::to_string(&Request::Lyrics).unwrap(), r#"{"cmd":"lyrics"}"#);
        assert_eq!(serde_json::from_str::<Request>(r#"{"cmd":"lyrics"}"#).unwrap(), Request::Lyrics);
        let reply = Reply::Lyrics {
            song_id: "i.1".into(),
            status: LyricsStatus::Synced,
            lines: vec![
                LyricLine { start: 1.0, end: 2.0, text: "la di".into(), second_voice: false, words: vec![LyricWord { start: 1.0, end: 1.5, text: "la ".into() }, LyricWord { start: 1.5, end: 2.0, text: "di".into() }] },
                LyricLine { start: 3.0, end: 4.0, text: "da".into(), second_voice: true, words: vec![] },
            ],
        };
        let text = serde_json::to_string(&reply).unwrap();
        assert!(text.starts_with(r#"{"reply":"lyrics","song_id":"i.1","status":"synced","lines":[{"start":1.0,"end":2.0,"text":"la di","second_voice":false,"words":[{"#), "{text}");
        assert!(text.contains(r#"{"start":3.0,"end":4.0,"text":"da","second_voice":true}"#), "no empty words: {text}");
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), reply);
        // Without lines (loading, none) the field is left out and reads back as empty.
        let loading = Reply::Lyrics { song_id: "i.1".into(), status: LyricsStatus::Loading, lines: vec![] };
        let text = serde_json::to_string(&loading).unwrap();
        assert_eq!(text, r#"{"reply":"lyrics","song_id":"i.1","status":"loading"}"#);
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), loading);
        // A newer app's extra fields are ignored; missing optional ones default.
        let newer = r#"{"reply":"lyrics","song_id":"x","status":"plain","lines":[{"start":0,"end":0,"text":"a","extra":1}],"more":true}"#;
        let Reply::Lyrics { lines, .. } = serde_json::from_str::<Reply>(newer).unwrap() else { panic!("not lyrics") };
        assert_eq!(lines[0], LyricLine { text: "a".into(), ..Default::default() });
    }

    /// An app without `Lyrics` answers it like any request it can't read.
    #[test]
    fn an_old_app_answers_lyrics_with_bad_request() {
        #[derive(Deserialize)]
        #[serde(tag = "cmd", rename_all = "snake_case")]
        #[allow(dead_code)]
        enum OldRequest {
            Status,
        }
        assert!(serde_json::from_str::<OldRequest>(r#"{"cmd":"lyrics"}"#).is_err());
    }

    #[test]
    fn serve_answers_in_order_and_cleans_up() {
        let path = temp_socket("order");
        let server = serve(&path, |req| match req {
            Request::Search { query } => Reply::Songs { songs: vec![SongInfo { title: query, ..Default::default() }] },
            _ => Reply::Error { message: "nope".into() },
        })
        .unwrap();
        let mut c = Client::connect_to(&path).unwrap();
        let r = c.request(&Request::Search { query: "abc".into() }).unwrap();
        assert!(matches!(r, Reply::Songs { ref songs } if songs[0].title == "abc"));
        let r = c.request(&Request::Status).unwrap();
        assert_eq!(r, Reply::Error { message: "nope".into() });
        // A second app on the same path is refused while the first answers.
        assert_eq!(serve(&path, |_| Reply::Error { message: String::new() }).err().map(|e| e.kind()), Some(io::ErrorKind::AddrInUse));
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn stale_socket_is_replaced_and_bad_lines_get_an_error() {
        let path = temp_socket("stale");
        {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let _dead = UnixListener::bind(&path).unwrap(); // bound, then dropped: nobody answers
        }
        assert!(path.exists());
        let _server = serve(&path, |_| Reply::Status(Status::default())).unwrap();
        let mut s = UnixStream::connect(&path).unwrap();
        s.write_all(b"not json\n").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        assert!(line.contains(r#""reply":"error""#), "{line}");
    }
}

#[cfg(test)]
mod history_protocol_tests {
    use super::*;
    #[test]
    fn history_extension_accepts_old_queue_and_round_trips_new_queue() {
        let old = r#"{"current":null,"up_next":[],"queued":0,"source":"","shuffle":false,"repeat":"off"}"#;
        let mut queue: Queue = serde_json::from_str(old).unwrap();
        assert!(queue.history.is_empty());
        queue.history.push(SongInfo { id: "played".into(), ..Default::default() });
        assert_eq!(serde_json::from_str::<Queue>(&serde_json::to_string(&queue).unwrap()).unwrap(), queue);
        let request = Request::History { index: 2 };
        assert_eq!(serde_json::from_str::<Request>(&serde_json::to_string(&request).unwrap()).unwrap(), request);
    }
}

#[cfg(test)]
mod build_tests {
    use super::*;

    #[test]
    fn the_build_file_reads_as_a_number() {
        assert_eq!(parse_build("1\n"), 1);
        assert_eq!(parse_build(" 12 \r\n"), 12);
        assert!(BUILD >= 1);
        assert_eq!(BUILD.to_string(), include_str!("../../BUILD").trim());
        assert_eq!(version_line(), format!("{} (build {BUILD}){}", env!("CARGO_PKG_VERSION"), dev_suffix()));
    }

    fn facts(branch: &'static str, hash: &'static str, dirty: bool, at_release_tag: bool) -> Option<GitFacts<'static>> {
        Some(GitFacts { branch, hash, dirty, at_release_tag })
    }

    #[test]
    fn the_dev_suffix_follows_the_rule() {
        // No git info (tarball, AUR source build): nothing.
        assert_eq!(git_suffix(None), "");
        // Exactly at the release tag, clean: nothing.
        assert_eq!(git_suffix(facts("main", "20c1fe9", false, true)), "");
        // Anywhere else: branch and short hash.
        assert_eq!(git_suffix(facts("main", "20c1fe9", false, false)), " \u{b7} main 20c1fe9");
        // Changes in the tree: "dirty", even at the tag.
        assert_eq!(git_suffix(facts("animated-art", "1a2b3c4", true, false)), " \u{b7} animated-art 1a2b3c4 dirty");
        assert_eq!(git_suffix(facts("main", "20c1fe9", true, true)), " \u{b7} main 20c1fe9 dirty");
    }

    #[test]
    fn the_build_script_facts_parse() {
        assert_eq!(parse_facts(""), None);
        assert_eq!(parse_facts("main\t20c1fe9\t1\t0"), facts("main", "20c1fe9", true, false));
        assert_eq!(parse_facts("main\t20c1fe9"), None);
    }

    #[test]
    #[should_panic]
    fn two_numbers_are_refused() {
        parse_build("1 2");
    }

    #[test]
    #[should_panic]
    fn zero_is_refused() {
        parse_build("0");
    }
}
