//! What Discord is told: the activity ("Listening to …") for what is playing, as a pure function
//! of the playback snapshot and the user's choices, so every rule can be tested without a socket.
//!
//! Shape (Discord's `SET_ACTIVITY`): `type` 2 (Listening), `details` = the title, `state` = the
//! artist, `assets` (the cover as a large image, the Uzumaki icon as the small one), `timestamps`
//! (start and end, so Discord draws a progress bar like Spotify's) and one button.

use serde_json::{Value, json};

use super::Prefs;

/// Discord's activity type for "Listening to …".
const LISTENING: u8 = 2;
/// The image keys uploaded to the Discord Application (docs/DISCORD.md): the Uzumaki icon.
pub const ICON_KEY: &str = "uzumaki";
/// Discord shows 2 to 128 characters per line.
const MAX_TEXT: usize = 128;
/// An external image URL longer than this is not accepted.
const MAX_IMAGE_URL: usize = 256;
/// A button's URL.
const MAX_BUTTON_URL: usize = 512;
/// Two timestamps this close are the same song at the same place: the clock drifts a little.
const DRIFT: u64 = 2;
/// The button's label.
pub const BUTTON: &str = "Open in Apple Music";
/// What the title line says when the title is hidden.
pub const HIDDEN_TITLE: &str = "Listening to music";

/// What is playing, as the app knows it. Built by `State::discord_snapshot`.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The album is only a stand-in (a local file's folder name): not shown.
    pub album_is_fallback: bool,
    /// The cover as an https URL Discord can fetch (Apple's, 512 px); None for a local file.
    pub artwork_url: Option<String>,
    /// The song's page on music.apple.com; None for a local file or a song Apple has no page for.
    pub song_url: Option<String>,
    pub playing: bool,
    /// Seconds into the song, and its length (0 when unknown).
    pub position: f64,
    pub duration: f64,
}

/// One activity, as sent. Timestamps are unix seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub details: String,
    pub state: Option<String>,
    pub large_image: String,
    pub large_text: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub button: Option<String>,
}

impl Activity {
    pub fn to_json(&self) -> Value {
        let mut assets = json!({"large_image": self.large_image});
        let mut put = |k: &str, v: &Option<String>| {
            if let Some(v) = v {
                assets[k] = json!(v);
            }
        };
        put("large_text", &self.large_text);
        put("small_image", &self.small_image);
        put("small_text", &self.small_text);
        let mut a = json!({"type": LISTENING, "details": self.details, "assets": assets});
        if let Some(s) = &self.state {
            a["state"] = json!(s);
        }
        if let Some(start) = self.start {
            let mut t = json!({"start": start});
            if let Some(end) = self.end {
                t["end"] = json!(end);
            }
            a["timestamps"] = t;
        }
        if let Some(url) = &self.button {
            a["buttons"] = json!([{"label": BUTTON, "url": url}]);
        }
        a
    }

    /// The same thing on show, give or take the clock's drift: not worth telling Discord again.
    pub fn same_as(&self, other: &Activity) -> bool {
        let near = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(a), Some(b)) => a.abs_diff(b) <= DRIFT,
            (a, b) => a == b,
        };
        let strip = |a: &Activity| Activity { start: None, end: None, ..a.clone() };
        strip(self) == strip(other) && near(self.start, other.start) && near(self.end, other.end)
    }
}

/// A line for Discord: trimmed, 2 to 128 characters (a one-character line is padded with a
/// no-break space, which Discord counts), None when empty.
fn line(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut out: String = s.chars().take(MAX_TEXT).collect();
    if out.chars().count() < 2 {
        out.push('\u{a0}');
    }
    Some(out)
}

fn https(url: &Option<String>, max: usize) -> Option<String> {
    url.as_ref().filter(|u| u.starts_with("https://") && u.len() <= max).cloned()
}

