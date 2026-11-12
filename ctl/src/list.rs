//! Numbered lists: `  1  Title        Artist     3:45`, fitted to the terminal width.

use uzumaki_ctl::{CollectionInfo, Kind, SongInfo};

use crate::fmt::{accent, bold, dim, fmt_time, pad_left, pad_right, plain, plural, render, text_width, truncate, Span};

pub struct Row {
    pub num: usize,
    /// The song that plays now: gets a coral `▶`.
    pub playing: bool,
    pub title: String,
    pub artist: String,
    /// Right-aligned last column (duration, song count).
    pub extra: String,
    pub dim: bool,
}

const GAP: usize = 2;

fn col_max(rows: &[Row], f: impl Fn(&Row) -> &str) -> usize {
    rows.iter().map(|r| text_width(f(r))).max().unwrap_or(0)
}

/// Widths for the title and artist columns: natural if they fit, else the artist gets at most
/// two fifths of what is left.
fn text_columns(title_max: usize, artist_max: usize, room: usize) -> (usize, usize) {
    if title_max + artist_max <= room {
        return (title_max, artist_max);
    }
    let artist = artist_max.min(room * 2 / 5);
    let title = title_max.min(room - artist);
    (title, artist_max.min(room - title))
}

pub fn rows(rows: &[Row], width: usize, color: bool) -> Vec<String> {
    let digits = rows.iter().map(|r| r.num.to_string().len()).max().unwrap_or(1);
    let (title_max, artist_max, extra_max) = (col_max(rows, |r| &r.title), col_max(rows, |r| &r.artist), col_max(rows, |r| &r.extra));
    let fixed = 2 + digits + if extra_max > 0 { GAP + extra_max } else { 0 } + if artist_max > 0 { GAP } else { 0 } + GAP;
    let (title_w, artist_w) = text_columns(title_max, artist_max, width.saturating_sub(fixed));
    rows.iter().map(|r| render(&row_spans(r, digits, title_w, artist_w, extra_max), width, color)).collect()
}

fn row_spans(r: &Row, digits: usize, title_w: usize, artist_w: usize, extra_w: usize) -> Vec<Span> {
    let gap = || plain(" ".repeat(GAP));
    let mut spans = vec![if r.playing { accent("▶") } else { plain(" ") }, plain(" "), dim(pad_left(&r.num.to_string(), digits)), gap()];
    let title = pad_right(&truncate(&r.title, title_w), title_w);
    spans.push(if r.dim { dim(title) } else if r.playing { bold(title) } else { plain(title) });
    if artist_w > 0 {
        spans.push(gap());
        spans.push(dim(pad_right(&truncate(&r.artist, artist_w), artist_w)));
    }
    if extra_w > 0 {
        spans.push(gap());
        spans.push(dim(pad_left(&r.extra, extra_w)));
    }
    trim_trailing_space(&mut spans);
    spans
}

/// Rows without an artist or count would otherwise end in padding.
fn trim_trailing_space(spans: &mut Vec<Span>) {
    while let Some(last) = spans.last_mut() {
        last.text.truncate(last.text.trim_end().len());
        if !last.text.is_empty() {
            break;
        }
        spans.pop();
    }
}

/// Songs numbered from 1; `playing_id` marks the song that plays now; unplayable ones are dimmed.
pub fn song_rows(songs: &[SongInfo], playing_id: Option<&str>) -> Vec<Row> {
    songs
        .iter()
        .enumerate()
        .map(|(i, s)| Row {
            num: i + 1,
            playing: playing_id == Some(s.id.as_str()),
            title: s.title.clone(),
            artist: s.artist.clone(),
            extra: if s.duration > 0.0 { fmt_time(s.duration) } else { String::new() },
            dim: !s.playable,
        })
        .collect()
}

/// Collections with their numbers in the full list (so a filtered list keeps its numbers).
pub fn collection_rows(items: &[(usize, &CollectionInfo)]) -> Vec<Row> {
    items
        .iter()
        .map(|(n, c)| Row {
            num: *n,
            playing: false,
            title: c.title.clone(),
            // Pinned playlists say so in the (otherwise empty) second column: plain text, so it
            // survives a pipe.
            artist: match (c.kind, c.pinned) {
                (Kind::Album, _) => c.subtitle.clone(),
                (_, Some(_)) => "pinned".into(),
                _ => String::new(),
            },
            extra: c.count.map(|k| plural(k as usize, "song")).unwrap_or_default(),
            dim: false,
        })
        .collect()
}

/// `Anima ;) — 48 songs, 2:41:00` (albums add the artist).
pub fn tracks_header(c: &CollectionInfo, songs: &[SongInfo], width: usize, color: bool) -> String {
    let total: f64 = songs.iter().map(|s| s.duration).sum();
    let mut spans = vec![bold(c.title.clone())];
    if c.kind == Kind::Album && !c.subtitle.is_empty() {
        spans.push(dim(" — "));
        spans.push(plain(c.subtitle.clone()));
    }
    let mut tail = format!(" — {}", plural(songs.len(), "song"));
    if total > 0.0 {
        tail.push_str(&format!(", {}", fmt_time(total)));
    }
    spans.push(dim(tail));
    render(&spans, width, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str, artist: &str, dur: f64, playable: bool) -> SongInfo {
        SongInfo { id: id.into(), title: title.into(), artist: artist.into(), duration: dur, playable, ..Default::default() }
    }

    #[test]
    fn rows_align_and_mark_the_playing_song() {
        let songs = vec![song("a", "Short", "Me", 225.0, true), song("b", "A longer title", "Someone Else", 65.0, true)];
        let lines = rows(&song_rows(&songs, Some("b")), 80, false);
        assert_eq!(lines[0], "  1  Short           Me            3:45");
        assert_eq!(lines[1], "▶ 2  A longer title  Someone Else  1:05");
    }

    #[test]
    fn rows_are_truncated_never_wrapped() {
        let songs = vec![song("a", "An extremely long song title indeed", "An Equally Long Artist Name", 225.0, true)];
        for w in [20, 30, 40, 60] {
            let lines = rows(&song_rows(&songs, None), w, false);
            assert!(text_width(&lines[0]) <= w, "{w}: {}", lines[0]);
            assert!(lines[0].contains('…'));
            assert!(lines[0].ends_with("3:45"), "{}", lines[0]);
        }
    }

    #[test]
    fn numbers_right_align_and_unplayable_dims() {
        let mut r = song_rows(&[song("a", "T", "A", 60.0, false)], None);
        r[0].num = 532;
        let lines = rows(&r, 40, true);
        assert!(lines[0].contains("\x1b[2m"));
        assert!(lines[0].contains("532"));
    }

    #[test]
    fn header() {
        let c = CollectionInfo { kind: Kind::Playlist, id: "p".into(), title: "Anima ;)".into(), subtitle: String::new(), count: None, favorites: false, editable: true, year: None, pinned: None };
        let songs: Vec<_> = (0..48).map(|i| song(&i.to_string(), "t", "a", 201.25, true)).collect();
        assert_eq!(tracks_header(&c, &songs, 80, false), "Anima ;) — 48 songs, 2:41:00");
    }
}
