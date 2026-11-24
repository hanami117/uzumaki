//! Lyrics from LRCLIB (<https://lrclib.net>, free, no key) for local files that have none of
//! their own (`local::lyrics` finds nothing beside the file or in its tags).
//!
//! - **Asking:** `/api/get` with title, artist, album and length first (LRCLIB matches the
//!   length within ±2 s); when that finds nothing, `/api/search` by title and artist, keeping a
//!   result whose length is within `TOLERANCE`. Synced lyrics win over plain ones; a record
//!   marked instrumental counts as "no lyrics". Requests carry a User-Agent naming the app, as
//!   LRCLIB asks.
//! - **Politeness:** one `Client` per app; a 429 or 5xx holds every request for its
//!   `Retry-After` (plus up to 30 % jitter) or a growing backoff, and so does a network error
//!   (offline: one quiet try every few minutes at most). The batch (`State::lrclib_all`) asks one
//!   song at a time with a pause between.
//! - **Cache:** one small JSON file per song in `$XDG_CACHE_HOME/uzumaki/lyrics/`, named by the
//!   file's identity (path, size, mtime: a re-tagged file is asked again). "Not found" is kept
//!   too and asked again after `NONE_KEEP`; instrumental is kept for good. Failures are never
//!   cached. Nothing is written into the music folders unless "Save .lrc files next to the
//!   songs" is on (`save_beside`), and then never over an existing file.
//!
//! Only artist, title, album and length leave the computer.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::lyrics::{Lyrics, parse_lrc};
use crate::models::Song;

/// LRCLIB's address.
pub const BASE: &str = "https://lrclib.net";
/// The credit shown under lyrics from LRCLIB.
pub const CREDIT: &str = "Lyrics from LRCLIB";
/// A search result this far from the file's length (seconds) is another recording.
const TOLERANCE: f64 = 2.0;
/// One request's time limit.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Largest reply read (a long song's LRC is a few KB; search returns up to 20 records).
const BODY_MAX: u64 = 2 << 20;
/// "Not found" is asked again after this (LRCLIB fills its database in the background).
const NONE_KEEP: Duration = Duration::from_secs(10 * 24 * 3600);
/// Backoff after a network error or a 5xx without Retry-After: doubles up to `MAX_HOLD`.
const FIRST_HOLD: Duration = Duration::from_secs(30);
const MAX_HOLD: Duration = Duration::from_secs(30 * 60);
/// The longest Retry-After believed (a broken header must not stop lyrics for a day).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// Version of the cache files; another version reads as missing.
const CACHE_VERSION: u32 = 1;

/// What LRCLIB is asked about one song.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub title: String,
    pub artist: String,
    /// None when the file has no album tag (the folder's name stands in for it).
    pub album: Option<String>,
    /// Seconds; 0 when unknown.
    pub duration: f64,
}

impl Query {
    /// The query for a local song; None without a title or an artist (nothing to match on).
    pub fn of(song: &Song) -> Option<Query> {
        let title = song.title.trim();
        let artist = song.artist.trim();
        if title.is_empty() || artist.is_empty() || artist.eq_ignore_ascii_case("unknown artist") {
            return None;
        }
        let album = Some(song.album.trim()).filter(|a| !a.is_empty() && !song.album_is_fallback).map(str::to_string);
        Some(Query { title: title.to_string(), artist: artist.to_string(), album, duration: song.duration.max(0.0) })
    }

    /// The length sent to `/api/get` (LRCLIB takes 1 to 3600 s), if known.
    fn seconds(&self) -> Option<u32> {
        let d = self.duration.round();
        (1.0..=3600.0).contains(&d).then_some(d as u32)
    }
}

/// One LRCLIB record (only the fields used).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Record {
    duration: f64,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