/// The activity to show for `snap` under `prefs`, `now` being unix seconds; None to show nothing
/// (nothing playing, or paused with "hide when paused").
pub fn build(snap: Option<&Snapshot>, prefs: &Prefs, now: u64) -> Option<Activity> {
    let snap = snap?;
    if !snap.playing && prefs.hide_when_paused {
        return None;
    }
    let hidden = prefs.hide_title;
    let details = if hidden { HIDDEN_TITLE.to_string() } else { line(&snap.title).unwrap_or_else(|| "Unknown song".into()) };
    let cover = if prefs.artwork { https(&snap.artwork_url, MAX_IMAGE_URL) } else { None };
    // The icon is the big picture when there is no cover, and then needs no small copy of itself.
    let (large_image, small_image) = match cover {
        Some(url) => (url, Some(ICON_KEY.to_string())),
        None => (ICON_KEY.to_string(), None),
    };
    let (start, end) = if snap.playing {
        let start = now.saturating_sub(snap.position.max(0.0) as u64);
        (Some(start), (snap.duration >= 1.0).then(|| start + snap.duration as u64))
    } else {
        (None, None)
    };
    // The link names the song, so it goes with the title.
    let button = if prefs.buttons && !hidden { https(&snap.song_url, MAX_BUTTON_URL) } else { None };
    let large_text = if hidden || snap.album_is_fallback { None } else { line(&snap.album) };
    Some(Activity {
        details,
        state: line(&snap.artist),
        large_image,
        large_text,
        small_text: small_image.as_ref().map(|_| if snap.playing { "Uzumaki" } else { "Uzumaki \u{b7} Paused" }.to_string()),
        small_image,
        start,
        end,
        button,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Snapshot {
        Snapshot {
            title: "Redbone".into(),
            artist: "Childish Gambino".into(),
            album: "\"Awaken, My Love!\"".into(),
            album_is_fallback: false,
            artwork_url: Some("https://is1-ssl.mzstatic.com/image/thumb/Music/x/512x512bb.jpg".into()),
            song_url: Some("https://music.apple.com/us/album/redbone/1161018478?i=1161018480".into()),
            playing: true,
            position: 30.4,
            duration: 327.0,
        }
    }

    fn on() -> Prefs {
        Prefs { enabled: true, ..Prefs::default() }
    }

    #[test]
    fn a_playing_song_is_a_listening_activity_with_a_progress_bar() {
        let a = build(Some(&song()), &on(), 1_000_000).unwrap();
        assert_eq!(
            a.to_json(),
            json!({
                "type": 2,
                "details": "Redbone",
                "state": "Childish Gambino",
                "assets": {
                    "large_image": "https://is1-ssl.mzstatic.com/image/thumb/Music/x/512x512bb.jpg",
                    "large_text": "\"Awaken, My Love!\"",
                    "small_image": "uzumaki",
                    "small_text": "Uzumaki"
                },
                "timestamps": {"start": 999_970, "end": 1_000_297},
                "buttons": [{"label": "Open in Apple Music", "url": "https://music.apple.com/us/album/redbone/1161018478?i=1161018480"}]
            })
        );
    }

    #[test]
    fn paused_hides_by_default_and_otherwise_shows_without_a_clock() {
        let mut s = song();
        s.playing = false;
        assert!(build(Some(&s), &on(), 1000).is_none(), "hide when paused is on by default");
        let shown = build(Some(&s), &Prefs { hide_when_paused: false, ..on() }, 1000).unwrap().to_json();
        assert!(shown.get("timestamps").is_none(), "a paused song has no running bar");
        assert_eq!(shown["assets"]["small_text"], "Uzumaki \u{b7} Paused");
        assert!(build(None, &on(), 1000).is_none(), "nothing playing, nothing shown");
    }

    #[test]
    fn a_local_file_has_title_and_artist_and_the_icon_only() {
        let mut s = song();
        s.artwork_url = None;
        s.song_url = None;
        s.album = "Downloads".into();
        s.album_is_fallback = true;
        let j = build(Some(&s), &on(), 1000).unwrap().to_json();
        assert_eq!(j["assets"], json!({"large_image": "uzumaki"}));
        assert!(j.get("buttons").is_none());
        assert_eq!((j["details"].as_str(), j["state"].as_str()), (Some("Redbone"), Some("Childish Gambino")));
    }

    #[test]
    fn the_choices_switch_the_cover_the_button_and_the_title_off() {
        let no_art = build(Some(&song()), &Prefs { artwork: false, ..on() }, 1000).unwrap().to_json();
        assert_eq!(no_art["assets"]["large_image"], "uzumaki");
        assert!(no_art["assets"].get("small_image").is_none());
        assert!(no_art["buttons"].is_array(), "the button is its own choice");
        let no_button = build(Some(&song()), &Prefs { buttons: false, ..on() }, 1000).unwrap().to_json();
        assert!(no_button.get("buttons").is_none());
        let private = build(Some(&song()), &Prefs { hide_title: true, ..on() }, 1000).unwrap().to_json();
        assert_eq!(private["details"], HIDDEN_TITLE);
        assert_eq!(private["state"], "Childish Gambino", "the artist stays");
        assert!(private.get("buttons").is_none(), "the link would name the song");
        assert!(private["assets"].get("large_text").is_none(), "so would the album");
    }

    #[test]
    fn text_is_cut_padded_and_only_https_images_go_out() {
        let mut s = song();
        s.title = "x".repeat(300);
        s.artist = "A".into();
        s.album = "  ".into();
        s.artwork_url = Some("file:///Users/me/cover.jpg".into());
        s.song_url = Some(format!("https://music.apple.com/{}", "a".repeat(600)));
        let j = build(Some(&s), &on(), 1000).unwrap().to_json();
        assert_eq!(j["details"].as_str().unwrap().chars().count(), 128);
        assert_eq!(j["state"], "A\u{a0}");
        assert!(j["assets"].get("large_text").is_none(), "a blank album is left out");
        assert_eq!(j["assets"]["large_image"], "uzumaki", "a file path is never sent");
        assert!(j.get("buttons").is_none(), "a URL over 512 characters is refused by Discord");
        s.artist = String::new();
        s.title = "  ".into();
        let j = build(Some(&s), &on(), 1000).unwrap().to_json();
        assert!(j.get("state").is_none());
        assert_eq!(j["details"], "Unknown song");
    }

    #[test]
    fn unknown_length_has_a_start_but_no_end_and_a_late_position_never_underflows() {
        let mut s = song();
        s.duration = 0.0;
        s.position = 5000.0;
        let j = build(Some(&s), &on(), 100).unwrap().to_json();
        assert_eq!(j["timestamps"], json!({"start": 0}));
    }

    #[test]
    fn drift_is_the_same_activity_a_seek_is_not() {
        let a = build(Some(&song()), &on(), 1000).unwrap();
        let drift = build(Some(&Snapshot { position: 31.9, ..song() }), &on(), 1000).unwrap();
        assert!(a.same_as(&drift));
        let seek = build(Some(&Snapshot { position: 120.0, ..song() }), &on(), 1000).unwrap();
        assert!(!a.same_as(&seek));
        let other = build(Some(&Snapshot { title: "Zombie".into(), ..song() }), &on(), 1000).unwrap();
        assert!(!a.same_as(&other));
    }
}
