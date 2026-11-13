//! The music folders as Albums, Artists and Folders (local only mode's library tabs).
//!
//! Derived from the songs once per index change, on the scan thread (`from_index`), never in the
//! view: the pages only look things up. The index file is not changed by any of this; old index
//! files load as before, and missing tags are filled in here as well as by the scanner:
//! title → file name without its extension, artist → "Unknown Artist", album → the folder's name,
//! album artist → the artist.
//!
//! Lists hold song indices (`u32`) into `LocalLibrary::songs`, so a 20k-song library is kept once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::LocalTrack;
use super::scan::hash16;
use crate::models::{Artist, Collection, CollectionKind, Song};
use crate::ui::fold;

/// Id prefix of a local album (`Route::Collection`), then a hash of its grouping key.
pub const ALBUM_PREFIX: &str = "local-album:";
/// Id prefix of a local artist (`Route::Artist`), then the folded name.
pub const ARTIST_PREFIX: &str = "local-artist:";
pub const UNKNOWN_ARTIST: &str = "Unknown Artist";

pub fn is_album_id(id: &str) -> bool {
    id.starts_with(ALBUM_PREFIX)
}

pub fn is_artist_id(id: &str) -> bool {
    id.starts_with(ARTIST_PREFIX)
}

/// The local artist called `name` (any case or accents).
pub fn artist_id(name: &str) -> String {
    format!("{ARTIST_PREFIX}{}", fold(name))
}

/// The artist link of a local song's artist name.
pub fn artist_of(song: &Song) -> Artist {
    Artist { id: artist_id(&song.artist), name: song.artist.clone() }
}

/// A folder as on disk: its subfolders (by name), its own songs (by file name) and how many
/// songs it holds with everything below it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folder {
    pub dirs: Vec<PathBuf>,
    pub files: Vec<u32>,
    pub total: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalView {
    /// By title, then album artist. `tracks` is empty (the songs are `album_songs`);
    /// `track_count` is set.
    pub albums: Vec<Collection>,
    /// Each album's songs, by disc, track, title.
    pub album_songs: Vec<Vec<u32>>,
    /// The album of each song.
    pub song_album: Vec<u32>,
    /// Album artists and song artists, by name.
    pub artists: Vec<Artist>,
    /// Each artist's albums (as album artist), oldest first.
    pub artist_albums: Vec<Vec<u32>>,
    /// Each artist's songs: their albums' songs in album order, then songs on other albums.
    pub artist_songs: Vec<Vec<u32>>,
    /// Every folder that holds songs, and every folder between it and its music folder.
    pub folders: HashMap<PathBuf, Folder>,
    /// The music folders, in the settings' order.
    pub roots: Vec<PathBuf>,
    album_ix: HashMap<String, u32>,
    artist_ix: HashMap<String, u32>,
}

/// The songs of the index entries inside `folders` (a removed folder's entries may still be in
/// an older index), and the view over them.
pub fn from_index(tracks: &[LocalTrack], folders: &[PathBuf]) -> (Vec<Song>, LocalView) {
    let inside = |p: &Path| folders.iter().any(|f| p.starts_with(f));
    let kept: Vec<&LocalTrack> = tracks.iter().filter(|t| inside(&t.path)).collect();
    let mut songs: Vec<Song> = kept.iter().map(|t| t.to_song()).collect();
    let album_artists: Vec<String> = kept.iter().map(|t| t.album_artist.clone()).collect();
    let view = derive(&mut songs, &album_artists, folders);
    (songs, view)
}