impl Record {
    fn synced(&self) -> Option<&str> {
        self.synced_lyrics.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
    fn plain(&self) -> Option<&str> {
        self.plain_lyrics.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
    fn answer(&self) -> Answer {
        if self.instrumental {
            return Answer::Instrumental;
        }
        match (self.synced(), self.plain()) {
            (Some(s), _) => Answer::Synced(s.to_string()),
            (None, Some(p)) => Answer::Plain(p.to_string()),
            (None, None) => Answer::None,
        }
    }
}

/// What LRCLIB said about a song (the raw text is what the cache and a sidecar keep).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum Answer {
    Synced(String),
    Plain(String),
    Instrumental,
    None,
}

impl Answer {
    /// The lyrics to show, credited to LRCLIB; None for "no lyrics".
    pub fn lyrics(&self) -> Option<Lyrics> {
        let mut l = match self {
            Answer::Synced(t) => parse_lrc(t)?,
            Answer::Plain(t) => {
                // Plain text that happens to start with "[" is still plain: drop any stamps.
                let mut l = parse_lrc(t)?;
                if l.synced {
                    l.synced = false;
                    for x in &mut l.lines {
                        x.start = 0.0;
                        x.end = 0.0;
                    }
                }
                l
            }
            Answer::Instrumental | Answer::None => return None,
        };
        l.credit = Some(CREDIT);
        Some(l)
    }
}

/// Why there is no answer this time (nothing is cached; ask again later).
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// Requests are on hold (a recent 429/5xx or network error) until then.
    Held(Instant),
    /// LRCLIB said 429 or 5xx.
    Busy(u16),
    /// No connection, a timeout, an unreadable reply.
    Network(String),
}

/// The hold every request respects.
#[derive(Debug, Default)]
struct Gate {
    until: Option<Instant>,
    /// Failures in a row (the backoff's exponent).
    strikes: u32,
}

/// The HTTP client. One per app, shared by the song-start lookups and the batch.
pub struct Client {
    agent: ureq::Agent,
    base: String,
    gate: Mutex<Gate>,
}

impl Client {
    pub fn new(base: &str) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .user_agent(format!("Uzumaki/{} (https://getuzumaki.app)", crate::VERSION))
            .build()
            .into();
        Client { agent, base: base.trim_end_matches('/').to_string(), gate: Mutex::new(Gate::default()) }
    }

    /// When requests may go again, if they are on hold now.
    pub fn held_until(&self) -> Option<Instant> {
        let g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        g.until.filter(|t| *t > Instant::now())
    }

    /// Network or server failures in a row.
    #[cfg(test)]
    pub fn strikes(&self) -> u32 {
        self.gate.lock().unwrap_or_else(|e| e.into_inner()).strikes
    }

    /// Asks LRCLIB about `q`: the exact match first, then a search.
    pub fn lookup(&self, q: &Query) -> Result<Answer, Failure> {
        if let Some(rec) = self.get(q)? {
            let a = rec.answer();
            if a != Answer::None {
                return Ok(a);
            }
        }
        Ok(best(&self.search(q)?, q))
    }

    fn get(&self, q: &Query) -> Result<Option<Record>, Failure> {
        let mut params: Vec<(&str, String)> = vec![("track_name", q.title.clone()), ("artist_name", q.artist.clone())];
        if let Some(a) = &q.album {
            params.push(("album_name", a.clone()));
        }
        if let Some(d) = q.seconds() {
            params.push(("duration", d.to_string()));
        }
        match self.call("/api/get", &params)? {
            Reply::NotFound => Ok(None),
            Reply::Json(v) => Ok(serde_json::from_value::<Record>(v).ok()),
        }
    }

    fn search(&self, q: &Query) -> Result<Vec<Record>, Failure> {
        let params = [("track_name", q.title.clone()), ("artist_name", q.artist.clone())];
        match self.call("/api/search", &params)? {
            Reply::NotFound => Ok(Vec::new()),
            Reply::Json(v) => Ok(serde_json::from_value::<Vec<Record>>(v).unwrap_or_default()),
        }
    }

    /// One GET, through the gate.
    fn call(&self, path: &str, params: &[(&str, String)]) -> Result<Reply, Failure> {
        if let Some(t) = self.held_until() {
            return Err(Failure::Held(t));
        }
        let mut req = self.agent.get(format!("{}{path}", self.base));
        for (k, v) in params {
            req = req.query(*k, v);
        }
        let mut resp = match req.call() {
            Ok(r) => r,
            Err(e) => {
                self.hold(None);
                return Err(Failure::Network(e.to_string()));
            }
        };
        let status = resp.status().as_u16();
        if status == 429 || status >= 500 {
            let after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
            self.hold(after.map(Duration::from_secs));
            return Err(Failure::Busy(status));
        }
        if status == 404 {
            self.ok();
            return Ok(Reply::NotFound);
        }
        if !(200..300).contains(&status) {
            // 400 and the like: this song's query is the problem, not the server.
            self.ok();
            return Ok(Reply::NotFound);
        }
        let body = match resp.body_mut().with_config().limit(BODY_MAX).read_to_vec() {
            Ok(b) => b,
            Err(e) => {
                self.hold(None);
                return Err(Failure::Network(e.to_string()));
            }
        };
        self.ok();
        match serde_json::from_slice(&body) {
            Ok(v) => Ok(Reply::Json(v)),
            Err(e) => Err(Failure::Network(format!("unreadable reply: {e}"))),
        }
    }

    fn ok(&self) {
        let mut g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        g.strikes = 0;
        g.until = None;
    }

    /// Holds every request: for `retry_after` (+ up to 30 % jitter), else the backoff.
    fn hold(&self, retry_after: Option<Duration>) {
        let mut g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        g.strikes = g.strikes.saturating_add(1);
        let wait = match retry_after {
            Some(d) => d.min(MAX_RETRY_AFTER),
            None => FIRST_HOLD.saturating_mul(1 << g.strikes.saturating_sub(1).min(10)).min(MAX_HOLD),
        };
        g.until = Some(Instant::now() + wait + jitter(wait));
    }
}

