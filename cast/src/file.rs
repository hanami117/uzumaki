//! Local files the device plays itself (`BUFFERED`): the TV is the player, so there is no live
//! buffer to wait for, seeks are the receiver's own `SEEK` and the file's own quality is kept.
//!
//! Only formats the Default Media Receiver plays are sent this way ([`probe`]); anything else (ALAC,
//! AIFF, FLAC above 96 kHz/24-bit, …) goes through the live stream as before. The files are the
//! user's own and are served, as they are, only to the device (same token and allow-list as the
//! live stream), with HTTP ranges so the receiver can seek.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::Path;

/// The content type to `LOAD` `path` with, if the receiver plays it.
pub fn probe(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mut f = File::open(path).ok()?;
    match ext.as_str() {
        "mp3" => Some("audio/mpeg"),
        "flac" => flac_ok(&mut f).then_some("audio/flac"),
        "ogg" | "oga" | "opus" => magic(&mut f, 0, b"OggS").then_some("audio/ogg"),
        "wav" => wav_ok(&mut f).then_some("audio/wav"),
        "m4a" | "mp4" | "aac" if magic(&mut f, 4, b"ftyp") => mp4_codec(&mut f).filter(|c| c == b"mp4a").map(|_| "audio/mp4"),
        "aac" => Some("audio/aac"),
        _ => None,
    }
}

/// The extension to serve a content type as (the receiver doesn't need it; it helps the logs).
pub fn ext_of(content_type: &str) -> &'static str {
    match content_type {
        "audio/mpeg" => "mp3",
        "audio/flac" => "flac",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "audio/aac" => "aac",
        _ => "m4a",
    }
}

fn magic(f: &mut File, at: u64, want: &[u8]) -> bool {
    let mut b = vec![0u8; want.len()];
    f.seek(SeekFrom::Start(at)).is_ok() && f.read_exact(&mut b).is_ok() && b == want
}

/// An ID3v2 tag in front (some FLAC files have one): its length.
fn id3_len(f: &mut File) -> u64 {
    let mut h = [0u8; 10];
    if f.seek(SeekFrom::Start(0)).is_err() || f.read_exact(&mut h).is_err() || &h[..3] != b"ID3" {
        return 0;
    }
    10 + h[6..10].iter().fold(0u64, |a, b| (a << 7) | u64::from(b & 0x7f))
}

/// FLAC up to 96 kHz, 24-bit, stereo (what Cast documents).
fn flac_ok(f: &mut File) -> bool {
    let start = id3_len(f);
    let mut b = [0u8; 4 + 4 + 18];
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut b).is_err() || &b[..4] != b"fLaC" {
        return false;
    }
    let si = &b[8..];
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let channels = ((si[12] >> 1) & 7) + 1;
    let bits = (((si[12] & 1) << 4) | (si[13] >> 4)) + 1;
    (8000..=96_000).contains(&rate) && channels <= 2 && (16..=24).contains(&bits)
}

/// PCM WAV, 16 or 24-bit, up to 96 kHz, stereo.
fn wav_ok(f: &mut File) -> bool {
    let mut h = [0u8; 12];
    if f.seek(SeekFrom::Start(0)).is_err() || f.read_exact(&mut h).is_err() || &h[..4] != b"RIFF" || &h[8..] != b"WAVE" {
        return false;
    }
    for _ in 0..16 {
        let mut c = [0u8; 8];
        if f.read_exact(&mut c).is_err() {
            return false;
        }
        let len = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
        if &c[..4] == b"fmt " {
            let mut fmt = [0u8; 16];
            if len < 16 || f.read_exact(&mut fmt).is_err() {
                return false;
            }
            let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
            let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
            return (tag == 1 || tag == 0xFFFE) && (1..=2).contains(&channels) && (8000..=96_000).contains(&rate) && (bits == 16 || bits == 24);
        }
        if f.seek(SeekFrom::Current(i64::from(len) + i64::from(len & 1))).is_err() {
            return false;
        }
    }
    false
}

