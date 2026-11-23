//! Mixed playlists: local files inside Apple playlists, and Uzumaki playlists (kept on this
//! computer) holding both kinds. Plain data and the file; `state/mixed.rs` wires it into the app.
//!
//! Apple can't store local files and has no public upload API (the private iTunes upload
//! protocol was rejected: terms of service and account risk), so nothing here ever goes to Apple.
//!
//! **Overlays.** For an Apple playlist id, the local songs inserted into it. Each entry is anchored
//! to the Apple songs around it when it was placed (`after`, `before`) plus the count of Apple
//! songs before it (`pos`), so Apple-side edits made on an iPhone or a Mac move it sensibly:
//! - the song before it is still there (wherever it went): the entry follows it;
//! - else the song after it is: the entry sits just before that one;
//! - else neither is (both removed): it keeps its old position (`pos`, clamped to the end).
//!
//! Entries resolving to the same spot keep their order in the list. Anchors match a song by its
//! id (library `i.…` ids are stable) or, when both know it, its catalog id.
//!
//! **Local songs** are kept as a `LocalRef`: the path plus the tags and duration. A file not at its
//! path any more is looked for by its tags (title, artist, album, duration within 2 s), so a moved
//! or renamed file is found again (`State::relink_moved` then stores the new path). No content
//! hash: reading every file would cost a full read of the library, and tags survive a move. A file
//! found neither way shows as missing (greyed, not playable) until it is back.
//!
//! **The file** is `$XDG_DATA_HOME/uzumaki/playlists.json` (beside `playlist-art/`, apart from the
//! caches Settings can clear). Versioned; written atomically (temp file + rename) on a background
//! thread, saves in order. A file of a newer version is read but never written over; one that
//! can't be parsed is kept as `playlists.json.bad` and the app starts empty.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::models::Song;
use crate::ui::fold;

/// Id prefix of Uzumaki playlists (Apple's are `p.` and `pl.`).
pub const UZU_PREFIX: &str = "u.";

/// Version of the file. Older versions are upgraded on read; newer ones are left alone.
pub const VERSION: u32 = 1;

/// Seconds a moved file's duration may differ by and still count as the same song.
const DURATION_SLACK: f64 = 2.0;

pub fn is_uzu_id(id: &str) -> bool {
    id.starts_with(UZU_PREFIX)
}

/// A local song as stored: where it was, and what it is (to find it again if it moved).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalRef {
    pub path: PathBuf,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    /// Seconds.
    #[serde(default)]
    pub duration: f64,
}

impl LocalRef {
    /// The reference of a local song (None for Apple songs).
    pub fn of(song: &Song) -> Option<Self> {
        Some(Self {
            path: song.local_path()?.to_path_buf(),
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: if song.album_is_fallback { String::new() } else { song.album.clone() },
            duration: song.duration,
        })
    }

    fn tags(&self) -> (String, String, String) {
        (fold(&self.title), fold(&self.artist), fold(&self.album))
    }
}

/// An Apple song an overlay entry sits next to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
}

impl Anchor {
    pub fn of(song: &Song) -> Self {
        Self { id: song.id.clone(), catalog: song.catalog_id.clone() }
    }
}

/// A local song inserted into an Apple playlist.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlayEntry {
    pub file: LocalRef,
    /// The Apple song just before it (None: it was first, or the list wasn't loaded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Anchor>,
    /// The Apple song just after it (None: it was last).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Anchor>,
    /// Apple songs before it when it was placed: the fallback when neither anchor is found.
    #[serde(default)]
    pub pos: usize,
    /// When it was added (seconds since the epoch).
    #[serde(default)]
    pub added: u64,
}

/// One song of an Uzumaki playlist.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum UzuEntry {
    /// An Apple Music song (catalog or library), as it was when added, so the list shows (and
    /// plays) without asking Apple.
    Apple { song: Song },
    Local { file: LocalRef },
}

/// A playlist kept on this computer only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UzuPlaylist {
    /// `u.` + a random hex string.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub modified: u64,
    #[serde(default)]
    pub entries: Vec<UzuEntry>,
}

/// Everything in `playlists.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    /// Local songs inside Apple playlists, by Apple playlist id.
    #[serde(default)]
    pub overlays: BTreeMap<String, Vec<OverlayEntry>>,
    /// Uzumaki playlists, newest first (sidebar order).
    #[serde(default)]
    pub playlists: Vec<UzuPlaylist>,
}