enum Reply {
    NotFound,
    Json(serde_json::Value),
}

/// Up to 30 % of `d`, from the clock's nanoseconds (no RNG needed for this).
fn jitter(d: Duration) -> Duration {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |t| t.subsec_nanos());
    d.mul_f64(f64::from(n % 1000) / 1000.0 * 0.3)
}

/// The best search result for `q`: within `TOLERANCE` of its length (any length when the file's
/// is unknown), synced before plain, then the closest length. Instrumental if that is all there is.
fn best(records: &[Record], q: &Query) -> Answer {
    let near = |r: &&Record| q.seconds().is_none() || (r.duration - q.duration).abs() <= TOLERANCE;
    let gap = |r: &Record| if q.seconds().is_some() { (r.duration - q.duration).abs() } else { 0.0 };
    let pick = |keep: &dyn Fn(&Record) -> bool| records.iter().filter(near).filter(|r| !r.instrumental && keep(r)).min_by(|a, b| gap(a).total_cmp(&gap(b)));
    if let Some(r) = pick(&|r| r.synced().is_some()) {
        return r.answer();
    }
    if let Some(r) = pick(&|r| r.plain().is_some()) {
        return r.answer();
    }
    if records.iter().filter(near).any(|r| r.instrumental) { Answer::Instrumental } else { Answer::None }
}

// ---- the cache ----

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    /// When LRCLIB answered, seconds since the epoch.
    at: u64,
    answer: Answer,
}

