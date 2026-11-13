//! Lyrics of the playing song, from Apple (`/songs/{id}/syllable-lyrics`, then `/lyrics`).
//!
//! Only an answer from Apple is kept: lyrics, or "none". An error (the network, a 401/429/5xx, the
//! page reloading, a reply that never comes) is never taken for "none": the song is asked again
//! with a backoff while it is current, and again whenever it starts. "None" is asked again too:
//! once soon (Apple also answers 404 when the request went without the user's token, which is
//! "none" in the same words), then after `NONE_KEEP` when the song plays again.
//!
//! The answers stay in `State::lyrics` (the views read it); `State::lyrics_fetch` keeps what is
//! in flight and when to ask again. Every request is keyed by the catalog id, so a reply that
//! lands after the song changed is stored for its own song.

use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Load, Msg, State};
use crate::lyrics::Lyrics;
use crate::models::Song;

/// A request without a reply after this is taken as lost (the page reloaded, say).
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// The first retry after a failure; it doubles up to `MAX_BACKOFF`.
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// Failures in a row before the views say "Couldn't load lyrics" instead of "Loading lyrics…".
const SHOW_FAILED_AFTER: u32 = 3;
/// A first "none" is checked once more after this, while the song plays.
const NONE_RECHECK: Duration = Duration::from_secs(30);
/// Then it holds this long; after that the song is asked again when it plays again.
const NONE_KEEP: Duration = Duration::from_secs(30 * 60);
/// More songs than this in memory: start over.
const MAX_SONGS: usize = 200;

/// The bookkeeping of one song's lyrics requests.
#[derive(Debug, Default, Clone)]
pub struct Fetch {
    /// When the request in flight was sent; None when none is.
    sent: Option<Instant>,
    /// Failed attempts in a row.
    failures: u32,
    /// "None" answers in a row.
    nones: u32,
    /// When to ask again (a failure's backoff, or a "none" going stale).
    again: Option<Instant>,
}

/// What one reply says.
#[derive(Debug, PartialEq)]
pub enum Answer {
    Lyrics(Lyrics),
    /// Apple has none (404, or no TTML in the reply).
    None,
    /// No answer: try again later.
    Failed(String),
}

/// Reads a lyrics reply. MusicKit hands Apple's error bodies back as a reply
/// (`{"errors":[{"status":"404","code":"40403",…}]}`), or throws (`[mk-007] NOT_FOUND`).
pub fn answer(r: Result<Value, String>) -> Answer {
    let v = match r {
        Ok(v) => v,
        Err(e) if e.contains("NOT_FOUND") => return Answer::None,
        Err(e) => return Answer::Failed(e),
    };
    if let Some(ttml) = v["data"][0]["attributes"]["ttml"].as_str() {
        return match crate::lyrics::parse_ttml(ttml) {
            Some(l) => Answer::Lyrics(l),
            None => Answer::Failed("unreadable TTML".into()),
        };
    }
    if let Some(err) = v["errors"].get(0) {
        let status = err["status"].as_str().unwrap_or("?");
        return match status {
            "404" => Answer::None,
            _ => Answer::Failed(format!("HTTP {status} {}", err["code"].as_str().unwrap_or("")).trim_end().to_string()),
        };
    }
    if v["data"].is_array() { Answer::None } else { Answer::Failed("empty reply".into()) }
}

/// The message for a reply: word-timed lyrics first; anything but lyrics there asks for the
/// line-timed ones (songs with only line timing, or a hiccup), whose answer decides.
pub fn reply_msg(id: String, words: bool, r: Result<Value, String>) -> Msg {
    match (answer(r), words) {
        (Answer::Lyrics(l), _) => Msg::Lyrics { id, result: Ok(Some(l)) },
        (_, true) => Msg::LyricsRetry { id },
        (Answer::None, false) => Msg::Lyrics { id, result: Ok(None) },
        (Answer::Failed(e), false) => Msg::Lyrics { id, result: Err(e) },
    }
}

fn backoff(failures: u32) -> Duration {
    FIRST_BACKOFF.saturating_mul(1 << failures.saturating_sub(1).min(16)).min(MAX_BACKOFF)
}