impl Default for Store {
    fn default() -> Self {
        Self { version: VERSION, overlays: BTreeMap::new(), playlists: Vec::new() }
    }
}

/// What reading the file gave.
#[derive(Debug, PartialEq)]
pub enum Loaded {
    /// Missing (first run) or read fine.
    Ok(Store),
    /// A newer app wrote it: shown, but never written over.
    Newer(Store),
    /// Unreadable: kept aside as `.bad`, the app starts empty.
    Broken,
    /// There, but reading it failed (permissions, I/O): shown empty and never written over,
    /// since it may be the only copy of the user's Uzumaki playlists.
    Unreadable(String),
}

impl Store {
    pub fn playlist(&self, id: &str) -> Option<&UzuPlaylist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    pub fn playlist_mut(&mut self, id: &str) -> Option<&mut UzuPlaylist> {
        self.playlists.iter_mut().find(|p| p.id == id)
    }

    pub fn overlay(&self, id: &str) -> &[OverlayEntry] {
        self.overlays.get(id).map_or(&[], Vec::as_slice)
    }

    /// Reads `path` (see `Loaded`).
    pub fn read(path: &Path) -> Loaded {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Ok(Store::default()),
            Err(e) => return Loaded::Unreadable(e.to_string()),
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            set_aside(path);
            return Loaded::Broken;
        };
        let version = value.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0);
        match serde_json::from_value::<Store>(value) {
            Ok(s) if version > u64::from(VERSION) => Loaded::Newer(s),
            Ok(mut s) => {
                s.version = VERSION;
                Loaded::Ok(s)
            }
            // A newer file we can't even parse: show nothing, write nothing.
            Err(_) if version > u64::from(VERSION) => Loaded::Newer(Store::default()),
            Err(_) => {
                set_aside(path);
                Loaded::Broken
            }
        }
    }

    /// Writes `path` now, atomically (temp file + rename).
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

/// Keeps an unreadable file as `<name>.bad` so the next save doesn't lose it.
fn set_aside(path: &Path) {
    let bad = path.with_extension("json.bad");
    let _ = std::fs::rename(path, bad);
}

/// `$XDG_DATA_HOME/uzumaki/playlists.json` (beside `playlist-art/`).
pub fn path() -> PathBuf {
    let art = crate::settings::playlist_art_dir();
    art.parent().map_or_else(|| art.clone(), Path::to_path_buf).join("playlists.json")
}

static SAVED_GEN: Mutex<u64> = Mutex::new(0);
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

/// Saves `store` to `path` on a background thread. Saves never interleave, and one that lost the
/// race to a newer one is dropped (the file never goes back to an older copy).
pub fn save_in_background(store: &Store, path: PathBuf) {
    let copy = store.clone();
    let generation = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
    std::thread::spawn(move || {
        let mut saved = SAVED_GEN.lock().unwrap_or_else(|e| e.into_inner());
        if generation < *saved {
            return;
        }
        if let Err(e) = copy.write(&path) {
            eprintln!("uzumaki: couldn't save {}: {e}", path.display());
            return;
        }
        *saved = generation;
    });
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A new Uzumaki playlist id: `u.` + 16 hex digits from the clock and a counter (unique enough
/// for one person's playlists, no random crate needed).
pub fn new_id(existing: &[UzuPlaylist]) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut n = nanos ^ 0x9e37_79b9_7f4a_7c15;
    loop {
        let id = format!("{UZU_PREFIX}{n:016x}");
        if existing.iter().all(|p| p.id != id) {
            return id;
        }
        n = n.wrapping_add(1);
    }
}

// ------------------------------------------------------------------------------ finding files

/// The music folders' songs by path and by tags, to resolve `LocalRef`s. Built once per index
/// change (`LocalLibrary::generation`).
#[derive(Default)]
pub struct LocalIndex {
    by_path: HashMap<PathBuf, usize>,
    by_tags: HashMap<(String, String, String), Vec<usize>>,
}

/// Where a `LocalRef` is now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Found {
    /// At its path: song `i` of the library.
    Here(usize),
    /// Moved or renamed: song `i` has its tags and duration.
    Moved(usize),
    Missing,
}

