//! Music folder scanner and the on-disk index. CONTRACT (owned by the main session; the bodies
//! are the local-files agent's job):
//!
//! - `scan` walks every folder recursively (no hidden entries, symlinked dirs not followed),
//!   keeps files with an audio extension we can play (flac, mp3, m4a, aac, alac, ogg/oga, wav),
//!   and reads tags + duration with lofty. A file whose path, mtime and size match an entry of
//!   `previous` reuses it without opening the file, so a rescan of an unchanged library only
//!   stats files.
//! - Covers: the embedded front cover, else `cover|folder|front|album.(jpg|jpeg|png)` beside
//!   the file. One thumbnail per album (max 600 px, JPEG) in `art_dir`, named by a hash of
//!   album artist + album (else the folder). `LocalTrack::cover` is that file's path.
//! - Missing tags fall back: title → file stem, artist → "Unknown Artist", album → folder name.
//! - Runs on a background thread; `progress(done)` is called every 100 files or so.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey, Tag};

use serde::{Deserialize, Serialize};

use crate::models::Song;

/// One audio file in the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalTrack {
    /// Absolute path.
    pub path: PathBuf,
    /// Seconds since the epoch.
    pub mtime: u64,
    pub size: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// `album` is the folder's name, the file having no album tag.
    #[serde(default)]
    pub album_fallback: bool,
    /// Album artist when tagged (else the artist), for grouping albums.
    pub album_artist: String,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<String>,
    pub genre: Option<String>,
    /// Seconds.
    pub duration: f64,
    /// Thumbnail path in `art_dir`, when the file or its folder has a cover.
    pub cover: Option<PathBuf>,
}

impl LocalTrack {
    /// The `Song` the rest of the app uses: id `file://<path>`, artwork `file://<cover>`
    /// (template without `{w}`, so every size is the same file), `playable: true`.
    pub fn to_song(&self) -> Song {
        Song {
            id: Song::local_id(&self.path),
            catalog_id: None,
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            album_is_fallback: self.album_fallback,
            duration: self.duration,
            track_number: self.track,
            disc_number: self.disc,
            explicit: false,
            artwork: self.cover.as_ref().map(|c| crate::models::Artwork { template: format!("file://{}", c.display()), bg: None }),
            playable: true,
            genre: self.genre.clone(),
            year: self.year.clone(),
            album_id: None,
            artists: Vec::new(),
        }
    }
}

/// `$XDG_CACHE_HOME/uzumaki/local_index.json`.
pub fn index_path() -> PathBuf {
    super::cache_base().join("local_index.json")
}

/// `$XDG_CACHE_HOME/uzumaki/local_art/`.
pub fn art_dir() -> PathBuf {
    super::cache_base().join("local_art")
}

/// Version of the index file; another version reads as empty (a rescan rebuilds it).
const INDEX_VERSION: u32 = 2;

/// Longest side of a cover thumbnail, pixels.
const COVER_MAX: u32 = 600;

/// Extensions we can play (lowercase). Opus isn't decodable yet.
const EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "mp4", "aac", "alac", "ogg", "oga", "wav"];

/// Cover file names (without extension) looked for beside the audio files.
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album"];
const COVER_EXTS: &[&str] = &["jpg", "jpeg", "png"];

#[derive(Serialize)]
struct IndexOut<'a> {
    version: u32,
    tracks: &'a [LocalTrack],
}

#[derive(Deserialize)]
struct IndexIn {
    version: u32,
    tracks: Vec<LocalTrack>,
}

/// Reads the index; empty when missing, unreadable or of another version.
pub fn load_index(path: &Path) -> Vec<LocalTrack> {
    let Ok(bytes) = fs::read(path) else { return Vec::new() };
    match serde_json::from_slice::<IndexIn>(&bytes) {
        Ok(i) if i.version == INDEX_VERSION => i.tracks,
        _ => Vec::new(),
    }
}

