//! Sorting, after yazi: `,` opens a small popup listing the sort keys and the next key picks
//! one (lowercase ascending, uppercase descending, `,` the original order). This module holds
//! the modes, the ordering (pure) and the choice remembered between runs.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uzumaki_ctl::{CollectionInfo, SongInfo};

/// What a sort compares.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Title,
    Artist,
    Album,
    Duration,
    Name,
    Count,
    Year,
    /// The original order itself; descending is that order reversed (newest first).
    Original,
}

impl Key {
    pub fn label(self) -> &'static str {
        match self {
            Key::Title => "title",
            Key::Artist => "artist",
            Key::Album => "album",
            Key::Duration => "duration",
            Key::Name => "name",
            Key::Count => "song count",
            Key::Year => "year",
            Key::Original => "original order",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Sort {
    pub key: Key,
    pub desc: bool,
}

impl Sort {
    /// "by name ↑".
    pub fn describe(self) -> String {
        if self.key == Key::Original {
            return if self.desc { "newest first".into() } else { "original order".into() };
        }
        format!("by {} {}", self.key.label(), if self.desc { '↓' } else { '↑' })
    }
}

/// Something that can be sorted. Songs are shared by screens 2 and 4 (and kept as one choice);
/// the search results have their own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Playlists,
    Albums,
    Songs,
    Search,
}

const SONG_MODES: &[(char, Key)] = &[('t', Key::Title), ('a', Key::Artist), ('l', Key::Album), ('d', Key::Duration)];
const PLAYLIST_MODES: &[(char, Key)] = &[('n', Key::Name), ('c', Key::Count)];
const ALBUM_MODES: &[(char, Key)] = &[('n', Key::Name), ('a', Key::Artist), ('y', Key::Year)];

impl View {
    /// The popup's title.
    pub fn title(self) -> &'static str {
        match self {
            View::Playlists => "Sort playlists",
            View::Albums => "Sort albums",
            View::Songs | View::Search => "Sort songs",
        }
    }

    /// What it sorts, for messages.
    pub fn what(self) -> &'static str {
        match self {
            View::Playlists => "playlists",
            View::Albums => "albums",
            View::Songs => "songs",
            View::Search => "search results",
        }
    }

    /// The keys (lowercase; the uppercase one reverses) and what they sort by.
    pub fn modes(self) -> &'static [(char, Key)] {
        match self {
            View::Playlists => PLAYLIST_MODES,
            View::Albums => ALBUM_MODES,
            View::Songs | View::Search => SONG_MODES,
        }
    }

    /// What a key picks in the popup: `Some(None)` is the original order, `None` is a key the
    /// popup doesn't list (it just closes).
    pub fn pick(self, c: char) -> Option<Option<Sort>> {
        match c {
            ',' | 'o' => return Some(None),
            'O' => return Some(Some(Sort { key: Key::Original, desc: true })),
            _ => {}
        }
        let desc = c.is_ascii_uppercase();
        let lower = c.to_ascii_lowercase();
        self.modes().iter().find(|(k, _)| *k == lower).map(|&(_, key)| Some(Sort { key, desc }))
    }
}

/// A sort as `uzu.toml` writes it: a key's name (`name`, `artist`, `song count`, …) and
/// optionally `asc` or `desc` (`"year desc"`); `original` is the original order and `newest
/// first` its reverse. `Ok(None)` is the original order.
pub fn parse_default(view: View, text: &str) -> Result<Option<Sort>, String> {
    let lower = text.trim().to_lowercase();
    let (name, desc) = match lower.rsplit_once(' ') {
        Some((n, "desc" | "descending")) => (n.trim(), true),
        Some((n, "asc" | "ascending")) => (n.trim(), false),
        _ => (lower.as_str(), false),
    };
    match name {
        "original" | "original order" | "none" if !desc => return Ok(None),
        "newest first" | "newest" => return Ok(Some(Sort { key: Key::Original, desc: true })),
        _ => {}
    }
    let name = if name == "count" { "song count" } else { name };
    if let Some(&(_, key)) = view.modes().iter().find(|(_, k)| k.label() == name) {
        return Ok(Some(Sort { key, desc }));
    }
    let valid: Vec<&str> = view.modes().iter().map(|(_, k)| k.label()).collect();
    let hint = match crate::config::did_you_mean(name, &valid) {
        Some(k) => format!("did you mean \"{k}\"?"),
        None => format!("use {}, with asc or desc after it, or \"original\"", crate::config::list_of(&valid, "\"", "\"")),
    };
    Err(format!("'{}' is not a way to sort {}; {hint}", text.trim(), view.what()))
}

/// The remembered choice of each view.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub playlists: Option<Sort>,
    pub albums: Option<Sort>,
    pub songs: Option<Sort>,
    pub search: Option<Sort>,
}