/// The cache of LRCLIB's answers, one file per song.
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Cache {
        Cache { dir }
    }

    /// `$XDG_CACHE_HOME/uzumaki/lyrics`.
    pub fn default_dir() -> PathBuf {
        super::cache_base().join("lyrics")
    }

    /// The file for the song at `path` (None when the file can't be read).
    fn file(&self, path: &Path) -> Option<PathBuf> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(path.to_string_lossy().as_bytes());
        eat(&[0]);
        eat(&meta.len().to_le_bytes());
        eat(&mtime.to_le_bytes());
        Some(self.dir.join(format!("{h:016x}.json")))
    }

    /// A kept answer still good: lyrics and instrumental for good, "none" for `NONE_KEEP`.
    pub fn get(&self, path: &Path) -> Option<Answer> {
        let bytes = std::fs::read(self.file(path)?).ok()?;
        let e: Entry = serde_json::from_slice(&bytes).ok()?;
        if e.version != CACHE_VERSION {
            return None;
        }
        if e.answer == Answer::None && now_secs().saturating_sub(e.at) >= NONE_KEEP.as_secs() {
            return None;
        }
        Some(e.answer)
    }

    pub fn put(&self, path: &Path, answer: &Answer) {
        let Some(file) = self.file(path) else { return };
        let e = Entry { version: CACHE_VERSION, at: now_secs(), answer: answer.clone() };
        let Ok(bytes) = serde_json::to_vec(&e) else { return };
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &file)).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Writes `answer` beside the song: synced lyrics as `<name>.lrc`, plain ones as `<name>.txt`.
/// Never over an existing `.lrc` or `.txt`; a read-only folder is skipped quietly.
pub fn save_beside(song: &Path, answer: &Answer) {
    let (ext, text) = match answer {
        Answer::Synced(t) => ("lrc", t),
        Answer::Plain(t) => ("txt", t),
        Answer::Instrumental | Answer::None => return,
    };
    if ["lrc", "LRC", "txt", "TXT"].iter().any(|e| song.with_extension(e).exists()) {
        return;
    }
    let target = song.with_extension(ext);
    let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).and_then(|mut f| {
        f.write_all(text.as_bytes())?;
        f.write_all(b"\n")
    });
    if let Err(e) = written {
        eprintln!("uzumaki: lyrics: can't write {}: {e}", target.display());
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(&target);
        }
    }
}

/// The client and the cache together: what the app holds (`State::lrclib`).
pub struct Lrclib {
    pub client: Client,
    pub cache: Cache,
}

/// What a lookup for one song came to.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// LRCLIB's answer (fresh, or from the cache when `cached`).
    Answer { answer: Answer, cached: bool },
    /// No answer this time (offline, LRCLIB busy): nothing kept, asked again later.
    Later(Failure),
}

impl Lrclib {
    pub fn new(base: &str, cache_dir: PathBuf) -> Lrclib {
        Lrclib { client: Client::new(base), cache: Cache::new(cache_dir) }
    }

    /// The app's: lrclib.net and the user's cache directory.
    pub fn for_app() -> Lrclib {
        Lrclib::new(BASE, Cache::default_dir())
    }

    /// The answer for the local file `path` (its song `q`): the cache's, else LRCLIB's (then
    /// kept). With `save`, a found answer is also written beside the file.
    pub fn find(&self, path: &Path, q: &Query, save: bool) -> Outcome {
        let outcome = match self.cache.get(path) {
            Some(answer) => Outcome::Answer { answer, cached: true },
            None => match self.client.lookup(q) {
                Ok(answer) => {
                    self.cache.put(path, &answer);
                    Outcome::Answer { answer, cached: false }
                }
                Err(f) => Outcome::Later(f),
            },
        };
        if let (true, Outcome::Answer { answer, .. }) = (save, &outcome) {
            save_beside(path, answer);
        }
        outcome
    }
}