/// The first audio sample entry of an MP4 (`mp4a` for AAC, `alac` for Apple Lossless, …).
fn mp4_codec(f: &mut File) -> Option<[u8; 4]> {
    let size = f.metadata().ok()?.len();
    let mut at = 0u64;
    // The top-level boxes, to find `moov` (it may come after `mdat`).
    while at + 8 <= size {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut len = u64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]));
        let mut head = 8;
        if len == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            len = u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]);
            head = 16;
        } else if len == 0 {
            len = size - at;
        }
        if len < head {
            return None;
        }
        if &h[4..8] == b"moov" {
            if len > 64 << 20 {
                return None;
            }
            let mut moov = vec![0u8; (len - head) as usize];
            f.read_exact(&mut moov).ok()?;
            return find_stsd(&moov, 0);
        }
        at += len;
    }
    None
}

/// Walks `moov/trak/mdia/minf/stbl/stsd` in memory; the first sample entry's fourcc.
fn find_stsd(data: &[u8], depth: u32) -> Option<[u8; 4]> {
    if depth > 6 {
        return None;
    }
    let mut at = 0usize;
    let mut found = None;
    while at + 8 <= data.len() {
        let len = u32::from_be_bytes(data[at..at + 4].try_into().ok()?) as usize;
        let kind = &data[at + 4..at + 8];
        if len < 8 || at + len > data.len() {
            return found;
        }
        let body = &data[at + 8..at + len];
        match kind {
            b"trak" | b"mdia" | b"minf" | b"stbl" => {
                if let Some(c) = find_stsd(body, depth + 1) {
                    // A video track's entry is not ours to judge; an audio one decides.
                    if is_audio(&c) {
                        return Some(c);
                    }
                    found.get_or_insert(c);
                }
            }
            b"stsd" if body.len() >= 16 => return body[12..16].try_into().ok(),
            _ => {}
        }
        at += len;
    }
    found
}

fn is_audio(fourcc: &[u8; 4]) -> bool {
    matches!(fourcc, b"mp4a" | b"alac" | b"ac-3" | b"ec-3" | b"Opus" | b"fLaC" | b".mp3")
}

/// A `Range` header's first range against a file of `size` bytes: the bytes to send (inclusive).
pub fn range(header: Option<&str>, size: u64) -> Option<(u64, u64)> {
    let spec = header?.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if size == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (size.saturating_sub(n), size - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
        (start, end)
    };
    (start <= end && start < size).then_some((start, end))
}