fn clean(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty()).then_some(s)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Fills in missing tags, sets each song's `album_id` and builds the view. `album_artists[i]` is
/// song `i`'s album artist ("" when unknown).
pub fn derive(songs: &mut [Song], album_artists: &[String], roots: &[PathBuf]) -> LocalView {
    let n = songs.len();
    let mut aa: Vec<String> = Vec::with_capacity(n);
    // Grouping key → album index (in first-seen order, sorted below).
    let mut groups: HashMap<String, usize> = HashMap::new();
    let mut albums: Vec<Collection> = Vec::new();
    let mut members: Vec<Vec<u32>> = Vec::new();
    let mut song_album = vec![0u32; n];

    for (i, song) in songs.iter_mut().enumerate() {
        let path = song.local_path().map(Path::to_path_buf).unwrap_or_default();
        let folder = path.parent().map(file_name).unwrap_or_default();
        if clean(&song.title).is_none() {
            song.title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        if clean(&song.artist).is_none() {
            song.artist = UNKNOWN_ARTIST.into();
        }
        if clean(&song.album).is_none() {
            song.album = folder.clone();
            song.album_is_fallback = true;
        }
        let album_artist = album_artists.get(i).and_then(|a| clean(a)).unwrap_or(&song.artist).to_string();
        // An untagged album is its folder (named after it, by Unknown Artist): two folders called
        // "CD1" stay two albums. A tagged album keeps its discs' folders together.
        let mut key = format!("{}\0{}", fold(&album_artist), fold(&song.album));
        if song.album == folder && album_artist == UNKNOWN_ARTIST {
            key.push('\0');
            key.push_str(&path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
        }
        let k = *groups.entry(key).or_insert_with_key(|key| {
            albums.push(Collection {
                kind: CollectionKind::Album,
                id: format!("{ALBUM_PREFIX}{}", hash16(key)),
                title: song.album.clone(),
                subtitle: album_artist.clone(),
                artwork: None,
                year: None,
                genre: None,
                description: None,
                track_count: None,
                modified: None,
                editable: false,
                favorites: false,
                public: None,
                artists: vec![Artist { id: artist_id(&album_artist), name: album_artist.clone() }],
                tracks: Vec::new(),
            });
            members.push(Vec::new());
            albums.len() - 1
        });
        members[k].push(i as u32);
        aa.push(album_artist);
    }

    // Each album's songs by disc, track, title; its cover, year and genre from the first that has one.
    for (k, list) in members.iter_mut().enumerate() {
        list.sort_by_cached_key(|&i| {
            let s = &songs[i as usize];
            (s.disc_number.unwrap_or(0), s.track_number.unwrap_or(u32::MAX), fold(&s.title), s.id.clone())
        });
        let c = &mut albums[k];
        let first = |f: &dyn Fn(&Song) -> bool| list.iter().map(|&i| &songs[i as usize]).find(|s| f(s));
        c.artwork = first(&|s| s.artwork.is_some()).and_then(|s| s.artwork.clone());
        c.year = first(&|s| s.year.is_some()).and_then(|s| s.year.clone());
        c.genre = first(&|s| s.genre.is_some()).and_then(|s| s.genre.clone());
        c.track_count = Some(list.len() as u32);
    }

    // Albums by title, then album artist.
    let mut order: Vec<usize> = (0..albums.len()).collect();
    order.sort_by_cached_key(|&k| (fold(&albums[k].title), fold(&albums[k].subtitle), albums[k].id.clone()));
    let mut rank = vec![0u32; albums.len()];
    for (pos, &k) in order.iter().enumerate() {
        rank[k] = pos as u32;
    }
    let mut taken: Vec<Option<Collection>> = albums.into_iter().map(Some).collect();
    let albums: Vec<Collection> = order.iter().map(|&k| taken[k].take().expect("each album once")).collect();
    let album_songs: Vec<Vec<u32>> = order.iter().map(|&k| std::mem::take(&mut members[k])).collect();
    let mut song_pos = vec![0u32; n];
    for (a, list) in album_songs.iter().enumerate() {
        for (p, &i) in list.iter().enumerate() {
            song_album[i as usize] = a as u32;
            song_pos[i as usize] = p as u32;
        }
    }
    for (i, song) in songs.iter_mut().enumerate() {
        song.album_id = Some(albums[song_album[i] as usize].id.clone());
    }
    let album_ix: HashMap<String, u32> = albums.iter().enumerate().map(|(a, c)| (c.id.clone(), a as u32)).collect();

    // Artists: every album artist and every song artist.
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    let mut artist = |name: &str| -> usize {
        *by_name.entry(fold(name)).or_insert_with(|| {
            names.push(name.to_string());
            names.len() - 1
        })
    };
    let album_artist_ix: Vec<usize> = albums.iter().map(|c| artist(&c.subtitle)).collect();
    let song_artist_ix: Vec<(usize, usize)> = songs.iter().enumerate().map(|(i, song)| (artist(&aa[i]), artist(&song.artist))).collect();
    let mut a_albums: Vec<Vec<u32>> = vec![Vec::new(); names.len()];
    let mut a_songs: Vec<Vec<u32>> = vec![Vec::new(); names.len()];
    for (a, &k) in album_artist_ix.iter().enumerate() {
        a_albums[k].push(a as u32);
    }
    for (i, &(k, j)) in song_artist_ix.iter().enumerate() {
        a_songs[k].push(i as u32);
        if j != k {
            a_songs[j].push(i as u32);
        }
    }
    let mut artist_order: Vec<usize> = (0..names.len()).collect();
    artist_order.sort_by_cached_key(|&k| (fold(&names[k]), k));
    let mut artists = Vec::with_capacity(names.len());
    let mut artist_albums = Vec::with_capacity(names.len());
    let mut artist_songs = Vec::with_capacity(names.len());
    for &k in &artist_order {
        let mut own = std::mem::take(&mut a_albums[k]);
        own.sort_by_cached_key(|&a| {
            let c = &albums[a as usize];
            (c.year.is_none(), c.year.clone(), fold(&c.title))
        });
        // Their own albums first (in `own`'s order), then the albums they appear on.
        let place: HashMap<u32, usize> = own.iter().enumerate().map(|(p, &a)| (a, p)).collect();
        let mut list = std::mem::take(&mut a_songs[k]);
        list.sort_by_key(|&i| {
            let a = song_album[i as usize];
            (place.get(&a).copied().unwrap_or(usize::MAX), a, song_pos[i as usize])
        });
        artists.push(Artist { id: artist_id(&names[k]), name: names[k].clone() });
        artist_albums.push(own);
        artist_songs.push(list);
    }
    let artist_ix: HashMap<String, u32> = artists.iter().enumerate().map(|(a, x)| (x.id.clone(), a as u32)).collect();

    let folders = folder_tree(songs, roots);
    LocalView {
        albums,
        album_songs,
        song_album,
        artists,
        artist_albums,
        artist_songs,
        folders,
        roots: roots.to_vec(),
        album_ix,
        artist_ix,
    }
}

/// The folders below each music folder that hold songs (and those in between).
fn folder_tree(songs: &[Song], roots: &[PathBuf]) -> HashMap<PathBuf, Folder> {
    let mut tree: HashMap<PathBuf, Folder> = roots.iter().map(|r| (r.clone(), Folder::default())).collect();
    for (i, song) in songs.iter().enumerate() {
        let Some(path) = song.local_path() else { continue };
        // The innermost music folder holding it.
        let Some(root) = roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.components().count()) else { continue };
        let Some(dir) = path.parent() else { continue };
        tree.entry(dir.to_path_buf()).or_default().files.push(i as u32);
        let mut d = dir;
        loop {
            tree.entry(d.to_path_buf()).or_default().total += 1;
            if d == root.as_path() {
                break;
            }
            let Some(up) = d.parent() else { break };
            let parent = tree.entry(up.to_path_buf()).or_default();
            if !parent.dirs.iter().any(|x| x == d) {
                parent.dirs.push(d.to_path_buf());
            }
            d = up;
        }
    }
    for f in tree.values_mut() {
        f.dirs.sort_by_cached_key(|d| (fold(&file_name(d)), d.clone()));
        f.files.sort_by_cached_key(|&i| {
            let p = songs[i as usize].local_path().map(Path::to_path_buf).unwrap_or_default();
            (fold(&file_name(&p)), p)
        });
    }
    tree
}

impl LocalView {
    pub fn album(&self, id: &str) -> Option<usize> {
        self.album_ix.get(id).map(|&a| a as usize)
    }

    pub fn artist(&self, id: &str) -> Option<usize> {
        self.artist_ix.get(id).map(|&a| a as usize)
    }

    pub fn folder(&self, path: &Path) -> Option<&Folder> {
        self.folders.get(path)
    }

    /// A folder's songs as it plays: its own files, then each subfolder's, by name.
    pub fn folder_songs(&self, path: &Path) -> Vec<u32> {
        let mut out = Vec::new();
        self.collect(path, &mut out);
        out
    }

    fn collect(&self, path: &Path, out: &mut Vec<u32>) {
        if let Some(f) = self.folders.get(path) {
            out.extend_from_slice(&f.files);
            for d in &f.dirs {
                self.collect(d, out);
            }
        }
    }

    /// The music folder holding `path` (itself for a music folder).
    pub fn root_of(&self, path: &Path) -> Option<&PathBuf> {
        self.roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.components().count())
    }
}

