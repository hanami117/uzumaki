//! Lyrics of a local file, read from disk on a background thread (`State::want_lyrics`).
//!
//! Looked for in this order, synced lyrics winning over plain ones:
//! 1. a `.lrc` beside the file (same name; `.LRC` too);
//! 2. the file's own tags: an ID3 `SYLT` frame (millisecond stamps), then the lyrics tag
//!    (`USLT`, Vorbis `LYRICS`/`UNSYNCEDLYRICS`, MP4 `©lyr`), whose text is read as LRC when it
//!    carries time stamps;
//! 3. a `.txt` beside the file (plain lyrics).
//!
//! Sidecar files over 1 MB are skipped. Nothing here goes to Apple.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use lofty::file::TaggedFileExt;
use lofty::id3::v2::{Frame, FrameId, SynchronizedTextFrame, TimestampFormat};
use lofty::tag::ItemKey;

use crate::lyrics::{Line, Lyrics, parse_lrc};

/// Largest sidecar file read, bytes.
const SIDECAR_MAX: u64 = 1 << 20;

/// The lyrics of the local file at `file`, if it has any (see the module docs).
pub fn read(file: &Path) -> Option<Lyrics> {
    let mut plain: Option<Lyrics> = None;
    let mut take = |l: Option<Lyrics>| -> Option<Lyrics> {
        match l {
            Some(l) if l.synced => Some(l),
            Some(l) => {
                plain.get_or_insert(l);
                None
            }
            None => None,
        }
    };
    if let Some(l) = take(sidecar(file, &["lrc", "LRC"])) {
        return Some(l);
    }
    if let Some(l) = take(sylt(file)) {
        return Some(l);
    }
    if let Some(l) = take(tag_text(file)) {
        return Some(l);
    }
    if let Some(l) = take(sidecar(file, &["txt", "TXT"])) {
        return Some(l);
    }
    plain
}

/// A sidecar file with one of `exts` beside `file`, parsed as LRC (plain text reads as plain).
fn sidecar(file: &Path, exts: &[&str]) -> Option<Lyrics> {
    let path: PathBuf = exts.iter().map(|e| file.with_extension(e)).find(|p| p.is_file())?;
    if std::fs::metadata(&path).ok()?.len() > SIDECAR_MAX {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    parse_lrc(&String::from_utf8_lossy(&bytes))
}

/// The lyrics tag of the file (any of its tags), as LRC or plain text.
fn tag_text(file: &Path) -> Option<Lyrics> {
    let tagged = lofty::read_from_path(file).ok()?;
    let text = tagged.tags().iter().find_map(|t| {
        [ItemKey::Lyrics, ItemKey::UnsyncLyrics].into_iter().find_map(|k| t.get_string(k).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
    })?;
    parse_lrc(&text)
}

/// An ID3v2 `SYLT` frame with millisecond stamps (MP3 files only).
fn sylt(file: &Path) -> Option<Lyrics> {
    if !file.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mp3")) {
        return None;
    }
    let mut f = std::fs::File::open(file).ok()?;
    let mp = <lofty::mpeg::MpegFile as lofty::file::AudioFile>::read_from(&mut f, lofty::config::ParseOptions::new()).ok()?;
    let tag = mp.id3v2()?;
    let id = FrameId::Valid(Cow::Borrowed("SYLT"));
    let Some(Frame::Binary(b)) = tag.get(&id) else { return None };
    let frame = SynchronizedTextFrame::parse(&b.data, b.flags()).ok()?;
    from_sylt(&frame)
}

fn from_sylt(frame: &SynchronizedTextFrame<'_>) -> Option<Lyrics> {
    if frame.timestamp_format != TimestampFormat::MS {
        return None;
    }
    let mut stamps: Vec<(f64, String)> = frame
        .content
        .iter()
        .map(|(ms, text)| (f64::from(*ms) / 1000.0, text.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect();
    stamps.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut lines = Vec::new();
    for (i, (start, text)) in stamps.iter().enumerate() {
        if text.is_empty() {
            continue;
        }
        let end = stamps.get(i + 1).map_or(start + 5.0, |n| n.0);
        lines.push(Line { start: *start, end, text: text.clone(), ..Line::default() });
    }
    (!lines.is_empty()).then_some(Lyrics { synced: true, lines, credit: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::config::WriteOptions;
    use lofty::id3::v2::{BinaryFrame, Id3v2Tag, SyncTextContentType};
    use lofty::TextEncoding;
    use lofty::tag::{Tag, TagExt, TagType};
    use std::fs;

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("uzumaki-lyrics-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A few silent MPEG-1 Layer III frames (128 kbps, 44.1 kHz): enough for lofty to read.
    fn mp3() -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..8 {
            let mut frame = vec![0u8; 417];
            frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
            v.extend(frame);
        }
        v
    }

    #[test]
    fn a_sidecar_lrc_wins_and_a_txt_is_plain() {
        let d = tmp_dir("sidecar");
        let song = d.join("a.flac");
        fs::write(&song, b"x").unwrap();
        assert_eq!(read(&song), None);
        fs::write(d.join("a.txt"), "First line\nSecond line\n").unwrap();
        let l = read(&song).unwrap();
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 2);
        fs::write(d.join("a.lrc"), "[00:01.00]Hello\n[00:03.50]World\n").unwrap();
        let l = read(&song).unwrap();
        assert!(l.synced);
        assert_eq!(l.lines[1].text, "World");
        assert!((l.lines[1].start - 3.5).abs() < 1e-9);
        // Too big: skipped (the .txt is read instead).
        fs::write(d.join("a.lrc"), vec![b'a'; (SIDECAR_MAX + 1) as usize]).unwrap();
        assert!(!read(&song).unwrap().synced);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn embedded_lyrics_tags_are_read_as_lrc_or_plain() {
        let d = tmp_dir("uslt");
        let song = d.join("a.mp3");
        fs::write(&song, mp3()).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::UnsyncLyrics, "[00:02.00]Stamped line\n[00:04.00]Next".into());
        tag.save_to_path(&song, WriteOptions::default()).unwrap();
        let l = read(&song).expect("lyrics");
        assert!(l.synced);
        assert_eq!(l.lines[0].text, "Stamped line");
        // A synced .lrc beats them; a plain .txt doesn't beat synced tags.
        fs::write(d.join("a.txt"), "Plain").unwrap();
        assert!(read(&song).unwrap().synced);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn an_id3_sylt_frame_is_synced() {
        let d = tmp_dir("sylt");
        let song = d.join("a.mp3");
        fs::write(&song, mp3()).unwrap();
        let sylt = SynchronizedTextFrame::new(
            TextEncoding::UTF8,
            *b"eng",
            TimestampFormat::MS,
            SyncTextContentType::Lyrics,
            None,
            vec![(4000, "Two".into()), (1500, "One".into())],
        );
        let bytes = sylt.as_bytes(WriteOptions::default()).unwrap();
        let mut tag = Id3v2Tag::new();
        tag.insert(Frame::Binary(BinaryFrame::new(FrameId::Valid(Cow::Borrowed("SYLT")), bytes)));
        tag.save_to_path(&song, WriteOptions::default()).unwrap();
        let l = read(&song).expect("lyrics");
        assert!(l.synced);
        let texts: Vec<_> = l.lines.iter().map(|x| (x.start, x.text.as_str())).collect();
        assert_eq!(texts, [(1.5, "One"), (4.0, "Two")]);
        let _ = fs::remove_dir_all(&d);
    }
}
