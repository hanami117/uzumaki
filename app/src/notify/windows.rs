//! Windows backend: a toast notification (WinRT `Windows.UI.Notifications`, through the
//! `windows` crate Rune already builds). Always called from a worker thread.
//!
//! The toast is a small XML text ([`toast_xml`]): the title, "Artist — Album", and the cover as
//! the toast's picture (a local file: a program with no package identity can't use web
//! pictures). It is silent, carries one tag, so each song replaces the last one (also in the
//! notification centre), and is gone after a restart of Windows.
//!
//! The toast is Uzumaki's through the app id (`win::set_app_id`, registered with its name and
//! icon by `win::register_app_id`). Windows itself may hold a toast back (Do not disturb, a
//! full-screen app): it then goes straight to the notification centre.

use std::sync::Mutex;

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::core::HSTRING;

use super::{Backend, Note};

/// Every toast of Uzumaki's has this tag in this group: a new one replaces the one before.
const TAG: &str = "now-playing";
const GROUP: &str = "uzumaki";

pub(super) struct Sys {
    /// Made at the first notification; dropped after a failure (Explorer restarted) so the next
    /// one makes a fresh one.
    notifier: Mutex<Option<ToastNotifier>>,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 has no control characters but tab and the line ends.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// A local file as the `file:///C:/…` address the toast's picture wants.
pub(super) fn file_url(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let mut out = String::from("file:///");
    for b in text.trim_start_matches('/').bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The toast: the title in bold, the body under it, the cover at the left, no sound.
pub(super) fn toast_xml(note: &Note) -> String {
    let mut xml = format!("<toast><visual><binding template=\"ToastGeneric\"><text>{}</text>", escape(&note.title));
    if !note.body.is_empty() {
        xml.push_str(&format!("<text>{}</text>", escape(&note.body)));
    }
    if let Some(cover) = &note.cover {
        xml.push_str(&format!("<image placement=\"appLogoOverride\" src=\"{}\"/>", escape(&file_url(cover))));
    }
    xml.push_str("</binding></visual><audio silent=\"true\"/></toast>");
    xml
}

fn toast(note: &Note) -> windows::core::Result<ToastNotification> {
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(toast_xml(note)))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.SetTag(&HSTRING::from(TAG))?;
    toast.SetGroup(&HSTRING::from(GROUP))?;
    // Gone after a restart of Windows (older Windows 10: no such setting, and the toast shows all
    // the same).
    let _ = toast.SetExpiresOnReboot(true);
    Ok(toast)
}

impl Sys {
    pub fn new() -> Self {
        Sys { notifier: Mutex::new(None) }
    }
}

impl Backend for Sys {
    fn send(&self, note: &Note) {
        let mut slot = self.notifier.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            // The app id's name and icon, for a copy with no installer; then the notifier.
            crate::win::register_app_id();
            match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(crate::APP_ID)) {
                Ok(n) => *slot = Some(n),
                Err(e) => return eprintln!("uzumaki: no notifications: {e}"),
            }
        }
        let Some(notifier) = slot.as_ref() else { return };
        if let Err(e) = toast(note).and_then(|t| notifier.Show(&t)) {
            eprintln!("uzumaki: a notification wasn't shown: {e}");
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_toast_has_the_song_the_cover_and_no_sound() {
        let note = Note { title: "Rock & Roll <live>".into(), body: "Led Zeppelin \u{2014} IV".into(), cover: Some(PathBuf::from(r"C:\Users\A B\AppData\Local\Uzumaki\Cache\art\00ff")) };
        assert_eq!(
            toast_xml(&note),
            "<toast><visual><binding template=\"ToastGeneric\"><text>Rock &amp; Roll &lt;live&gt;</text><text>Led Zeppelin \u{2014} IV</text>\
             <image placement=\"appLogoOverride\" src=\"file:///C:/Users/A%20B/AppData/Local/Uzumaki/Cache/art/00ff\"/>\
             </binding></visual><audio silent=\"true\"/></toast>"
        );
        let bare = Note { title: "Song".into(), body: String::new(), cover: None };
        assert_eq!(toast_xml(&bare), "<toast><visual><binding template=\"ToastGeneric\"><text>Song</text></binding></visual><audio silent=\"true\"/></toast>");
    }

    #[test]
    fn a_path_becomes_a_file_address() {
        assert_eq!(file_url(Path::new(r"C:\Music\è.jpg")), "file:///C:/Music/%C3%A8.jpg");
        assert_eq!(file_url(Path::new(r"\\?\C:\a b\c.png")), "file:///C:/a%20b/c.png");
    }

    /// The real objects on this machine: the XML loads and a toast is made from it (not shown).
    #[test]
    fn windows_takes_the_toast() {
        let note = Note { title: "Song".into(), body: "Artist \u{2014} Album".into(), cover: None };
        let t = toast(&note).expect("a toast from the XML");
        assert_eq!(t.Tag().unwrap().to_string(), TAG);
    }
}