impl Prefs {
    pub fn get(&self, view: View) -> Option<Sort> {
        match view {
            View::Playlists => self.playlists,
            View::Albums => self.albums,
            View::Songs => self.songs,
            View::Search => self.search,
        }
    }

    pub fn set(&mut self, view: View, sort: Option<Sort>) {
        match view {
            View::Playlists => self.playlists = sort,
            View::Albums => self.albums = sort,
            View::Songs => self.songs = sort,
            View::Search => self.search = sort,
        }
    }

    /// Drops choices whose key doesn't belong to the view (a hand-edited file).
    fn checked(mut self) -> Self {
        for view in [View::Playlists, View::Albums, View::Songs, View::Search] {
            if let Some(s) = self.get(view) {
                if s.key != Key::Original && !view.modes().iter().any(|(_, k)| *k == s.key) {
                    self.set(view, None);
                }
            }
        }
        self
    }
}

// ----------------------------------------------------------------------------- persistence

/// `$XDG_STATE_HOME/uzumaki/uzu.json`, else `~/.local/state/uzumaki/uzu.json`.
pub fn state_path() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = var("XDG_STATE_HOME").or_else(|| var("HOME").map(|h| h.join(".local/state")))?;
    Some(base.join("uzumaki").join("uzu.json"))
}

/// The remembered choices; none when the file is missing or unreadable.
pub fn load(path: &Path) -> Prefs {
    let read = || -> Option<Prefs> {
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        serde_json::from_value(value.get("sort")?.clone()).ok()
    };
    read().unwrap_or_default().checked()
}

/// Writes the choices under the file's `"sort"` key (other keys stay). Errors are ignored.
pub fn save(path: &Path, prefs: &Prefs) {
    let mut doc = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    let Ok(value) = serde_json::to_value(prefs) else { return };
    doc["sort"] = value;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&doc) {
        let _ = std::fs::write(path, text);
    }
}

// ---------------------------------------------------------------------------------- ordering

/// Compares ignoring case, with runs of digits compared as numbers: "Track 2" < "Track 10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(d) = it.next_if(char::is_ascii_digit) {
                        digits.push(d);
                    }
                    digits
                };
                let (x, y) = (run(&mut a), run(&mut b));
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                // No overflow, however long: more digits is bigger, else compare as text.
                let o = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

enum Val<'a> {
    Text(&'a str),
    Num(f64),
}

fn compare(a: &Val, b: &Val) -> Ordering {
    match (a, b) {
        (Val::Text(x), Val::Text(y)) => natural_cmp(x, y),
        (Val::Num(x), Val::Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        _ => Ordering::Equal,
    }
}

/// Indices of `items` in sorted order; the original order without a sort. The sort is stable
/// (ties keep their place), and items with no value (`None`) come last either way.
fn order<T, F>(items: &[T], sort: Option<Sort>, value: F) -> Vec<usize>
where
    F: for<'a> Fn(&'a T, Key) -> Option<Val<'a>>,
{
    let mut idx: Vec<usize> = (0..items.len()).collect();
    let Some(s) = sort else { return idx };
    if s.key == Key::Original {
        if s.desc {
            idx.reverse();
        }
        return idx;
    }
    idx.sort_by(|&a, &b| match (value(&items[a], s.key), value(&items[b], s.key)) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let o = compare(&x, &y);
            if s.desc { o.reverse() } else { o }
        }
    });
    idx
}

fn song_value(s: &SongInfo, key: Key) -> Option<Val<'_>> {
    match key {
        Key::Title => Some(Val::Text(&s.title)),
        Key::Artist => Some(Val::Text(&s.artist)),
        Key::Album => Some(Val::Text(&s.album)),
        Key::Duration => (s.duration > 0.0).then_some(Val::Num(s.duration)),
        _ => None,
    }
}

fn collection_value(c: &CollectionInfo, key: Key) -> Option<Val<'_>> {
    match key {
        Key::Name => Some(Val::Text(&c.title)),
        Key::Artist => Some(Val::Text(&c.subtitle)),
        Key::Count => c.count.map(|n| Val::Num(f64::from(n))),
        Key::Year => c.year.as_deref().filter(|y| !y.trim().is_empty()).map(Val::Text),
        _ => None,
    }
}

pub fn order_songs(songs: &[SongInfo], sort: Option<Sort>) -> Vec<usize> {
    order(songs, sort, song_value)
}