impl LocalIndex {
    pub fn new(songs: &[Song]) -> Self {
        let mut ix = Self::default();
        for (i, s) in songs.iter().enumerate() {
            if let Some(p) = s.local_path() {
                ix.by_path.insert(p.to_path_buf(), i);
            }
            let album = if s.album_is_fallback { String::new() } else { fold(&s.album) };
            ix.by_tags.entry((fold(&s.title), fold(&s.artist), album)).or_default().push(i);
        }
        ix
    }

    pub fn find(&self, songs: &[Song], r: &LocalRef) -> Found {
        if let Some(&i) = self.by_path.get(&r.path) {
            return Found::Here(i);
        }
        if r.title.trim().is_empty() {
            return Found::Missing;
        }
        let close = |&&i: &&usize| songs.get(i).is_some_and(|s| r.duration <= 0.0 || (s.duration - r.duration).abs() <= DURATION_SLACK);
        match self.by_tags.get(&r.tags()).and_then(|v| v.iter().find(close)) {
            Some(&i) => Found::Moved(i),
            None => Found::Missing,
        }
    }
}

/// The song `r` stands for: the library's when found, else a stand-in built from the stored tags.
/// The stand-in is playable while the library isn't read yet (`loaded` false: the file is likely
/// there), and not once it is (a missing file: `is_missing`).
pub fn resolve(r: &LocalRef, songs: &[Song], index: &LocalIndex, loaded: bool) -> Song {
    match index.find(songs, r) {
        Found::Here(i) | Found::Moved(i) => songs[i].clone(),
        Found::Missing => stand_in(r, !loaded),
    }
}

fn stand_in(r: &LocalRef, playable: bool) -> Song {
    let file = r.path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Song {
        id: Song::local_id(&r.path),
        catalog_id: None,
        title: if r.title.trim().is_empty() { file } else { r.title.clone() },
        artist: r.artist.clone(),
        album: r.album.clone(),
        album_is_fallback: r.album.is_empty(),
        duration: r.duration,
        track_number: None,
        disc_number: None,
        explicit: false,
        artwork: None,
        playable,
        genre: None,
        year: None,
        album_id: None,
        artists: Vec::new(),
    }
}

/// A local song whose file isn't in the music folders any more (a stand-in from `resolve`).
pub fn is_missing(song: &Song) -> bool {
    song.is_local() && !song.playable
}

// ------------------------------------------------------------------------------ merging

/// Where a row of a merged list comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Song `i` of Apple's list.
    Apple(usize),
    /// Entry `i` of the overlay.
    Local(usize),
}

/// An Apple playlist with its overlay: the rows, and where each comes from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    pub tracks: Vec<Song>,
    pub origin: Vec<Origin>,
}

impl Merged {
    /// Rows of Apple songs only, with no overlay.
    pub fn plain(apple: &[Song]) -> Self {
        Self { tracks: apple.to_vec(), origin: (0..apple.len()).map(Origin::Apple).collect() }
    }
}

/// First position of each Apple song, by id and by catalog id.
struct Positions<'a> {
    ids: HashMap<&'a str, usize>,
    catalog: HashMap<&'a str, usize>,
}

impl<'a> Positions<'a> {
    fn of(apple: &'a [Song]) -> Self {
        let mut ids = HashMap::with_capacity(apple.len());
        let mut catalog = HashMap::new();
        for (i, s) in apple.iter().enumerate() {
            ids.entry(s.id.as_str()).or_insert(i);
            if let Some(c) = &s.catalog_id {
                catalog.entry(c.as_str()).or_insert(i);
            }
        }
        Self { ids, catalog }
    }

    fn find(&self, a: &Anchor) -> Option<usize> {
        self.ids.get(a.id.as_str()).or_else(|| a.catalog.as_deref().and_then(|c| self.catalog.get(c))).copied()
    }
}

/// For each entry, how many Apple songs come before it (see the module docs).
pub fn points(apple: &[Song], entries: &[OverlayEntry]) -> Vec<usize> {
    let at = Positions::of(apple);
    entries
        .iter()
        .map(|e| {
            e.after.as_ref().and_then(|a| at.find(a)).map(|i| i + 1)
                .or_else(|| e.before.as_ref().and_then(|b| at.find(b)))
                .unwrap_or_else(|| e.pos.min(apple.len()))
        })
        .collect()
}