/// Answers one request for the file (200, or 206 for a range), streaming it from disk. `keep_on`
/// says whether to go on (the URL is still the current one and the server runs). Returns what
/// was sent, in bytes.
pub fn serve(conn: &mut TcpStream, path: &Path, content_type: &str, range_header: Option<&str>, head_only: bool, keep_on: impl Fn() -> bool) -> Result<u64, String> {
    let mut f = File::open(path).map_err(|e| format!("open: {e}"))?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    let part = range(range_header, size);
    if range_header.is_some() && part.is_none() && size > 0 && !range_header.is_some_and(|r| r.trim() == "bytes=0-") {
        let _ = write!(conn, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return Ok(0);
    }
    let (start, end) = part.unwrap_or((0, size.saturating_sub(1)));
    let len = if size == 0 { 0 } else { end - start + 1 };
    let status = if part.is_some() { format!("206 Partial Content\r\nContent-Range: bytes {start}-{end}/{size}") } else { "200 OK".into() };
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-cache, no-store\r\nConnection: close\r\n\r\n");
    conn.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    if head_only || len == 0 {
        return Ok(0);
    }
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut left = len;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 {
        if !keep_on() {
            return Err("a newer URL was loaded".into());
        }
        let want = left.min(buf.len() as u64) as usize;
        let n = f.read(&mut buf[..want]).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        conn.write_all(&buf[..n]).map_err(|_| "closed by the device".to_string())?;
        left -= n as u64;
    }
    Ok(len - left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("uzumaki-cast-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn mp4_with(codec: &[u8; 4]) -> Vec<u8> {
        fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        }
        let mut stsd_body = vec![0u8; 8]; // version/flags, entry count
        stsd_body.extend_from_slice(&36u32.to_be_bytes());
        stsd_body.extend_from_slice(codec);
        stsd_body.extend_from_slice(&[0u8; 28]);
        let stbl = bx(b"stbl", &bx(b"stsd", &stsd_body));
        let trak = bx(b"trak", &bx(b"mdia", &bx(b"minf", &stbl)));
        let mut file = bx(b"ftyp", b"M4A \0\0\0\0");
        file.extend(bx(b"mdat", &[1u8; 300])); // the audio first, `moov` at the end
        file.extend(bx(b"moov", &[bx(b"mvhd", &[0u8; 20]), trak].concat()));
        file
    }

    fn flac_with(rate: u32, bits: u8) -> Vec<u8> {
        let mut v = b"fLaC".to_vec();
        v.extend_from_slice(&[0x80, 0, 0, 34]);
        let mut si = vec![0u8; 34];
        si[10] = (rate >> 12) as u8;
        si[11] = (rate >> 4) as u8;
        si[12] = ((rate & 0xf) as u8) << 4 | (1 << 1) | ((bits - 1) >> 4);
        si[13] = ((bits - 1) & 0xf) << 4;
        v.extend(si);
        v
    }

    #[test]
    fn only_what_the_receiver_plays_is_sent_as_a_file() {
        assert_eq!(probe(&tmp("a.m4a", &mp4_with(b"mp4a"))), Some("audio/mp4"));
        assert_eq!(probe(&tmp("b.m4a", &mp4_with(b"alac"))), None, "Apple Lossless: the live stream");
        assert_eq!(probe(&tmp("c.flac", &flac_with(44_100, 16))), Some("audio/flac"));
        assert_eq!(probe(&tmp("d.flac", &flac_with(96_000, 24))), Some("audio/flac"));
        assert_eq!(probe(&tmp("e.flac", &flac_with(192_000, 24))), None, "above 96 kHz");
        let mut id3 = b"ID3\x04\0\0\0\0\0\x05".to_vec();
        id3.extend_from_slice(&[0u8; 5]);
        id3.extend(flac_with(48_000, 24));
        assert_eq!(probe(&tmp("f.flac", &id3)), Some("audio/flac"), "an ID3 tag in front");
        assert_eq!(probe(&tmp("g.mp3", b"ID3")), Some("audio/mpeg"));
        assert_eq!(probe(&tmp("h.ogg", b"OggS\0")), Some("audio/ogg"));
        assert_eq!(probe(&tmp("i.aiff", b"FORM")), None);
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        wav.extend_from_slice(b"LIST\x04\0\0\0abcd");
        wav.extend_from_slice(b"fmt \x10\0\0\0");
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&[0u8; 6]);
        wav.extend_from_slice(&16u16.to_le_bytes());
        assert_eq!(probe(&tmp("j.wav", &wav)), Some("audio/wav"));
        let float = wav.iter().enumerate().map(|(i, b)| if i == 28 { 3 } else { *b }).collect::<Vec<_>>();
        assert_eq!(probe(&tmp("k.wav", &float)), None, "float WAV");
        assert_eq!(probe(Path::new("/nonexistent/x.mp3")), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(range(Some("bytes=0-"), 100), Some((0, 99)));
        assert_eq!(range(Some("bytes=10-19"), 100), Some((10, 19)));
        assert_eq!(range(Some("bytes=90-200"), 100), Some((90, 99)));
        assert_eq!(range(Some("bytes=-10"), 100), Some((90, 99)));
        assert_eq!(range(Some("bytes=100-"), 100), None);
        assert_eq!(range(Some("items=0-"), 100), None);
        assert_eq!(range(None, 100), None);
    }
}