/// Writes the index atomically (temp file + rename).
pub fn save_index(path: &Path, tracks: &[LocalTrack]) {
    let Ok(json) = serde_json::to_vec(&IndexOut { version: INDEX_VERSION, tracks }) else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if fs::write(&tmp, json).is_err() || fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Scans `folders` (see the module docs). Tracks come back sorted for display: album artist,
/// album, disc, track number, then title (case-insensitive). Files without a disc number sort
/// as disc 0 and files without a track number after the numbered ones.
pub fn scan(folders: &[PathBuf], previous: &[LocalTrack], art_dir: &Path, mut progress: impl FnMut(usize)) -> Vec<LocalTrack> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for folder in folders {
        let root = std::path::absolute(folder).unwrap_or_else(|_| folder.clone());
        walk(&root, &mut files, &mut seen);
    }

    let prev: HashMap<&Path, &LocalTrack> = previous.iter().map(|t| (t.path.as_path(), t)).collect();
    // Thumbnails of this scan by file stem (the album hash), shared by all tracks of the album.
    let mut covers: HashMap<String, PathBuf> = HashMap::new();
    let mut tracks = Vec::with_capacity(files.len());
    for (n, (path, mtime, size)) in files.into_iter().enumerate() {
        let reused = prev
            .get(path.as_path())
            .filter(|p| p.mtime == mtime && p.size == size && p.cover.as_ref().is_none_or(|c| c.exists()))
            .map(|p| (*p).clone());
        let track = match reused {
            Some(t) => {
                if let Some(c) = &t.cover {
                    if let Some(stem) = c.file_stem().and_then(|s| s.to_str()) {
                        covers.entry(stem.to_string()).or_insert_with(|| c.clone());
                    }
                }
                t
            }
            None => read_track(&path, mtime, size, art_dir, &mut covers),
        };
        tracks.push(track);
        if (n + 1) % 100 == 0 {
            progress(n + 1);
        }
    }
    progress(tracks.len());

    // A track without a cover of its own takes its album's thumbnail when another track made one.
    for t in &mut tracks {
        if t.cover.is_none() {
            let key = album_key(t, !t.album.is_empty());
            if let Some(c) = covers.get(&key) {
                t.cover = Some(c.clone());
            }
        }
    }

    tracks.sort_by(|a, b| {
        let key = |t: &LocalTrack| (t.album_artist.to_lowercase(), t.album.to_lowercase(), t.disc.unwrap_or(0), t.track.unwrap_or(u32::MAX), t.title.to_lowercase());
        key(a).cmp(&key(b)).then_with(|| a.path.cmp(&b.path))
    });
    tracks
}

/// Collects audio files below `dir`: (path, mtime seconds, size).
fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64, u64)>, seen: &mut HashSet<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if !is_music_app_library(&path) {
                walk(&path, out, seen);
            }
            continue;
        }
        // A symlink to a file counts, a symlink to a directory doesn't.
        let Ok(meta) = (if ft.is_symlink() { fs::metadata(&path) } else { entry.metadata() }) else { continue };
        if !meta.is_file() || !has_audio_extension(&path) {
            continue;
        }
        let mtime = meta.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        if seen.insert(path.clone()) {
            out.push((path, mtime, meta.len()));
        }
    }
}

/// macOS: `~/Music/Music`, the Music app's own library, is left out when `~/Music` is scanned
/// (it holds the Music app's copies; chosen as a folder itself, it is scanned).
fn is_music_app_library(dir: &Path) -> bool {
    cfg!(target_os = "macos")
        && std::env::var_os("HOME").is_some_and(|h| dir == Path::new(&h).join("Music/Music"))
}

fn has_audio_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn clean(s: Option<impl AsRef<str>>) -> Option<String> {
    s.map(|s| s.as_ref().trim().to_string()).filter(|s| !s.is_empty())
}

