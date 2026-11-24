//! Favourites for local files: the star on a song from the music folders. Kept on this computer
//! only (Apple can't store local files), in `$XDG_DATA_HOME/uzumaki/local-favourites.json`,
//! beside `playlists.json` and apart from the caches Settings can clear.
//!
//! Each favourite is a `mixed::LocalRef` (path plus tags), so a moved or renamed file keeps its
//! star (`State::relink_moved` stores the new path). The file is versioned and written atomically
//! on a background thread; one of a newer version is read but never written over, and one that
//! can't be parsed is kept as `local-favourites.json.bad`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::mixed::LocalRef;
use crate::models::{Collection, CollectionKind, Song};

/// The id of the Favourite Songs page made of local favourites alone (local only mode, or no
/// Apple Favourite Songs playlist to show them in).
pub const PAGE_ID: &str = "local-favourites";

pub const VERSION: u32 = 1;

pub fn is_page_id(id: &str) -> bool {
    id == PAGE_ID
}

/// The sidebar entry of the local Favourite Songs page (no songs; `State::local_favourites_page`
/// gives them).
pub fn card() -> &'static Collection {
    static CARD: std::sync::OnceLock<Collection> = std::sync::OnceLock::new();
    CARD.get_or_init(|| Collection {
        kind: CollectionKind::Playlist,
        id: PAGE_ID.into(),
        title: "Favourite Songs".into(),
        subtitle: "On this computer".into(),
        artwork: None,
        year: None,
        genre: None,
        description: None,
        track_count: None,
        modified: None,
        editable: false,
        favorites: true,
        public: None,
        artists: Vec::new(),
        tracks: Vec::new(),
    })
}

/// One starred file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub file: LocalRef,
    /// When it was starred (seconds since the epoch).
    #[serde(default)]
    pub added: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FileData {
    version: u32,
    #[serde(default)]
    songs: Vec<Entry>,
}

/// The local favourites, oldest first (the order Apple's Favourite Songs uses).
#[derive(Default)]
pub struct LocalFavs {
    entries: Vec<Entry>,
    paths: HashSet<PathBuf>,
    /// Counts changes (lists showing stars and the merged Favourite Songs rebuild).
    pub rev: u64,
    /// The file; None in headless runs and tests (never written).
    path: Option<PathBuf>,
    read_only: bool,
}

impl LocalFavs {
    /// The app's favourites (`persist`), or an empty set that is never saved (headless, tests).
    pub fn new(persist: bool) -> Self {
        if !persist {
            return Self::default();
        }
        let path = path();
        let (entries, read_only) = read(&path);
        let mut f = Self { entries: Vec::new(), paths: HashSet::new(), rev: 0, path: Some(path), read_only };
        f.replace(entries);
        f
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, song: &Song) -> bool {
        song.local_path().is_some_and(|p| self.paths.contains(p))
    }

    /// Stars (`on`) or un-stars local `songs`; returns how many changed.
    pub fn set(&mut self, songs: &[&Song], on: bool) -> usize {
        let now = crate::mixed::now_secs();
        let mut n = 0;
        for s in songs {
            let Some(file) = LocalRef::of(s) else { continue };
            if on && self.paths.insert(file.path.clone()) {
                self.entries.push(Entry { file, added: now });
                n += 1;
            } else if !on && self.paths.remove(&file.path) {
                self.entries.retain(|e| e.file.path != file.path);
                n += 1;
            }
        }
        if n > 0 {
            self.changed();
        }
        n
    }

    /// Lets `fix` update each stored reference (a moved file's new path); saves when it did, and
    /// says so.
    pub fn relink(&mut self, mut fix: impl FnMut(&mut LocalRef) -> bool) -> bool {
        let mut changed = false;
        for e in &mut self.entries {
            changed |= fix(&mut e.file);
        }
        if changed {
            let entries = std::mem::take(&mut self.entries);
            self.replace(entries);
            self.changed();
        }
        changed
    }

    fn replace(&mut self, entries: Vec<Entry>) {
        self.paths.clear();
        // A file starred twice (two references that resolved to one path) counts once.
        self.entries = entries.into_iter().filter(|e| self.paths.insert(e.file.path.clone())).collect();
    }

