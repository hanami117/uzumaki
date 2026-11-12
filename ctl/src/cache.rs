//! The app's library cache on disk, for `uzu playlists` & co. when Uzumaki isn't running.
//! Read-only; the format is the app's (`app/src/models.rs`), only the fields we show are parsed.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use uzumaki_ctl::{CollectionInfo, Kind, SongInfo};

#[derive(Deserialize)]
struct File<T> {
    data: T,
}

#[derive(Deserialize)]
struct Collection {
    id: String,
    title: String,
    #[serde(default)]
    subtitle: String,
    track_count: Option<u32>,
    #[serde(default)]
    favorites: bool,
    #[serde(default)]
    editable: bool,
    year: Option<String>,
    #[serde(default)]
    tracks: Vec<Song>,
}

/// Only the song count of a cached collection (its songs aren't parsed).
#[derive(Deserialize)]
struct Counted {
    #[serde(default)]
    tracks: Vec<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct Song {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: String,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    explicit: bool,
    #[serde(default)]
    playable: bool,
}

/// `$XDG_CACHE_HOME/uzumaki/library`, else `~/.cache/uzumaki/library`.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("uzumaki").join("library"))
}

/// The app's settings file, `$XDG_CONFIG_HOME/uzumaki/settings.json`, else
/// `~/.config/uzumaki/settings.json` (as `app/src/settings.rs` finds it).
pub fn default_settings() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("uzumaki").join("settings.json"))
}

#[derive(Deserialize)]
struct Settings {
    #[serde(default)]
    pins: Vec<Pin>,
}

#[derive(Deserialize)]
struct Pin {
    kind: String,
    id: String,
}

/// The ids of the playlists pinned in the app's sidebar, in pin order (albums, artists and
/// stations left out). Empty when the file is missing or unreadable.
pub fn pinned_playlists(settings: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(settings) else { return Vec::new() };
    let pins = serde_json::from_str::<Settings>(&text).map(|s| s.pins).unwrap_or_default();
    pins.into_iter().filter(|p| p.kind == "Playlist").map(|p| p.id).collect()
}

/// Marks the pinned playlists with their pin rank and puts them first, as the app does.
pub fn apply_pins(items: &mut [CollectionInfo], pins: &[String]) {
    for c in items.iter_mut() {
        c.pinned = pins.iter().position(|id| *id == c.id).map(|i| i as u32);
    }
    uzumaki_ctl::pinned_first(items);
}

fn info(c: &Collection, kind: Kind) -> CollectionInfo {
    CollectionInfo {
        kind,
        id: c.id.clone(),
        title: c.title.clone(),
        subtitle: c.subtitle.clone(),
        count: c.track_count.or((!c.tracks.is_empty()).then_some(c.tracks.len() as u32)),
        favorites: c.favorites,
        editable: c.editable,
        year: c.year.clone(),
        pinned: None,
    }
}

fn song(s: &Song) -> SongInfo {
    SongInfo {
        id: s.id.clone(),
        title: s.title.clone(),
        artist: s.artist.clone(),
        album: s.album.clone(),
        duration: s.duration,
        explicit: s.explicit,
        local: s.id.starts_with("file://"),
        playable: s.playable,
    }
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str::<File<T>>(&text).map(|f| f.data).map_err(|e| format!("{}: {e}", path.display()))
}

/// Playlists (Favourite Songs first, then file order) or albums (file order).
/// `Offline` then puts the pinned playlists first (`apply_pins`).
pub fn collections(dir: &Path, kind: Kind) -> Result<Vec<CollectionInfo>, String> {
    let file = if kind == Kind::Playlist { "playlists.json" } else { "albums.json" };
    let all: Vec<Collection> = read(&dir.join(file))?;
    let mut items: Vec<CollectionInfo> = all.iter().map(|c| info(c, kind)).collect();
    if kind == Kind::Playlist {
        items.sort_by_key(|c| !c.favorites); // stable: the rest keep file order
        // Library playlists come without a count; the app caches every playlist's songs.
        for c in items.iter_mut().filter(|c| c.count.is_none()) {
            c.count = cached_count(dir, &c.id);
        }
    }
    Ok(items)
}