/// The id lyrics are asked by: the catalog id (a library song's `playParams.catalogId`), or the
/// id itself when it is a catalog one. None for local files and library songs Apple didn't match.
pub fn key(song: &Song) -> Option<&str> {
    match &song.catalog_id {
        Some(c) => Some(c),
        None if song.is_local() || song.id.starts_with("i.") => None,
        None => Some(&song.id),
    }
}

impl State {
    /// The lyrics of `song` as loaded so far (None: never asked, e.g. a local file or a library
    /// song Apple didn't match).
    pub fn lyrics_of(&self, song: &Song) -> Option<&Load<Option<Lyrics>>> {
        self.lyrics.get(key(song)?)
    }

    /// The song starts or shows: asks for its lyrics unless they are known, on their way, or a
    /// "none" still fresh. A failed song is asked again at once.
    pub(super) fn want_lyrics(&mut self, song: &Song) {
        let Some(id) = key(song).map(str::to_string) else { return };
        if !self.session.authorized {
            return;
        }
        let now = crate::clock::now();
        let fetch = self.lyrics_fetch.get(&id);
        let ask = match self.lyrics.get(&id) {
            None => true,
            Some(Load::Ready(Some(_))) => false,
            Some(Load::Ready(None)) => fetch.and_then(|f| f.again).is_some_and(|t| now >= t),
            // Loading / Failed: unless a request is in flight and not overdue.
            Some(_) => !fetch.and_then(|f| f.sent).is_some_and(|t| now.saturating_duration_since(t) < REPLY_TIMEOUT),
        };
        if ask {
            eprintln!("uzumaki: lyrics source {} -> catalog {id}", song.id);
            self.ask_lyrics(id);
        }
    }

    /// While a song plays (each time report): asks again for its lyrics when a retry is due, and
    /// takes a request that never got a reply as failed.
    pub(super) fn lyrics_due(&mut self) {
        let Some(id) = self.playback.song.as_ref().and_then(key).map(str::to_string) else { return };
        if !self.session.authorized {
            return;
        }
        let now = crate::clock::now();
        let Some(f) = self.lyrics_fetch.get(&id) else { return };
        match f.sent {
            Some(t) if now.saturating_duration_since(t) >= REPLY_TIMEOUT => {
                self.lyrics_failed(id.clone(), "no reply".into());
            }
            Some(_) => return,
            None => {}
        }
        let f = &self.lyrics_fetch[&id];
        let still = !matches!(self.lyrics.get(&id), Some(Load::Ready(Some(_))));
        // A stale "none" waits for the song to start again; a failure or the first "none" is
        // checked while it plays.
        let fresh_none = matches!(self.lyrics.get(&id), Some(Load::Ready(None))) && f.nones > 1;
        if still && !fresh_none && f.sent.is_none() && f.again.is_some_and(|t| now >= t) {
            self.ask_lyrics(id);
        }
    }

    fn ask_lyrics(&mut self, id: String) {
        if self.lyrics.len() > MAX_SONGS {
            self.lyrics.clear();
            self.lyrics_fetch.clear();
        }
        // A known "none" stays on screen while it is checked again; anything else is loading,
        // until failures say otherwise.
        let fetch = self.lyrics_fetch.entry(id.clone()).or_default();
        fetch.sent = Some(crate::clock::now());
        let failures = fetch.failures;
        match self.lyrics.get(&id) {
            Some(Load::Ready(_)) => {}
            Some(Load::Failed(_)) if failures >= SHOW_FAILED_AFTER => {}
            _ => {
                self.lyrics.insert(id.clone(), Load::Loading);
            }
        }
        self.request_lyrics(id, true);
    }

    /// Word-timed ("syllable") lyrics first, for the per-word highlight; then the line-timed ones.
    pub(super) fn request_lyrics(&self, id: String, words: bool) {
        let kind = if words { "syllable-lyrics" } else { "lyrics" };
        let path = format!("/v1/catalog/{{{{storefrontId}}}}/songs/{id}/{kind}");
        #[cfg(test)]
        self.sent.borrow_mut().push(format!("{path} {{}}"));
        if let Some(b) = &self.bridge {
            b.api(&path, serde_json::json!({}), move |r| Some(reply_msg(id, words, r)));
        }
    }