/// `list`'s songs, cloned out of `songs` (the pages need them as a slice).
pub fn pick(songs: &[Song], list: &[u32]) -> Vec<Song> {
    list.iter().filter_map(|&i| songs.get(i as usize).cloned()).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An index entry under `/m` (the music folder). `tags`: title, artist, album, album artist
    /// ("" for none), disc and track.
    pub(crate) fn track(path: &str, title: &str, artist: &str, album: &str, album_artist: &str, disc: Option<u32>, track: Option<u32>) -> LocalTrack {
        LocalTrack {
            path: PathBuf::from(path),
            mtime: 1,
            size: 1,
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_fallback: false,
            album_artist: album_artist.into(),
            track,
            disc,
            year: None,
            genre: None,
            duration: 180.0,
            cover: None,
        }
    }

    fn view(tracks: &[LocalTrack]) -> (Vec<Song>, LocalView) {
        from_index(tracks, &[PathBuf::from("/m")])
    }

    fn titles(songs: &[Song], list: &[u32]) -> Vec<String> {
        list.iter().map(|&i| songs[i as usize].title.clone()).collect()
    }

    #[test]
    fn an_album_named_after_its_folder_is_marked_a_stand_in() {
        let mut bare = track("/m/Music/a.mp3", "A", "Unknown Artist", "", "", None, None);
        bare.album_fallback = true;
        let (songs, _) = view(&[bare, track("/m/Music/b.mp3", "B", "Band", "Music", "", None, None)]);
        assert!(songs[0].album_is_fallback && songs[0].album == "Music");
        assert!(!songs[1].album_is_fallback, "a real album called Music is not a stand-in");
        assert_eq!(songs[0].tile_name(), "A");
        assert_eq!(songs[1].tile_name(), "Music");
    }

    #[test]
    fn albums_group_by_album_artist_and_album_with_songs_by_disc_and_track() {
        let (songs, v) = view(&[
            track("/m/B/2-01.flac", "Late", "Band", "Big", "", Some(2), Some(1)),
            track("/m/B/1-02.flac", "Second", "Band", "Big", "", Some(1), Some(2)),
            track("/m/B/1-01.flac", "First", "Band", "Big", "", Some(1), Some(1)),
            track("/m/B/bonus.flac", "Bonus", "Band", "big", "", Some(1), None),
            // A tagged album named like its folder, its second disc in a subfolder: one album.
            track("/m/Big/a.flac", "Big A", "Band", "Big", "", Some(3), Some(1)),
            track("/m/Big/CD4/b.flac", "Big B", "Band", "Big", "", Some(4), Some(1)),
            // Same album name, another album artist: another album.
            track("/m/C/x.flac", "Other", "Choir", "Big", "", None, Some(1)),
            // A guest song on a compilation keeps to the compilation.
            track("/m/V/1.flac", "Guest", "Band", "Hits", "Various Artists", None, Some(1)),
            track("/m/V/2.flac", "Host", "Choir", "Hits", "Various Artists", None, Some(2)),
        ]);
        let names: Vec<_> = v.albums.iter().map(|c| (c.title.as_str(), c.subtitle.as_str())).collect();
        assert_eq!(names, [("Big", "Band"), ("Big", "Choir"), ("Hits", "Various Artists")]);
        assert_eq!(titles(&songs, &v.album_songs[0]), ["First", "Second", "Bonus", "Late", "Big A", "Big B"]);
        assert_eq!(v.albums[0].track_count, Some(6));
        assert!(is_album_id(&v.albums[0].id));
        assert_eq!(v.albums[0].artists, [Artist { id: artist_id("Band"), name: "Band".into() }]);
        // Every song knows its album; lookups by id work.
        for (i, s) in songs.iter().enumerate() {
            let a = v.song_album[i] as usize;
            assert_eq!(s.album_id.as_deref(), Some(v.albums[a].id.as_str()));
            assert_eq!(v.album(&v.albums[a].id), Some(a));
        }
    }

    #[test]
    fn untagged_files_take_the_folder_name_and_unknown_artist() {
        // An old index entry with empty fields, and two untagged folders both called "CD1".
        let (songs, v) = view(&[
            track("/m/Live/CD1/01 Intro.mp3", "", "", "", "", None, None),
            track("/m/Live/CD1/02 Song.mp3", "", "", "", "", None, None),
            track("/m/Studio/CD1/01 Other.mp3", "01 Other", "Unknown Artist", "CD1", "Unknown Artist", None, None),
        ]);
        assert_eq!(songs[0].title, "01 Intro");
        assert_eq!(songs[0].artist, UNKNOWN_ARTIST);
        assert_eq!(songs[0].album, "CD1");
        assert_eq!(v.albums.len(), 2, "two folders named CD1 are two albums");
        assert!(v.albums.iter().all(|c| c.title == "CD1" && c.subtitle == UNKNOWN_ARTIST));
        assert_eq!(v.artists.len(), 1);
        assert_eq!(v.artists[0].name, UNKNOWN_ARTIST);
        assert_eq!(v.artist_songs[0].len(), 3);
        assert_eq!(v.artist_albums[0].len(), 2);
    }

    #[test]
    fn artists_come_from_album_artists_and_song_artists() {
        let (songs, v) = view(&[
            track("/m/A/1.flac", "One", "Ann", "Debut", "", None, Some(1)),
            track("/m/A/2.flac", "Two", "Ann feat. Bo", "Debut", "Ann", None, Some(2)),
            track("/m/Z/1.flac", "Old", "Ann", "Zero", "", None, Some(1)),
            track("/m/V/1.flac", "Guest", "ann", "Hits", "Various Artists", None, Some(1)),
        ]);
        let names: Vec<_> = v.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Ann", "Ann feat. Bo", "Various Artists"]);
        let ann = v.artist(&artist_id("ANN")).unwrap();
        // Their own albums (Debut, Zero: no years, so by title), then the compilation.
        let own: Vec<_> = v.artist_albums[ann].iter().map(|&a| v.albums[a as usize].title.as_str()).collect();
        assert_eq!(own, ["Debut", "Zero"]);
        assert_eq!(titles(&songs, &v.artist_songs[ann]), ["One", "Two", "Old", "Guest"]);
        let feat = v.artist(&artist_id("Ann feat. Bo")).unwrap();
        assert!(v.artist_albums[feat].is_empty());
        assert_eq!(titles(&songs, &v.artist_songs[feat]), ["Two"]);
    }

    #[test]
    fn albums_of_an_artist_go_oldest_first() {
        let mut t = vec![
            track("/m/a/1.flac", "x", "Ann", "New", "", None, None),
            track("/m/b/1.flac", "y", "Ann", "Old", "", None, None),
            track("/m/c/1.flac", "z", "Ann", "Undated", "", None, None),
        ];
        t[0].year = Some("2020".into());
        t[1].year = Some("1999".into());
        let (_, v) = view(&t);
        let ann = v.artist(&artist_id("Ann")).unwrap();
        let own: Vec<_> = v.artist_albums[ann].iter().map(|&a| v.albums[a as usize].title.as_str()).collect();
        assert_eq!(own, ["Old", "New", "Undated"]);
        assert_eq!(v.albums.iter().find(|c| c.title == "New").unwrap().year.as_deref(), Some("2020"));
    }

    #[test]
    fn the_folder_tree_lists_folders_then_files_and_plays_in_that_order() {
        let (songs, v) = view(&[
            track("/m/b.mp3", "b", "", "", "", None, None),
            track("/m/Rock/Z/1.mp3", "z1", "", "", "", None, None),
            track("/m/Rock/a.mp3", "ra", "", "", "", None, None),
            track("/m/Rock/B/1.mp3", "b1", "", "", "", None, None),
            track("/m/A.mp3", "a", "", "", "", None, None),
            track("/elsewhere/x.mp3", "out", "", "", "", None, None),
        ]);
        assert_eq!(songs.len(), 5, "files outside the music folders are left out");
        let root = v.folder(Path::new("/m")).unwrap();
        assert_eq!(root.dirs, [PathBuf::from("/m/Rock")]);
        assert_eq!(titles(&songs, &root.files), ["a", "b"]);
        assert_eq!(root.total, 5);
        let rock = v.folder(Path::new("/m/Rock")).unwrap();
        assert_eq!(rock.dirs, [PathBuf::from("/m/Rock/B"), PathBuf::from("/m/Rock/Z")]);
        assert_eq!(rock.total, 3);
        assert_eq!(titles(&songs, &v.folder_songs(Path::new("/m"))), ["a", "b", "ra", "b1", "z1"]);
        assert_eq!(v.root_of(Path::new("/m/Rock/B")), Some(&PathBuf::from("/m")));
        assert!(v.folder(Path::new("/elsewhere")).is_none());
    }

    #[test]
    fn an_empty_music_folder_still_shows() {
        let (_, v) = from_index(&[], &[PathBuf::from("/m"), PathBuf::from("/n")]);
        assert_eq!(v.roots.len(), 2);
        assert_eq!(v.folder(Path::new("/n")).map(|f| f.total), Some(0));
        assert!(v.albums.is_empty() && v.artists.is_empty());
    }

    /// The derivation of a large library stays cheap (it runs once per scan, off the UI thread).
    #[test]
    fn twenty_thousand_songs_derive_quickly() {
        let tracks = synthetic(20_000);
        let t = std::time::Instant::now();
        let (songs, v) = view(&tracks);
        let took = t.elapsed();
        assert_eq!(songs.len(), 20_000);
        assert!(v.albums.len() > 1000);
        eprintln!("derive 20k: {took:?}, {} albums, {} artists, {} folders", v.albums.len(), v.artists.len(), v.folders.len());
        assert!(took.as_secs_f64() < 2.0, "{took:?}");
    }

    /// `n` songs under `/m`: 12 per album, 8 albums per artist, every 10th album untagged.
    pub(crate) fn synthetic(n: usize) -> Vec<LocalTrack> {
        (0..n)
            .map(|i| {
                let (album, artist) = (i / 12, i / 96);
                if album % 10 == 9 {
                    track(&format!("/m/Unsorted/rip {album}/track {:02}.mp3", i % 12 + 1), "", "", "", "", None, None)
                } else {
                    let mut t = track(
                        &format!("/m/Artist {artist}/Album {album}/{:02} Song {i}.flac", i % 12 + 1),
                        &format!("Song {i}"),
                        &format!("Artist {artist}"),
                        &format!("Album {album}"),
                        "",
                        Some(1),
                        Some((i % 12 + 1) as u32),
                    );
                    t.year = Some(format!("{}", 1970 + album % 50));
                    t
                }
            })
            .collect()
    }
}