/// How many songs the app cached for `id` (none when the file is missing or empty).
fn cached_count(dir: &Path, id: &str) -> Option<u32> {
    let c: Counted = read(&dir.join("collections").join(collection_file(id))).ok()?;
    (!c.tracks.is_empty()).then_some(c.tracks.len() as u32)
}

/// The file the app caches a collection's songs in: the id with odd characters replaced by `_`.
pub fn collection_file(id: &str) -> String {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    format!("{safe}.json")
}

pub fn tracks(dir: &Path, kind: Kind, id: &str) -> Result<(CollectionInfo, Vec<SongInfo>), String> {
    let path = dir.join("collections").join(collection_file(id));
    if !path.exists() {
        return Err("its songs aren't in the cache (start Uzumaki: uzu start)".into());
    }
    let c: Collection = read(&path)?;
    Ok((info(&c, kind), c.tracks.iter().map(song).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_match_the_app() {
        assert_eq!(collection_file("p.Abc-1_x"), "p.Abc-1_x.json");
        assert_eq!(collection_file("file:///a b"), "file____a_b.json");
    }

    #[test]
    fn reads_a_cache_dir() {
        let dir = std::env::temp_dir().join(format!("uzu-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("collections")).unwrap();
        std::fs::write(
            dir.join("playlists.json"),
            r#"{"saved_at":1,"data":[{"id":"p.1","title":"Mine","kind":"Playlist","track_count":null,"tracks":[]},{"id":"p.2","title":"Favourite Songs","favorites":true,"track_count":3,"tracks":[]}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("collections").join("p.1.json"),
            r#"{"saved_at":1,"data":{"id":"p.1","title":"Mine","tracks":[{"id":"file:///x.mp3","title":"T","artist":"A","album":"B","duration":61.0,"explicit":false,"playable":true}]}}"#,
        )
        .unwrap();
        let lists = collections(&dir, Kind::Playlist).unwrap();
        assert_eq!(lists[0].title, "Favourite Songs");
        assert_eq!(lists[0].count, Some(3));
        assert_eq!(lists[1].count, Some(1), "a playlist without a count gets its cached songs' count");
        let (c, songs) = tracks(&dir, Kind::Playlist, "p.1").unwrap();
        assert_eq!((c.count, songs[0].local, songs[0].duration), (Some(1), true, 61.0));
        assert!(tracks(&dir, Kind::Playlist, "p.9").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinned_playlists_come_first_in_pin_order() {
        let dir = std::env::temp_dir().join(format!("uzu-pins-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(
            &file,
            r#"{"pins":[{"kind":"Album","id":"l.a","title":"A"},{"kind":"Playlist","id":"p.3","title":"Three","artwork":null},{"kind":"Station","id":"ra.1","title":"S"},{"kind":"Playlist","id":"p.1","title":"One"}],"sidebar_collapsed":false}"#,
        )
        .unwrap();
        let pins = pinned_playlists(&file);
        assert_eq!(pins, ["p.3", "p.1"], "only playlists, in pin order");
        let item = |id: &str| CollectionInfo { kind: Kind::Playlist, id: id.into(), title: id.into(), subtitle: String::new(), count: None, favorites: false, editable: true, year: None, pinned: None };
        let mut items: Vec<_> = ["fav", "p.1", "p.2", "p.3", "p.4"].map(item).into();
        apply_pins(&mut items, &pins);
        let order: Vec<_> = items.iter().map(|c| (c.id.as_str(), c.pinned)).collect();
        assert_eq!(order, [("p.3", Some(0)), ("p.1", Some(1)), ("fav", None), ("p.2", None), ("p.4", None)]);
        assert!(pinned_playlists(&dir.join("missing.json")).is_empty());
        std::fs::write(&file, "not json").unwrap();
        assert!(pinned_playlists(&file).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
