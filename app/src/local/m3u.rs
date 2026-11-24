//! `.m3u8` playlists: writing the local songs of a playlist, and reading one back.
//!
//! Written as extended M3U in UTF-8: `#EXTM3U`, then per song `#EXTINF:<seconds>,<artist> -
//! <title>` and its absolute path. Apple Music songs have no file, so they are left out (the
//! caller says how many).
//!
//! Read leniently: `#` lines are comments (`#EXTINF` gives a title), a line is a path (absolute,
//! or relative to the playlist's folder, `\` taken as `/` off Windows) or a `file://` URL
//! (percent-decoded); web URLs are skipped. Files over `MAX_BYTES` or entries past
//! `MAX_ENTRIES` are not read ("small" playlists only: it runs on the UI thread).

use std::path::{Path, PathBuf};

use crate::models::Song;

/// Largest playlist file read, bytes.
pub const MAX_BYTES: u64 = 1 << 20;
/// Most entries taken from one playlist.
pub const MAX_ENTRIES: usize = 5000;

/// The `.m3u8` text for the local songs among `songs`, and how many were written.
pub fn write(songs: &[Song]) -> (String, usize) {
    let mut out = String::from("#EXTM3U\n");
    let mut n = 0;
    for s in songs {
        let Some(path) = s.local_path() else { continue };
        let secs = if s.duration > 0.0 { s.duration.round() as i64 } else { -1 };
        let name = if s.artist.trim().is_empty() { one_line(&s.title) } else { format!("{} - {}", one_line(&s.artist), one_line(&s.title)) };
        out.push_str(&format!("#EXTINF:{secs},{name}\n{}\n", one_line(&path.to_string_lossy())));
        n += 1;
    }
    (out, n)
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

/// The files `text` lists, in order (see the module docs). `dir`: the playlist's folder.
pub fn parse(text: &str, dir: &Path) -> Vec<PathBuf> {
    let text = text.trim_start_matches('\u{feff}');
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = if let Some(rest) = line.strip_prefix("file://") {
            // file:///abs/path or file://localhost/abs/path
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            PathBuf::from(percent_decode(rest))
        } else if line.contains("://") {
            continue;
        } else {
            let p = if cfg!(windows) { line.to_string() } else { line.replace('\\', "/") };
            let p = PathBuf::from(p);
            if p.is_absolute() { p } else { dir.join(p) }
        };
        out.push(normalize(&path));
        if out.len() >= MAX_ENTRIES {
            break;
        }
    }
    out
}

/// `a/./b/../c` → `a/c`, without touching the disk (the file may sit behind a symlink).
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && s.is_char_boundary(i + 3)
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str, artist: &str, duration: f64) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            album_is_fallback: false,
            duration,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: Vec::new(),
        }
    }

    #[test]
    fn writes_local_songs_only() {
        let (text, n) = write(&[
            song("file:///m/a b.flac", "Song A", "Ann", 61.4),
            song("123", "Apple", "X", 100.0),
            song("file:///m/c.mp3", "Line\nbreak", "", 0.0),
        ]);
        assert_eq!(n, 2);
        assert_eq!(text, "#EXTM3U\n#EXTINF:61,Ann - Song A\n/m/a b.flac\n#EXTINF:-1,Line break\n/m/c.mp3\n");
    }

    #[test]
    fn reads_paths_urls_and_relative_entries() {
        let text = "\u{feff}#EXTM3U\n#EXTINF:61,Ann - A\n/m/a.flac\n\nsub/b.mp3\n../up/c.ogg\nfile:///m/d%20e.flac\nfile://localhost/m/f.flac\nhttps://example.invalid/x.mp3\n# a note\n";
        let got = parse(text, Path::new("/lists"));
        let want: Vec<PathBuf> = ["/m/a.flac", "/lists/sub/b.mp3", "/up/c.ogg", "/m/d e.flac", "/m/f.flac"].iter().map(PathBuf::from).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn a_written_playlist_reads_back() {
        let songs = [song("file:///m/one.flac", "One", "A", 10.0), song("file:///m/two%.flac", "Two", "B", 20.0)];
        let (text, _) = write(&songs);
        let back = parse(&text, Path::new("/elsewhere"));
        assert_eq!(back, [PathBuf::from("/m/one.flac"), PathBuf::from("/m/two%.flac")]);
    }

    #[test]
    fn long_playlists_are_capped() {
        let text: String = (0..MAX_ENTRIES + 10).map(|i| format!("/m/{i}.mp3\n")).collect();
        assert_eq!(parse(&text, Path::new("/")).len(), MAX_ENTRIES);
    }
}