/// 64-bit FNV-1a as 16 hex chars (stable across runs and Rust versions, unlike `DefaultHasher`).
pub(crate) fn hash16(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Thumbnail name of an album: album artist + album, or the folder when there's no album tag.
fn album_key(t: &LocalTrack, has_album: bool) -> String {
    if has_album {
        hash16(&format!("{}\0{}", t.album_artist, t.album))
    } else {
        hash16(&t.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
    }
}

fn read_track(path: &Path, mtime: u64, size: u64, art_dir: &Path, covers: &mut HashMap<String, PathBuf>) -> LocalTrack {
    let tagged = lofty::read_from_path(path).ok();
    let tag: Option<&Tag> = tagged.as_ref().and_then(|f| f.primary_tag().or_else(|| f.first_tag()));
    let duration = tagged.as_ref().map_or(0.0, |f| f.properties().duration().as_secs_f64());

    let folder_name = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let album_tag = tag.and_then(|t| clean(t.album()));
    let artist_tag = tag.and_then(|t| clean(t.artist()));
    let artist = artist_tag.unwrap_or_else(|| "Unknown Artist".into());
    let album_artist = tag.and_then(|t| clean(t.get_string(ItemKey::AlbumArtist))).unwrap_or_else(|| artist.clone());
    let year = tag.and_then(|t| {
        t.date()
            .map(|d| d.year.to_string())
            .or_else(|| clean(t.get_string(ItemKey::Year).or_else(|| t.get_string(ItemKey::RecordingDate))).map(|y| y.chars().take(4).collect()))
    });

    let mut track = LocalTrack {
        path: path.to_path_buf(),
        mtime,
        size,
        title: tag.and_then(|t| clean(t.title())).unwrap_or(stem),
        artist,
        album: album_tag.clone().or(folder_name).unwrap_or_default(),
        album_fallback: album_tag.is_none(),
        album_artist,
        track: tag.and_then(|t| t.track()),
        disc: tag.and_then(|t| t.disk()),
        year,
        genre: tag.and_then(|t| clean(t.genre())),
        duration,
        cover: None,
    };

    let key = album_key(&track, album_tag.is_some());
    track.cover = match covers.get(&key) {
        Some(c) => Some(c.clone()),
        None => {
            let thumb = art_dir.join(format!("{key}.jpg"));
            if thumb.exists() {
                covers.insert(key, thumb.clone());
                Some(thumb)
            } else {
                let data = tag.and_then(embedded_cover).or_else(|| folder_cover(path));
                data.filter(|d| write_thumbnail(d, art_dir, &thumb)).map(|_| {
                    covers.insert(key, thumb.clone());
                    thumb
                })
            }
        }
    };
    track
}

/// The front cover bytes, else the first picture.
fn embedded_cover(tag: &Tag) -> Option<Vec<u8>> {
    let pic = tag.get_picture_type(PictureType::CoverFront).or_else(|| tag.pictures().first())?;
    Some(pic.data().to_vec())
}

/// `cover|folder|front|album.(jpg|jpeg|png)` beside the file (case-insensitive).
fn folder_cover(file: &Path) -> Option<Vec<u8>> {
    let dir = file.parent()?;
    let mut found: Vec<(usize, PathBuf)> = Vec::new();
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let (Some(stem), Some(ext)) = (p.file_stem().and_then(|s| s.to_str()), p.extension().and_then(|s| s.to_str())) else { continue };
        let (stem, ext) = (stem.to_ascii_lowercase(), ext.to_ascii_lowercase());
        if let Some(rank) = COVER_NAMES.iter().position(|n| *n == stem) {
            if COVER_EXTS.contains(&ext.as_str()) && p.is_file() {
                found.push((rank, p));
            }
        }
    }
    found.sort();
    found.into_iter().find_map(|(_, p)| fs::read(p).ok())
}

/// Decodes `data`, shrinks it to at most `COVER_MAX` px and saves a JPEG at `thumb` (atomically).
fn write_thumbnail(data: &[u8], art_dir: &Path, thumb: &Path) -> bool {
    let Ok(img) = image::load_from_memory(data) else { return false };
    let img = if img.width() > COVER_MAX || img.height() > COVER_MAX {
        img.resize(COVER_MAX, COVER_MAX, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    if fs::create_dir_all(art_dir).is_err() {
        return false;
    }
    let tmp = thumb.with_extension("jpg.part");
    let ok = img.to_rgb8().save_with_format(&tmp, image::ImageFormat::Jpeg).is_ok() && fs::rename(&tmp, thumb).is_ok();
    if !ok {
        let _ = fs::remove_file(&tmp);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::picture::{MimeType, Picture};
    use lofty::config::WriteOptions;
    use lofty::tag::{TagExt, TagType};

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("uzumaki-scan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 1 s of 8 kHz mono 16-bit silence.
    fn wav() -> Vec<u8> {
        let data = 16000u32;
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(8000u32.to_le_bytes());
        v.extend(16000u32.to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        v.extend(std::iter::repeat_n(0u8, data as usize));
        v
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 60]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// Writes a WAV at `path` (parents created), optionally tagged.
    fn make(path: &Path, tags: &[(&str, &str)], pic: Option<Vec<u8>>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, wav()).unwrap();
        if tags.is_empty() && pic.is_none() {
            return;
        }
        let mut tag = Tag::new(TagType::Id3v2);
        for (k, v) in tags {
            match *k {
                "title" => tag.set_title(v.to_string()),
                "artist" => tag.set_artist(v.to_string()),
                "album" => tag.set_album(v.to_string()),
                "genre" => tag.set_genre(v.to_string()),
                "track" => tag.set_track(v.parse().unwrap()),
                "disc" => tag.set_disk(v.parse().unwrap()),
                "albumartist" => {
                    tag.insert_text(ItemKey::AlbumArtist, v.to_string());
                }
                "year" => {
                    tag.insert_text(ItemKey::RecordingDate, v.to_string());
                }
                _ => panic!("tag {k}"),
            }
        }
        if let Some(p) = pic {
            tag.push_picture(Picture::unchecked(p).pic_type(PictureType::CoverFront).mime_type(MimeType::Png).build());
        }
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    }

    fn run(folders: &[PathBuf], prev: &[LocalTrack], art: &Path) -> Vec<LocalTrack> {
        scan(folders, prev, art, |_| {})
    }

    #[test]
    fn fallbacks_without_tags() {
        let d = tmp_dir("fallback");
        make(&d.join("Some Album").join("01 song.WAV"), &[], None);
        let t = run(&[d.clone()], &[], &d.join("art"));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].title, "01 song");
        assert_eq!(t[0].artist, "Unknown Artist");
        assert!(t[0].album_fallback, "no album tag: the folder name is a stand-in");
        assert_eq!(t[0].album, "Some Album");
        assert_eq!(t[0].album_artist, "Unknown Artist");
        assert!((t[0].duration - 1.0).abs() < 0.05, "{}", t[0].duration);
        assert_eq!(t[0].cover, None);
        assert!(t[0].size > 0 && t[0].mtime > 0);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn reads_tags() {
        let d = tmp_dir("tags");
        make(
            &d.join("x.wav"),
            &[("title", "Hello"), ("artist", "Me"), ("album", "Al"), ("albumartist", "Various"), ("track", "3"), ("disc", "2"), ("genre", "Pop"), ("year", "1999-05-01")],
            None,
        );
        let t = run(&[d.clone()], &[], &d.join("art"));
        let t = &t[0];
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str(), t.album_artist.as_str()), ("Hello", "Me", "Al", "Various"));
        assert_eq!((t.track, t.disc), (Some(3), Some(2)));
        assert_eq!(t.genre.as_deref(), Some("Pop"));
        assert_eq!(t.year.as_deref(), Some("1999"));
        let s = t.to_song();
        assert!(s.is_local());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn sorting() {
        let d = tmp_dir("sort");
        make(&d.join("1.wav"), &[("title", "b"), ("artist", "Zed"), ("album", "A"), ("track", "1")], None);
        make(&d.join("2.wav"), &[("title", "Z2"), ("artist", "abe"), ("album", "Y"), ("disc", "2"), ("track", "1")], None);
        make(&d.join("3.wav"), &[("title", "Z1"), ("artist", "abe"), ("album", "Y"), ("disc", "1"), ("track", "9")], None);
        make(&d.join("4.wav"), &[("title", "A0"), ("artist", "abe"), ("album", "y"), ("disc", "1"), ("track", "2")], None);
        let t = run(&[d.clone()], &[], &d.join("art"));
        let titles: Vec<_> = t.iter().map(|t| t.title.as_str()).collect();
        // abe/Y/d1/t2 (A0, album "y" sorts equal to "Y" case-insensitively) < t9 < disc 2 < Zed.
        assert_eq!(titles, ["A0", "Z1", "Z2", "b"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn skips_hidden_and_unsupported() {
        let d = tmp_dir("skip");
        make(&d.join("ok.wav"), &[], None);
        make(&d.join(".hidden.wav"), &[], None);
        make(&d.join(".hiddendir").join("a.wav"), &[], None);
        fs::write(d.join("notes.txt"), "x").unwrap();
        fs::write(d.join("song.opus"), "x").unwrap();
        fs::write(d.join("noext"), "x").unwrap();
        #[cfg(unix)]
        {
            let real = tmp_dir("skip-real");
            make(&real.join("linked.wav"), &[], None);
            std::os::unix::fs::symlink(&real, d.join("linkdir")).unwrap();
            let t = run(&[d.clone()], &[], &d.join("art"));
            assert_eq!(t.len(), 1, "{t:?}");
            let _ = fs::remove_dir_all(&real);
        }
        let t = run(&[d.clone()], &[], &d.join("art"));
        assert_eq!(t.len(), 1);
        assert!(t[0].path.ends_with("ok.wav"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unchanged_entries_are_reused_without_reading() {
        let d = tmp_dir("reuse");
        make(&d.join("a.wav"), &[("title", "Real")], None);
        let art = d.join("art");
        let mut first = run(&[d.clone()], &[], &art);
        first[0].title = "Fake".into();
        let again = run(&[d.clone()], &first, &art);
        assert_eq!(again[0].title, "Fake");
        // A changed size makes it re-read.
        first[0].size += 1;
        let again = run(&[d.clone()], &first, &art);
        assert_eq!(again[0].title, "Real");
        // So does a changed mtime.
        first[0].size -= 1;
        first[0].mtime += 1;
        assert_eq!(run(&[d.clone()], &first, &art)[0].title, "Real");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn reuse_needs_its_cover_file() {
        let d = tmp_dir("reusecover");
        make(&d.join("a.wav"), &[("title", "Real"), ("album", "Al"), ("artist", "Me")], Some(png(50, 50)));
        let art = d.join("art");
        let mut first = run(&[d.clone()], &[], &art);
        let cover = first[0].cover.clone().expect("cover");
        assert!(cover.exists());
        first[0].title = "Fake".into();
        assert_eq!(run(&[d.clone()], &first, &art)[0].title, "Fake");
        fs::remove_file(&cover).unwrap();
        let again = run(&[d.clone()], &first, &art);
        assert_eq!(again[0].title, "Real");
        assert!(again[0].cover.as_ref().unwrap().exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn embedded_cover_is_resized_and_shared_by_album() {
        let d = tmp_dir("cover");
        let art = d.join("art");
        let tags = [("album", "Al"), ("albumartist", "Me")];
        make(&d.join("a.wav"), &[tags[0], tags[1], ("title", "1")], Some(png(1200, 800)));
        make(&d.join("b.wav"), &[tags[0], tags[1], ("title", "2")], None);
        let t = run(&[d.clone()], &[], &art);
        assert_eq!(t[0].cover, t[1].cover);
        let cover = t[0].cover.clone().expect("cover");
        assert_eq!(cover.parent().unwrap(), art);
        let name = cover.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 16 + 4);
        assert_eq!(name, format!("{}.jpg", hash16("Me\0Al")));
        let img = image::open(&cover).unwrap();
        assert_eq!((img.width(), img.height()), (600, 400));
        assert_eq!(fs::read_dir(&art).unwrap().count(), 1);
        // An existing thumbnail isn't rewritten.
        fs::write(&cover, b"marker").unwrap();
        let t = run(&[d.clone()], &[], &art);
        assert_eq!(t[0].cover.as_ref(), Some(&cover));
        assert_eq!(fs::read(&cover).unwrap(), b"marker");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn folder_cover_fallback() {
        let d = tmp_dir("foldercover");
        let art = d.join("art");
        make(&d.join("alb").join("a.wav"), &[], None);
        fs::write(d.join("alb").join("Folder.PNG"), png(40, 40)).unwrap();
        let t = run(&[d.clone()], &[], &art);
        let c = t[0].cover.clone().expect("cover");
        assert!(c.starts_with(&art) && c.exists());
        // The cover file isn't an audio file, and a second folder without one has no cover.
        make(&d.join("other").join("b.wav"), &[], None);
        let t = run(&[d.clone()], &[], &art);
        assert_eq!(t.len(), 2);
        assert_eq!(t.iter().filter(|t| t.cover.is_some()).count(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unreadable_file_still_gets_an_entry() {
        let d = tmp_dir("broken");
        fs::write(d.join("junk.mp3"), b"not audio").unwrap();
        let t = run(&[d.clone()], &[], &d.join("art"));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].title, "junk");
        assert_eq!(t[0].duration, 0.0);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn progress_is_reported() {
        let d = tmp_dir("progress");
        for i in 0..3 {
            fs::write(d.join(format!("{i}.mp3")), b"x").unwrap();
        }
        let mut calls = Vec::new();
        scan(&[d.clone()], &[], &d.join("art"), |n| calls.push(n));
        assert_eq!(calls.last(), Some(&3));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn index_round_trip_and_version_mismatch() {
        let d = tmp_dir("index");
        let p = d.join("sub").join("idx.json");
        assert!(load_index(&p).is_empty());
        let t = vec![LocalTrack {
            path: "/m/a.flac".into(),
            mtime: 5,
            size: 6,
            title: "T".into(),
            artist: "A".into(),
            album: "B".into(),
            album_fallback: false,
            album_artist: "A".into(),
            track: Some(1),
            disc: None,
            year: Some("2001".into()),
            genre: None,
            duration: 12.5,
            cover: Some("/c.jpg".into()),
        }];
        save_index(&p, &t);
        assert_eq!(load_index(&p), t);
        assert!(!d.join("sub").join("idx.json.tmp").exists());
        fs::write(&p, r#"{"version": 2, "tracks": []}"#).unwrap();
        assert!(load_index(&p).is_empty());
        fs::write(&p, r#"{"version": 2, "tracks": [{"bogus": 1}]}"#).unwrap();
        assert!(load_index(&p).is_empty());
        fs::write(&p, "garbage").unwrap();
        assert!(load_index(&p).is_empty());
        let _ = fs::remove_dir_all(&d);
    }
}