/// Pinned playlists (`pinned` is their rank in the app) stay on top under any sort, in pin
/// order (the user's own order, whatever the key and direction); the others are sorted below
/// them. Without pins (albums, signed-out) this is just the sort.
pub fn order_collections(items: &[CollectionInfo], sort: Option<Sort>) -> Vec<usize> {
    let mut pinned: Vec<usize> = (0..items.len()).filter(|&i| items[i].pinned.is_some()).collect();
    pinned.sort_by_key(|&i| items[i].pinned);
    let rest: Vec<usize> = (0..items.len()).filter(|&i| items[i].pinned.is_none()).collect();
    let refs: Vec<&CollectionInfo> = rest.iter().map(|&i| &items[i]).collect();
    pinned.extend(order(&refs, sort, |c, key| collection_value(c, key)).into_iter().map(|k| rest[k]));
    pinned
}

#[cfg(test)]
mod tests {
    use super::*;
    use uzumaki_ctl::Kind;

    fn song(title: &str, artist: &str, album: &str, duration: f64) -> SongInfo {
        SongInfo { id: title.into(), title: title.into(), artist: artist.into(), album: album.into(), duration, playable: true, ..Default::default() }
    }

    fn coll(title: &str, subtitle: &str, count: Option<u32>, year: Option<&str>) -> CollectionInfo {
        CollectionInfo { kind: Kind::Album, id: title.into(), title: title.into(), subtitle: subtitle.into(), count, favorites: false, editable: false, year: year.map(Into::into), pinned: None }
    }

    fn by(key: Key, desc: bool) -> Option<Sort> {
        Some(Sort { key, desc })
    }

    #[test]
    fn natural_order_ignores_case_and_reads_numbers() {
        use Ordering::*;
        assert_eq!(natural_cmp("Track 2", "Track 10"), Less);
        assert_eq!(natural_cmp("track 10", "Track 2"), Greater);
        assert_eq!(natural_cmp("apple", "Banana"), Less);
        assert_eq!(natural_cmp("ABC", "abc"), Equal);
        assert_eq!(natural_cmp("a2b", "a2c"), Less);
        assert_eq!(natural_cmp("Track 02", "Track 2"), Equal);
        assert_eq!(natural_cmp("Track", "Track 1"), Less);
        assert_eq!(natural_cmp("Весна", "весна"), Equal);
        assert_eq!(natural_cmp("99999999999999999999999", "100000000000000000000000"), Less);
        assert_eq!(natural_cmp("", "a"), Less);
    }

    #[test]
    fn songs_sort_stably_reverse_and_unknowns_last() {
        let songs = vec![
            song("Track 10", "b", "X", 100.0),
            song("track 2", "a", "Y", 0.0),
            song("Track 2", "A", "X", 100.0),
            song("Other", "", "Y", 50.0),
        ];
        assert_eq!(order_songs(&songs, None), [0, 1, 2, 3]);
        // "track 2" and "Track 2" tie: they keep their original order, ascending and descending.
        assert_eq!(order_songs(&songs, by(Key::Title, false)), [3, 1, 2, 0]);
        assert_eq!(order_songs(&songs, by(Key::Title, true)), [0, 1, 2, 3]);
        assert_eq!(order_songs(&songs, by(Key::Artist, false)), [3, 1, 2, 0]);
        assert_eq!(order_songs(&songs, by(Key::Album, false)), [0, 2, 1, 3]);
        assert_eq!(order_songs(&songs, by(Key::Album, true)), [1, 3, 0, 2]);
        // Duration 0 is unknown and comes last whichever way.
        assert_eq!(order_songs(&songs, by(Key::Duration, false)), [3, 0, 2, 1]);
        assert_eq!(order_songs(&songs, by(Key::Duration, true)), [0, 2, 3, 1]);
    }

    #[test]
    fn collections_sort_by_name_count_artist_and_year() {
        let items = vec![
            coll("b", "Zed", Some(10), Some("2013")),
            coll("A", "yan", None, None),
            coll("c", "Zed", Some(2), Some("1999")),
            coll("d", "Amy", Some(2), Some(" ")),
        ];
        assert_eq!(order_collections(&items, by(Key::Name, false)), [1, 0, 2, 3]);
        assert_eq!(order_collections(&items, by(Key::Name, true)), [3, 2, 0, 1]);
        assert_eq!(order_collections(&items, by(Key::Count, false)), [2, 3, 0, 1]);
        assert_eq!(order_collections(&items, by(Key::Count, true)), [0, 2, 3, 1]);
        assert_eq!(order_collections(&items, by(Key::Artist, false)), [3, 1, 0, 2]);
        assert_eq!(order_collections(&items, by(Key::Year, false)), [2, 0, 1, 3]);
        assert_eq!(order_collections(&items, by(Key::Year, true)), [0, 2, 1, 3]);
    }