/// The order of the merged rows (no songs cloned).
pub fn order(apple: &[Song], entries: &[OverlayEntry]) -> Vec<Origin> {
    let pts = points(apple, entries);
    let mut locals: Vec<usize> = (0..entries.len()).collect();
    locals.sort_by_key(|&k| pts[k]); // stable: ties keep the overlay's order
    let mut out = Vec::with_capacity(apple.len() + entries.len());
    let mut l = locals.into_iter().peekable();
    for i in 0..=apple.len() {
        while let Some(&k) = l.peek().filter(|&&k| pts[k] <= i) {
            out.push(Origin::Local(k));
            l.next();
        }
        if i < apple.len() {
            out.push(Origin::Apple(i));
        }
    }
    out
}

/// Apple's list with the overlay's songs in their places; `song` gives each entry's song.
pub fn merge(apple: &[Song], entries: &[OverlayEntry], song: impl Fn(&LocalRef) -> Song) -> Merged {
    if entries.is_empty() {
        return Merged::plain(apple);
    }
    let origin = order(apple, entries);
    let tracks = origin
        .iter()
        .map(|o| match *o {
            Origin::Apple(i) => apple[i].clone(),
            Origin::Local(k) => song(&entries[k].file),
        })
        .collect();
    Merged { tracks, origin }
}

/// The anchors of a local row at position `m` of the merged order `seq`.
fn anchors_at(apple: &[Song], seq: &[Origin], m: usize) -> (Option<Anchor>, Option<Anchor>, usize) {
    let after = seq[..m].iter().rev().find_map(|o| match o {
        Origin::Apple(i) => Some(*i),
        Origin::Local(_) => None,
    });
    let before = seq[m + 1..].iter().find_map(|o| match o {
        Origin::Apple(i) => Some(*i),
        Origin::Local(_) => None,
    });
    let pos = seq[..m].iter().filter(|o| matches!(o, Origin::Apple(_))).count();
    (after.map(|i| Anchor::of(&apple[i])), before.map(|i| Anchor::of(&apple[i])), pos)
}

/// Appends `files` to the end of the playlist (after Apple's last song, as Apple appends).
/// `count`: Apple's song count when its list isn't loaded (`apple` empty).
pub fn append(entries: &mut Vec<OverlayEntry>, apple: &[Song], count: usize, files: Vec<LocalRef>) {
    let after = apple.last().map(Anchor::of);
    let pos = if apple.is_empty() { count } else { apple.len() };
    let added = now_secs();
    entries.extend(files.into_iter().map(|file| OverlayEntry { file, after: after.clone(), before: None, pos, added }));
}