#[cfg(test)]
pub mod fake {
    //! A fake LRCLIB on 127.0.0.1 for tests: answers each request from a function of its path
    //! and query, and records what it was asked (no live network in tests).

    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// (status, extra headers, body).
    pub type Answer = (u16, Vec<(&'static str, String)>, String);

    pub struct Server {
        pub base: String,
        pub asked: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        pub fn start(answer: impl Fn(&str) -> Answer + Send + 'static) -> Server {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", l.local_addr().unwrap());
            let asked = Arc::new(Mutex::new(Vec::new()));
            let log = asked.clone();
            std::thread::spawn(move || {
                for stream in l.incoming() {
                    let Ok(mut s) = stream else { continue };
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    let mut first = String::new();
                    if r.read_line(&mut first).is_err() {
                        continue;
                    }
                    let mut ua = String::new();
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                            break;
                        }
                        if let Some(v) = h.to_ascii_lowercase().strip_prefix("user-agent:") {
                            ua = v.trim().to_string();
                        }
                    }
                    let target = first.split_whitespace().nth(1).unwrap_or("").to_string();
                    log.lock().unwrap().push(format!("{target} ua={ua}"));
                    let (status, headers, body) = answer(&target);
                    let mut out = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                    for (k, v) in headers {
                        out.push_str(&format!("{k}: {v}\r\n"));
                    }
                    out.push_str("\r\n");
                    out.push_str(&body);
                    let _ = s.write_all(out.as_bytes());
                }
            });
            Server { base, asked }
        }

        pub fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    pub fn record(duration: f64, synced: Option<&str>, plain: Option<&str>, instrumental: bool) -> serde_json::Value {
        serde_json::json!({
            "id": 1, "trackName": "T", "artistName": "A", "albumName": "B", "duration": duration,
            "instrumental": instrumental, "plainLyrics": plain, "syncedLyrics": synced,
        })
    }

    pub const NOT_FOUND: &str = r#"{"message":"Failed to find specified track","name":"TrackNotFound","statusCode":404}"#;
}

#[cfg(test)]
mod tests {
    use super::fake::{NOT_FOUND, Server, record};
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("uzumaki-lrclib-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn q(duration: f64) -> Query {
        Query { title: "Song & Co".into(), artist: "Ann".into(), album: Some("First".into()), duration }
    }

    fn ok(v: serde_json::Value) -> fake::Answer {
        (200, vec![], v.to_string())
    }

    #[test]
    fn get_sends_every_field_and_the_user_agent_and_prefers_synced() {
        let srv = Server::start(|_| ok(record(201.0, Some("[00:01.00]One\n[00:02.00]Two"), Some("One\nTwo"), false)));
        let c = Client::new(&srv.base);
        let a = c.lookup(&q(200.6)).unwrap();
        assert!(matches!(&a, Answer::Synced(t) if t.contains("[00:01.00]One")));
        let asked = srv.asked();
        assert_eq!(asked.len(), 1, "{asked:?}");
        let line = &asked[0];
        assert!(line.starts_with("/api/get?"), "{line}");
        for part in ["track_name=Song+%26+Co", "artist_name=Ann", "album_name=First", "duration=201"] {
            assert!(line.contains(part) || line.contains(&part.replace('+', "%20")), "{part} in {line}");
        }
        assert!(line.contains(&format!("ua=uzumaki/{} (https://getuzumaki.app)", crate::VERSION)), "{line}");
        let l = a.lyrics().unwrap();
        assert!(l.synced);
        assert_eq!(l.credit, Some(CREDIT));
        assert_eq!(l.lines[1].text, "Two");
    }

    #[test]
    fn no_album_tag_and_no_length_are_left_out() {
        let srv = Server::start(|_| ok(record(10.0, None, Some("Only plain"), false)));
        let c = Client::new(&srv.base);
        let mut query = q(0.0);
        query.album = None;
        let a = c.lookup(&query).unwrap();
        assert_eq!(a, Answer::Plain("Only plain".into()));
        let l = a.lyrics().unwrap();
        assert!(!l.synced);
        let line = &srv.asked()[0];
        assert!(!line.contains("album_name") && !line.contains("duration"), "{line}");
    }

    #[test]
    fn a_miss_falls_back_to_search_within_two_seconds() {
        let srv = Server::start(|path| {
            if path.starts_with("/api/get") {
                return (404, vec![], NOT_FOUND.into());
            }
            ok(serde_json::json!([
                record(150.0, Some("[00:01.00]Too short"), None, false),
                record(201.5, None, Some("Plain near"), false),
                record(198.6, Some("[00:01.00]Synced near"), None, false),
                record(200.0, None, None, true),
            ]))
        });
        let c = Client::new(&srv.base);
        assert_eq!(c.lookup(&q(200.0)).unwrap(), Answer::Synced("[00:01.00]Synced near".into()));
        let asked = srv.asked();
        assert!(asked[1].starts_with("/api/search?") && !asked[1].contains("album_name"), "{asked:?}");
    }