    #[test]
    fn popup_keys_pick_modes_by_case() {
        assert_eq!(View::Songs.pick('t'), Some(by(Key::Title, false)));
        assert_eq!(View::Songs.pick('T'), Some(by(Key::Title, true)));
        assert_eq!(View::Search.pick('D'), Some(by(Key::Duration, true)));
        assert_eq!(View::Playlists.pick('c'), Some(by(Key::Count, false)));
        assert_eq!(View::Albums.pick('Y'), Some(by(Key::Year, true)));
        assert_eq!(View::Albums.pick(','), Some(None));
        assert_eq!(View::Albums.pick('o'), Some(None));
        assert_eq!(View::Playlists.pick('O'), Some(by(Key::Original, true)));
        assert_eq!(by(Key::Original, true).unwrap().describe(), "newest first");
        // Keys of another view, and anything else, aren't listed.
        assert_eq!(View::Playlists.pick('t'), None);
        assert_eq!(View::Songs.pick('n'), None);
        assert_eq!(View::Songs.pick('x'), None);
        assert_eq!(View::Songs.pick('1'), None);
    }

    #[test]
    fn choices_round_trip_and_bad_files_are_ignored() {
        let dir = std::env::temp_dir().join(format!("uzu-sort-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("uzu.json");
        assert_eq!(load(&path), Prefs::default());
        let prefs = Prefs { playlists: by(Key::Name, true), albums: None, songs: by(Key::Duration, false), search: by(Key::Artist, false) };
        save(&path, &prefs); // creates the directories
        assert_eq!(load(&path), prefs);
        // Other keys in the file survive a save.
        std::fs::write(&path, r#"{"other":{"x":1},"sort":{"songs":{"key":"title","desc":true}}}"#).unwrap();
        assert_eq!(load(&path).songs, by(Key::Title, true));
        save(&path, &prefs);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"other\"") && text.contains("\"duration\""), "{text}");
        // A key that doesn't belong to its view is dropped; garbage is no choice at all.
        std::fs::write(&path, r#"{"sort":{"playlists":{"key":"title","desc":false},"albums":{"key":"year","desc":false}}}"#).unwrap();
        assert_eq!(load(&path), Prefs { albums: by(Key::Year, false), ..Default::default() });
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), Prefs::default());
        // A file that can't be written is ignored.
        save(&dir.join("nested").join("uzu.json").join("below-a-file"), &prefs);
        // Reversed original order is remembered; a file from before it still loads.
        let newest = Prefs { songs: by(Key::Original, true), playlists: by(Key::Original, true), ..Default::default() };
        save(&path, &newest);
        assert_eq!(load(&path), newest);
        std::fs::write(&path, r#"{"sort":{"songs":{"key":"title","desc":false},"search":null}}"#).unwrap();
        assert_eq!(load(&path).songs, by(Key::Title, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reversed_original_reverses_the_indices() {
        let songs = vec![song("a", "", "", 1.0), song("b", "", "", 1.0), song("c", "", "", 1.0)];
        assert_eq!(order_songs(&songs, by(Key::Original, true)), [2, 1, 0]);
        assert_eq!(order_songs(&songs, None), [0, 1, 2]);
    }

    #[test]
    fn pinned_playlists_stay_first_in_pin_order_under_any_sort() {
        let list = |id: &str, count: u32, pinned: Option<u32>| CollectionInfo { kind: Kind::Playlist, id: id.into(), title: id.into(), subtitle: String::new(), count: Some(count), favorites: false, editable: true, year: None, pinned };
        // The app sends the pinned ones first, in pin order.
        let items = vec![list("Zed", 5, Some(0)), list("Mid", 50, Some(1)), list("Bee", 20, None), list("Ant", 90, None), list("Cat", 1, None)];
        let names = |sort: Option<Sort>| order_collections(&items, sort).into_iter().map(|i| items[i].id.as_str()).collect::<Vec<_>>();
        let by = |key, desc| Some(Sort { key, desc });
        assert_eq!(names(None), ["Zed", "Mid", "Bee", "Ant", "Cat"]);
        assert_eq!(names(by(Key::Name, false)), ["Zed", "Mid", "Ant", "Bee", "Cat"]);
        assert_eq!(names(by(Key::Name, true)), ["Zed", "Mid", "Cat", "Bee", "Ant"]);
        assert_eq!(names(by(Key::Count, false)), ["Zed", "Mid", "Cat", "Bee", "Ant"]);
        assert_eq!(names(by(Key::Count, true)), ["Zed", "Mid", "Ant", "Bee", "Cat"]);
        assert_eq!(names(by(Key::Original, true)), ["Zed", "Mid", "Cat", "Ant", "Bee"]);
        // Without pins nothing changes (albums, signed-out).
        let plain: Vec<CollectionInfo> = items.iter().cloned().map(|c| CollectionInfo { pinned: None, ..c }).collect();
        assert_eq!(order_collections(&plain, by(Key::Name, false)).into_iter().map(|i| plain[i].id.as_str()).collect::<Vec<_>>(), ["Ant", "Bee", "Cat", "Mid", "Zed"]);
    }
}