/// Moves the local row at merged position `m` one row up (`delta` -1) or down (+1), past whatever
/// row is there. Returns false when it can't move (not a local row, or at an end).
pub fn move_row(entries: &mut Vec<OverlayEntry>, apple: &[Song], m: usize, delta: isize) -> bool {
    let mut seq = order(apple, entries);
    let Some(Origin::Local(k)) = seq.get(m).copied() else { return false };
    let Some(to) = m.checked_add_signed(delta).filter(|&t| t < seq.len()) else { return false };
    seq.swap(m, to);
    let (after, before, pos) = anchors_at(apple, &seq, to);
    let e = &mut entries[k];
    (e.after, e.before, e.pos) = (after, before, pos);
    // The overlay in the new row order, so entries landing on one spot keep the order shown.
    let mut old: Vec<Option<OverlayEntry>> = std::mem::take(entries).into_iter().map(Some).collect();
    *entries = seq.iter().filter_map(|o| match o {
        Origin::Local(k) => old[*k].take(),
        Origin::Apple(_) => None,
    }).collect();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apple(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: Some(format!("c{id}")),
            title: id.into(),
            artist: "A".into(),
            album: "B".into(),
            album_is_fallback: false,
            duration: 200.0,
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

    fn file(name: &str) -> LocalRef {
        LocalRef { path: format!("/music/{name}.flac").into(), title: name.into(), artist: "L".into(), album: "LA".into(), duration: 180.0 }
    }

    fn local_song(r: &LocalRef) -> Song {
        stand_in(r, true)
    }

    fn titles(m: &Merged) -> Vec<String> {
        m.tracks.iter().map(|t| t.title.clone()).collect()
    }

    fn list(ids: &[&str]) -> Vec<Song> {
        ids.iter().map(|i| apple(i)).collect()
    }

    /// The overlay after placing `name` at merged row `m` of `apple` + `entries`.
    fn placed(apple: &[Song], entries: &mut Vec<OverlayEntry>, name: &str, m: usize) {
        append(entries, apple, apple.len(), vec![file(name)]);
        let mut at = order(apple, entries).iter().position(|o| *o == Origin::Local(entries.len() - 1)).unwrap();
        while at > m {
            assert!(move_row(entries, apple, at, -1));
            at -= 1;
        }
    }

    #[test]
    fn appended_files_go_last_and_follow_the_last_song() {
        let a = list(&["i.1", "i.2"]);
        let mut e = Vec::new();
        append(&mut e, &a, 2, vec![file("x"), file("y")]);
        assert_eq!(titles(&merge(&a, &e, local_song)), ["i.1", "i.2", "x", "y"]);
        // Apple adds a song at the end (on the iPhone): the files stay after i.2.
        let a2 = list(&["i.1", "i.2", "i.3"]);
        assert_eq!(titles(&merge(&a2, &e, local_song)), ["i.1", "i.2", "x", "y", "i.3"]);
    }

    #[test]
    fn an_entry_follows_its_song_when_apple_reorders() {
        let a = list(&["i.1", "i.2", "i.3", "i.4"]);
        let mut e = Vec::new();
        placed(&a, &mut e, "x", 2); // after i.2, before i.3
        assert_eq!(titles(&merge(&a, &e, local_song)), ["i.1", "i.2", "x", "i.3", "i.4"]);
        let moved = list(&["i.3", "i.1", "i.4", "i.2"]);
        assert_eq!(titles(&merge(&moved, &e, local_song)), ["i.3", "i.1", "i.4", "i.2", "x"]);
    }

    #[test]
    fn a_removed_neighbour_falls_back_to_the_next_song_then_to_the_old_position() {
        let a = list(&["i.1", "i.2", "i.3", "i.4"]);
        let mut e = Vec::new();
        placed(&a, &mut e, "x", 2);
        // i.2 removed on the Mac: x sits before i.3.
        let without_before = list(&["i.3", "i.1", "i.4"]);
        assert_eq!(titles(&merge(&without_before, &e, local_song)), ["x", "i.3", "i.1", "i.4"]);
        // Both neighbours gone: two Apple songs before it, as when it was placed.
        let neither = list(&["i.1", "i.4", "i.5"]);
        assert_eq!(titles(&merge(&neither, &e, local_song)), ["i.1", "i.4", "x", "i.5"]);
        // And clamped to the end of a shorter list.
        assert_eq!(titles(&merge(&list(&["i.9"]), &e, local_song)), ["i.9", "x"]);
    }

    #[test]
    fn anchors_match_by_catalog_id_too() {
        let a = list(&["i.1", "i.2"]);
        let mut e = Vec::new();
        placed(&a, &mut e, "x", 1);
        // The same songs under new library ids (re-added): their catalog ids still match.
        let mut again = list(&["i.8", "i.9"]);
        again[0].catalog_id = Some("ci.1".into());
        again[1].catalog_id = Some("ci.2".into());
        assert_eq!(titles(&merge(&again, &e, local_song)), ["i.8", "x", "i.9"]);
    }

    #[test]
    fn rows_move_past_apple_songs_and_past_each_other() {
        let a = list(&["i.1", "i.2"]);
        let mut e = Vec::new();
        append(&mut e, &a, 2, vec![file("x"), file("y")]);
        // y up past x: same spot, the overlay's order changes.
        assert!(move_row(&mut e, &a, 3, -1));
        assert_eq!(titles(&merge(&a, &e, local_song)), ["i.1", "i.2", "y", "x"]);
        // y up past i.2 and i.1: first.
        assert!(move_row(&mut e, &a, 2, -1));
        assert!(move_row(&mut e, &a, 1, -1));
        assert_eq!(titles(&merge(&a, &e, local_song)), ["y", "i.1", "i.2", "x"]);
        assert!(!move_row(&mut e, &a, 0, -1), "already first");
        assert!(!move_row(&mut e, &a, 1, 1), "an Apple row is Apple's to move");
        assert!(!move_row(&mut e, &a, 3, 1), "already last");
        // Its anchor is now "before i.1": Apple putting a song first keeps y before i.1.
        let a2 = list(&["i.0", "i.1", "i.2"]);
        assert_eq!(titles(&merge(&a2, &e, local_song)), ["i.0", "y", "i.1", "i.2", "x"]);
    }

    #[test]
    fn appending_before_the_list_loads_uses_the_count() {
        let mut e = Vec::new();
        append(&mut e, &[], 3, vec![file("x")]);
        assert_eq!(titles(&merge(&list(&["i.1", "i.2", "i.3", "i.4"]), &e, local_song)), ["i.1", "i.2", "i.3", "x", "i.4"]);
    }

    #[test]
    fn a_moved_file_is_found_by_its_tags_and_a_gone_one_is_missing() {
        let mut moved = local_song(&file("x"));
        moved.id = Song::local_id(Path::new("/elsewhere/renamed.flac"));
        moved.duration = 181.0;
        let songs = vec![local_song(&file("y")), moved];
        let ix = LocalIndex::new(&songs);
        assert_eq!(ix.find(&songs, &file("y")), Found::Here(0));
        assert_eq!(ix.find(&songs, &file("x")), Found::Moved(1));
        assert_eq!(resolve(&file("x"), &songs, &ix, true).id, "file:///elsewhere/renamed.flac");
        let gone = resolve(&file("z"), &songs, &ix, true);
        assert!(is_missing(&gone));
        assert_eq!(gone.title, "z");
        assert!(!is_missing(&resolve(&file("z"), &songs, &ix, false)), "unknown until the index is read");
        // Same tags, a different length: another recording, not the moved file.
        let mut long = file("x");
        long.duration = 300.0;
        assert_eq!(ix.find(&songs, &long), Found::Missing);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("uzumaki-mixed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("playlists.json")
    }

    #[test]
    fn the_file_round_trips_and_leaves_no_temp_file() {
        let path = scratch("roundtrip");
        let mut s = Store::default();
        append(s.overlays.entry("p.1".into()).or_default(), &list(&["i.1"]), 1, vec![file("x")]);
        s.playlists.push(UzuPlaylist {
            id: "u.1".into(),
            name: "Mix".into(),
            description: None,
            created: 1,
            modified: 2,
            entries: vec![UzuEntry::Apple { song: apple("123") }, UzuEntry::Local { file: file("y") }],
        });
        s.write(&path).unwrap();
        assert_eq!(Store::read(&path), Loaded::Ok(s));
        assert!(!path.with_extension("json.tmp").exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"kind\": \"local\""), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_newer_file_is_read_but_flagged_and_a_broken_one_set_aside() {
        let path = scratch("versions");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"version": 9, "playlists": [{"id": "u.1", "name": "Later"}], "future": true}"#).unwrap();
        match Store::read(&path) {
            Loaded::Newer(s) => assert_eq!(s.playlists[0].name, "Later"),
            other => panic!("{other:?}"),
        }
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Store::read(&path), Loaded::Broken);
        assert!(!path.exists() && path.with_extension("json.bad").exists(), "kept for the user, not overwritten");
        assert_eq!(Store::read(&path), Loaded::Ok(Store::default()), "missing: a first run");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

#[cfg(all(test, unix))]
mod review_edges {
    use super::*;

    /// playlists.json exists but can't be read (EACCES after a `sudo uzumaki`, EIO on a failing
    /// disk, a home on a network mount that is slow to come up). That is read as a first run:
    /// `Loaded::Ok(empty)`, not read-only, so the next playlist change writes an empty store over
    /// the user's Uzumaki playlists (rename replaces the file even when it isn't ours to read).
    #[test]
    fn an_unreadable_file_is_not_a_first_run() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("uzumaki-review-edges-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("playlists.json");
        let mut store = Store::default();
        store.playlists.push(UzuPlaylist { id: "u.1".into(), name: "Mine".into(), description: None, created: 1, modified: 1, entries: vec![] });
        store.write(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            // Root (a Docker run) reads it anyway: nothing to test here.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let loaded = Store::read(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(loaded, Loaded::Ok(Store::default()), "an unreadable file reads as an empty, writable store");
        assert!(matches!(loaded, Loaded::Unreadable(_)), "{loaded:?}");
    }
}