    fn changed(&mut self) {
        self.rev += 1;
        if let (Some(path), false) = (&self.path, self.read_only) {
            save_in_background(FileData { version: VERSION, songs: self.entries.clone() }, path.clone());
        }
    }
}

/// `$XDG_DATA_HOME/uzumaki/local-favourites.json` (beside `playlists.json`).
pub fn path() -> PathBuf {
    let p = crate::mixed::path();
    p.parent().map_or_else(|| PathBuf::from("local-favourites.json"), |d| d.join("local-favourites.json"))
}

/// The entries in `path`, and whether the file must not be written over (newer, or there but
/// unreadable). A missing file is a first run; an unparsable one is kept as `.bad`.
fn read(path: &Path) -> (Vec<Entry>, bool) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), false),
        Err(e) => {
            eprintln!("uzumaki: can't read {}: {e}; local favourites won't be saved", path.display());
            return (Vec::new(), true);
        }
    };
    let value: Option<serde_json::Value> = serde_json::from_slice(&bytes).ok();
    let version = value.as_ref().and_then(|v| v.get("version")).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let newer = version > u64::from(VERSION);
    match value.map(serde_json::from_value::<FileData>) {
        Some(Ok(d)) => {
            if newer {
                eprintln!("uzumaki: {} is from a newer Uzumaki; local favourites won't be saved", path.display());
            }
            (d.songs, newer)
        }
        _ if newer => (Vec::new(), true),
        _ => {
            eprintln!("uzumaki: {} was unreadable; kept as local-favourites.json.bad", path.display());
            let _ = std::fs::rename(path, path.with_extension("json.bad"));
            (Vec::new(), false)
        }
    }
}

fn write(data: &FileData, path: &Path) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(data).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

static SAVED_GEN: Mutex<u64> = Mutex::new(0);
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

/// Saves in order on a background thread; a save that lost the race to a newer one is dropped.
fn save_in_background(data: FileData, path: PathBuf) {
    let generation = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
    std::thread::spawn(move || {
        let mut saved = SAVED_GEN.lock().unwrap_or_else(|e| e.into_inner());
        if generation < *saved {
            return;
        }
        if let Err(e) = write(&data, &path) {
            eprintln!("uzumaki: couldn't save {}: {e}", path.display());
            return;
        }
        *saved = generation;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(path: &str, title: &str) -> Song {
        Song {
            id: format!("file://{path}"),
            catalog_id: None,
            title: title.into(),
            artist: "A".into(),
            album: "B".into(),
            album_is_fallback: false,
            duration: 100.0,
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
    fn starring_twice_counts_once_and_unstarring_removes() {
        let mut f = LocalFavs::default();
        let (a, b) = (song("/m/a.flac", "a"), song("/m/b.flac", "b"));
        assert_eq!(f.set(&[&a, &b, &a], true), 2);
        assert!(f.contains(&a) && f.contains(&b));
        assert_eq!(f.set(&[&a], true), 0);
        let rev = f.rev;
        assert_eq!(f.set(&[&a], false), 1);
        assert!(!f.contains(&a));
        assert!(f.rev > rev);
        assert_eq!(f.entries().len(), 1);
        // Apple songs are never stored here.
        let apple = Song { id: "123".into(), ..song("/m/c.flac", "c") };
        assert_eq!(f.set(&[&apple], true), 0);
    }

    #[test]
    fn the_file_round_trips_and_a_broken_one_is_set_aside() {
        let d = std::env::temp_dir().join(format!("uzumaki-lfav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("local-favourites.json");
        assert_eq!(read(&p), (Vec::new(), false), "first run");
        let e = Entry { file: LocalRef::of(&song("/m/a.flac", "a")).unwrap(), added: 7 };
        write(&FileData { version: VERSION, songs: vec![e.clone()] }, &p).unwrap();
        assert_eq!(read(&p), (vec![e.clone()], false));
        std::fs::write(&p, r#"{"version": 99, "songs": []}"#).unwrap();
        assert_eq!(read(&p), (Vec::new(), true), "newer: never written over");
        std::fs::write(&p, "garbage").unwrap();
        assert_eq!(read(&p), (Vec::new(), false));
        assert!(d.join("local-favourites.json.bad").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
