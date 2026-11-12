//! Linux backend: `org.freedesktop.Notifications.Notify` over the session bus (zbus blocking
//! API, always called from a worker thread). Each song replaces the previous notification
//! (`replaces_id`). With no notification server nothing happens and nothing is printed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::{Backend, Note};

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// A hint's value (the D-Bus variant).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Hint {
    Str(String),
    Bool(bool),
    Byte(u8),
}

/// The arguments of `Notify`, before they go on the bus.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Request {
    pub app_name: &'static str,
    pub replaces_id: u32,
    pub app_icon: &'static str,
    pub summary: String,
    pub body: String,
    pub hints: Vec<(&'static str, Hint)>,
    pub expire_timeout: i32,
}

/// Builds the message for `note`; `replaces` is the id of the previous notification (0: none).
pub(super) fn build(note: &Note, replaces: u32) -> Request {
    let mut hints = vec![
        ("desktop-entry", Hint::Str(crate::APP_ID.to_string())),
        // Low urgency: a song change is not an alert.
        ("urgency", Hint::Byte(0)),
        // Not kept in a notification history that would fill with every song.
        ("transient", Hint::Bool(true)),
    ];
    if let Some(cover) = &note.cover {
        hints.push(("image-path", Hint::Str(path_str(cover))));
    }
    Request {
        app_name: "Uzumaki",
        replaces_id: replaces,
        app_icon: crate::APP_ID,
        summary: note.title.clone(),
        body: note.body.clone(),
        hints,
        expire_timeout: -1,
    }
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

fn hint_value(h: &Hint) -> Value<'_> {
    match h {
        Hint::Str(s) => Value::from(s.as_str()),
        Hint::Bool(b) => Value::from(*b),
        Hint::Byte(b) => Value::from(*b),
    }
}

pub struct Sys {
    /// The bus connection and the id of the notification on screen, kept between songs.
    state: Mutex<Option<(Connection, u32)>>,
}

impl Sys {
    pub fn new() -> Self {
        Self { state: Mutex::new(None) }
    }

    fn notify(conn: &Connection, req: &Request) -> zbus::Result<u32> {
        let hints: HashMap<&str, Value<'_>> = req.hints.iter().map(|(k, v)| (*k, hint_value(v))).collect();
        let actions: Vec<&str> = Vec::new();
        let reply = conn.call_method(
            Some(DEST),
            PATH,
            Some(DEST),
            "Notify",
            &(req.app_name, req.replaces_id, req.app_icon, req.summary.as_str(), req.body.as_str(), actions, hints, req.expire_timeout),
        )?;
        Ok(reply.body().deserialize::<u32>()?)
    }
}

impl Backend for Sys {
    fn send(&self, note: &Note) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, last) = match state.take() {
            Some(s) => s,
            None => match Connection::session() {
                Ok(c) => (c, 0),
                Err(_) => return,
            },
        };
        match Self::notify(&conn, &build(note, last)) {
            Ok(id) => *state = Some((conn, id)),
            // No server (or it went away): start from a fresh connection next time.
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn note(cover: Option<&str>) -> Note {
        Note { title: "Song".into(), body: "Artist \u{2014} Album".into(), cover: cover.map(PathBuf::from) }
    }

    #[test]
    fn the_message_has_the_app_the_text_and_the_cover() {
        let r = build(&note(Some("/cache/abc")), 0);
        assert_eq!(r.app_name, "Uzumaki");
        assert_eq!(r.app_icon, "io.github.hanami117.uzumaki");
        assert_eq!((r.summary.as_str(), r.body.as_str()), ("Song", "Artist \u{2014} Album"));
        assert_eq!(r.replaces_id, 0);
        assert_eq!(r.expire_timeout, -1);
        let hint = |k: &str| r.hints.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(hint("image-path"), Some(Hint::Str("/cache/abc".into())));
        assert_eq!(hint("desktop-entry"), Some(Hint::Str("io.github.hanami117.uzumaki".into())));
        assert_eq!(hint("urgency"), Some(Hint::Byte(0)));
        assert_eq!(hint("transient"), Some(Hint::Bool(true)));
    }

    #[test]
    fn no_cover_means_no_image_hint_and_the_next_song_replaces_the_last() {
        let r = build(&note(None), 42);
        assert!(r.hints.iter().all(|(k, _)| *k != "image-path"));
        assert_eq!(r.replaces_id, 42);
    }

    #[test]
    fn hints_convert_to_the_right_variants() {
        assert_eq!(hint_value(&Hint::Byte(0)).value_signature().to_string(), "y");
        assert_eq!(hint_value(&Hint::Bool(true)).value_signature().to_string(), "b");
        assert_eq!(hint_value(&Hint::Str("x".into())).value_signature().to_string(), "s");
    }
}