    /// A reply (`Msg::Lyrics`). Lyrics replace anything; "none" and failures never replace
    /// lyrics already shown (a late reply of an older request).
    pub(super) fn lyrics_reply(&mut self, id: String, result: Result<Option<Lyrics>, String>) {
        let now = crate::clock::now();
        match result {
            Ok(Some(l)) => {
                self.lyrics_fetch.remove(&id);
                self.lyrics.insert(id, Load::Ready(Some(l)));
            }
            Ok(None) => {
                let f = self.lyrics_fetch.entry(id.clone()).or_default();
                f.sent = None;
                f.failures = 0;
                f.nones += 1;
                f.again = Some(now + if f.nones == 1 { NONE_RECHECK } else { NONE_KEEP });
                if f.nones <= 2 {
                    // Apple says 404 both for a song without lyrics and for a request without the
                    // user's token: the bridge log says which (`user token yes/no`).
                    let next = if f.nones == 1 { "checking again in 30 s" } else { "again" };
                    eprintln!("uzumaki: lyrics {id}: Apple says none ({next}; signed in: {})", self.session.authorized);
                }
                if !matches!(self.lyrics.get(&id), Some(Load::Ready(Some(_)))) {
                    self.lyrics.insert(id, Load::Ready(None));
                }
            }
            Err(e) => self.lyrics_failed(id, e),
        }
    }

    fn lyrics_failed(&mut self, id: String, e: String) {
        let f = self.lyrics_fetch.entry(id.clone()).or_default();
        f.sent = None;
        f.failures += 1;
        let wait = backoff(f.failures);
        f.again = Some(crate::clock::now() + wait);
        let failures = f.failures;
        eprintln!("uzumaki: lyrics {id}: {e} (failure {failures}, asking again in {} s)", wait.as_secs());
        match self.lyrics.get(&id) {
            Some(Load::Ready(_)) => {} // an answer already shows; keep it
            _ if failures >= SHOW_FAILED_AFTER => {
                self.lyrics.insert(id, Load::Failed(e));
            }
            _ => {
                self.lyrics.insert(id, Load::Loading);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TTML: &str = "<tt xmlns='http://www.w3.org/ns/ttml' xmlns:itunes='http://music.apple.com/lyric-ttml-internal' itunes:timing='Line'><body><div><p begin='1.0' end='2.0'>hello</p></div></body></tt>";

    #[test]
    fn errors_are_never_none() {
        let lyrics = json!({"data": [{"attributes": {"ttml": TTML}}]});
        assert!(matches!(answer(Ok(lyrics)), Answer::Lyrics(l) if l.lines.len() == 1));
        assert!(matches!(answer(Ok(json!({"data": [{"attributes": {"ttml": "<broken"}}]}))), Answer::Failed(_)));
        let err = |status: &str| Ok(json!({"errors": [{"status": status, "code": "40403"}]}));
        assert_eq!(answer(err("404")), Answer::None, "Apple's own 'No related resources'");
        assert_eq!(answer(Ok(json!({"data": []}))), Answer::None);
        assert_eq!(answer(Err("[mk-007] NOT_FOUND".into())), Answer::None);
        for status in ["401", "403", "429", "500", "503"] {
            assert!(matches!(answer(err(status)), Answer::Failed(e) if e.contains(status)), "{status}");
        }
        for e in ["MusicKit not ready yet (page still loading)", "Failed to fetch", "[mk-000] UNAUTHORIZED_ERROR"] {
            assert!(matches!(answer(Err(e.into())), Answer::Failed(_)), "{e}");
        }
        assert!(matches!(answer(Ok(Value::Null)), Answer::Failed(_)), "a null reply is no answer");
    }

    #[test]
    fn word_timing_falls_back_to_lines_and_lines_decide() {
        let none = || Ok(json!({"errors": [{"status": "404"}]}));
        assert!(matches!(reply_msg("1".into(), true, none()), Msg::LyricsRetry { .. }));
        assert!(matches!(reply_msg("1".into(), true, Err("x".into())), Msg::LyricsRetry { .. }));
        assert!(matches!(reply_msg("1".into(), false, none()), Msg::Lyrics { result: Ok(None), .. }));
        assert!(matches!(reply_msg("1".into(), false, Err("x".into())), Msg::Lyrics { result: Err(_), .. }));
    }

    #[test]
    fn the_backoff_doubles_up_to_five_minutes() {
        let secs: Vec<u64> = (1..=10).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    }
}