    #[test]
    fn search_results_pick_plain_then_instrumental_then_none() {
        let r = |d, s, p, i| serde_json::from_value::<Record>(record(d, s, p, i)).unwrap();
        let query = q(100.0);
        assert_eq!(best(&[r(100.0, None, Some("p"), false), r(130.0, Some("[00:01.00]x"), None, false)], &query), Answer::Plain("p".into()));
        assert_eq!(best(&[r(101.0, None, None, true)], &query), Answer::Instrumental);
        assert_eq!(best(&[r(120.0, None, Some("far"), false)], &query), Answer::None);
        assert_eq!(best(&[], &query), Answer::None);
        // The exact match marked instrumental is respected.
        assert_eq!(r(100.0, Some("[00:01.00]x"), Some("x"), true).answer(), Answer::Instrumental);
        assert_eq!(Answer::Instrumental.lyrics(), None);
    }

    #[test]
    fn busy_and_offline_hold_every_request() {
        let srv = Server::start(|_| (429, vec![("Retry-After", "60".into())], r#"{"name":"RateLimitExceeded"}"#.into()));
        let c = Client::new(&srv.base);
        assert_eq!(c.lookup(&q(100.0)), Err(Failure::Busy(429)));
        let until = c.held_until().expect("held");
        let left = until.saturating_duration_since(Instant::now());
        assert!(left > Duration::from_secs(55) && left <= Duration::from_secs(79), "Retry-After + ≤30 % jitter: {left:?}");
        assert!(matches!(c.lookup(&q(100.0)), Err(Failure::Held(_))));
        assert_eq!(srv.asked().len(), 1, "nothing sent while held");

        let srv = Server::start(|_| (503, vec![], "{}".into()));
        let c = Client::new(&srv.base);
        assert_eq!(c.lookup(&q(100.0)), Err(Failure::Busy(503)));
        assert!(c.held_until().is_some());

        // Nothing listens there: a network error, held too.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = Client::new(&format!("http://127.0.0.1:{port}"));
        assert!(matches!(c.lookup(&q(100.0)), Err(Failure::Network(_))));
        assert!(c.held_until().is_some());
        assert_eq!(c.strikes(), 1);
    }

    #[test]
    fn the_backoff_grows_and_a_success_clears_it() {
        let c = Client::new("http://127.0.0.1:9");
        c.hold(None);
        let first = c.held_until().unwrap().saturating_duration_since(Instant::now());
        c.hold(None);
        c.hold(None);
        let third = c.held_until().unwrap().saturating_duration_since(Instant::now());
        assert!(first <= FIRST_HOLD.mul_f64(1.3) && third >= FIRST_HOLD * 4 - Duration::from_secs(1), "{first:?} {third:?}");
        for _ in 0..40 {
            c.hold(None);
        }
        assert!(c.held_until().unwrap().saturating_duration_since(Instant::now()) <= MAX_HOLD.mul_f64(1.3));
        c.ok();
        assert_eq!(c.held_until(), None);
        assert_eq!(c.strikes(), 0);
    }

    #[test]
    fn answers_are_cached_by_file_identity_and_none_goes_stale() {
        let d = tmp("cache");
        let song = d.join("a.flac");
        fs::write(&song, b"x").unwrap();
        let cache = Cache::new(d.join("cache"));
        assert_eq!(cache.get(&song), None);
        cache.put(&song, &Answer::Plain("hi".into()));
        assert_eq!(cache.get(&song), Some(Answer::Plain("hi".into())));
        // Another size (re-tagged): another entry.
        fs::write(&song, b"xy").unwrap();
        assert_eq!(cache.get(&song), None);
        cache.put(&song, &Answer::None);
        assert_eq!(cache.get(&song), Some(Answer::None));
        // An old "none" is asked again; an old instrumental isn't.
        let file = cache.file(&song).unwrap();
        let old = |a: Answer| serde_json::to_vec(&Entry { version: CACHE_VERSION, at: now_secs() - NONE_KEEP.as_secs() - 1, answer: a }).unwrap();
        fs::write(&file, old(Answer::None)).unwrap();
        assert_eq!(cache.get(&song), None);
        fs::write(&file, old(Answer::Instrumental)).unwrap();
        assert_eq!(cache.get(&song), Some(Answer::Instrumental));
        fs::write(&file, b"{broken").unwrap();
        assert_eq!(cache.get(&song), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn find_asks_once_then_reads_the_cache_and_never_caches_failures() {
        let d = tmp("find");
        let song = d.join("a.mp3");
        fs::write(&song, b"x").unwrap();
        let srv = Server::start(|_| ok(record(100.0, Some("[00:01.00]Hey"), None, false)));
        let lr = Lrclib::new(&srv.base, d.join("cache"));
        assert!(matches!(lr.find(&song, &q(100.0), false), Outcome::Answer { cached: false, .. }));
        assert!(matches!(lr.find(&song, &q(100.0), false), Outcome::Answer { cached: true, .. }));
        assert_eq!(srv.asked().len(), 1, "asked once");
        assert!(!d.join("a.lrc").exists(), "nothing written beside the song by default");

        let other = d.join("b.mp3");
        fs::write(&other, b"y").unwrap();
        let srv = Server::start(|_| (500, vec![], "{}".into()));
        let lr = Lrclib::new(&srv.base, d.join("cache"));
        assert!(matches!(lr.find(&other, &q(100.0), false), Outcome::Later(Failure::Busy(500))));
        assert_eq!(lr.cache.get(&other), None, "a failure isn't kept");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn saving_beside_never_overwrites() {
        let d = tmp("beside");
        let song = d.join("a.flac");
        fs::write(&song, b"x").unwrap();
        save_beside(&song, &Answer::Synced("[00:01.00]A".into()));
        assert_eq!(fs::read_to_string(d.join("a.lrc")).unwrap(), "[00:01.00]A\n");
        save_beside(&song, &Answer::Synced("[00:01.00]B".into()));
        assert_eq!(fs::read_to_string(d.join("a.lrc")).unwrap(), "[00:01.00]A\n", "kept");
        let other = d.join("b.flac");
        fs::write(&other, b"x").unwrap();
        save_beside(&other, &Answer::Plain("Plain".into()));
        assert_eq!(fs::read_to_string(d.join("b.txt")).unwrap(), "Plain\n");
        save_beside(&other, &Answer::Synced("[00:01.00]S".into()));
        assert!(!d.join("b.lrc").exists(), "a .txt beside it already holds lyrics");
        save_beside(&d.join("c.flac"), &Answer::Instrumental);
        assert!(!d.join("c.lrc").exists() && !d.join("c.txt").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn query_of_a_local_song() {
        let t = crate::local::LocalTrack {
            path: "/m/a.flac".into(), mtime: 1, size: 1, title: " T ".into(), artist: "A".into(), album: "Folder".into(),
            album_fallback: true, album_artist: "A".into(), album_artist_tagged: false, compilation: false, track: None,
            disc: None, year: None, genre: None, duration: 12.4, cover: None,
        };
        let mut s = t.to_song();
        assert_eq!(Query::of(&s), Some(Query { title: "T".into(), artist: "A".into(), album: None, duration: 12.4 }));
        s.album_is_fallback = false;
        assert_eq!(Query::of(&s).unwrap().album.as_deref(), Some("Folder"));
        assert_eq!(Query::of(&s).unwrap().seconds(), Some(12));
        s.artist = String::new();
        assert_eq!(Query::of(&s), None);
    }
}
