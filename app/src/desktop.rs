//! Small desktop integrations: links to music.apple.com and "Show in Folder".

use std::path::Path;

use crate::models::Song;

/// The song's page on music.apple.com, in the user's storefront ("it", else "us").
pub fn apple_music_url(song: &Song, storefront: Option<&str>) -> Option<String> {
    let id = song.catalog_ref()?;
    Some(format!("https://music.apple.com/{}/song/{id}", storefront.unwrap_or("us")))
}

/// Opens the file manager on `path`'s folder with the file selected (the freedesktop
/// FileManager1 `ShowItems` call: Dolphin, Nautilus, Nemo, Thunar…), else just the folder.
/// Runs on a thread: D-Bus activation can take a moment. On macOS, Finder does it (`open -R`).
pub fn show_in_folder(path: &Path) {
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
        return;
    }
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        if show_items(&path).is_err() {
            if let Some(dir) = path.parent() {
                let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
            }
        }
    });
}

fn show_items(path: &Path) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    conn.call_method(
        Some("org.freedesktop.FileManager1"),
        "/org/freedesktop/FileManager1",
        Some("org.freedesktop.FileManager1"),
        "ShowItems",
        &(vec![file_uri(path)], ""),
    )?;
    Ok(())
}

/// `file://` URI of an absolute path, percent-encoding everything but unreserved characters
/// and '/'.
pub fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.as_os_str().as_encoded_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(*b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_escape_spaces_and_non_ascii() {
        assert_eq!(file_uri(Path::new("/music/Telu/Late Train (live) è.flac")), "file:///music/Telu/Late%20Train%20%28live%29%20%C3%A8.flac");
    }

    #[test]
    fn apple_music_links_need_a_catalog_id() {
        let mut s = Song {
            id: "i.abc".into(),
            catalog_id: Some("1440857781".into()),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            album_is_fallback: false,
            duration: 0.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: Vec::new(),
        };
        assert_eq!(apple_music_url(&s, Some("it")).as_deref(), Some("https://music.apple.com/it/song/1440857781"));
        s.catalog_id = None;
        assert_eq!(apple_music_url(&s, Some("it")), None, "an unmatched library song has no page");
        s.id = "file:///m/a.flac".into();
        assert_eq!(apple_music_url(&s, None), None);
    }
}
