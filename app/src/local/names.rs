//! Artist, title, album and track number guessed from a local file's name and folders, for files
//! whose tags are missing. Only the missing fields are filled (real tags always win), and every
//! field taken from here is marked in `Guessed`, so it can be shown as a guess and is never
//! written into a file (the app writes no tags).
//!
//! The file name, `_` read as a space and spaces squeezed, is split on ` - `, ` – ` and ` — `:
//!
//! - `Title`, `04 Title` (a zero-padded number), `4. Title`, `4) Title`
//! - `Artist - Title`, `NN - Title`, `NN - Artist - Title`, `Artist - NN - Title`
//! - `Artist - Album - NN Title`
//! - more parts: the first is the artist (after a number), the rest is the title.
//!
//! A plain `7 Rings` or `99 Problems` keeps its number: only a zero-padded number, a dot or a
//! bracket after it, or a separator makes it a track number. When the file name has no artist,
//! the folders `Artist/Album/` below the music folder give artist and album (a disc folder such
//! as `CD1` is skipped); one folder alone is left to the album stand-in (the folder's name).
//!
//! Brackets stay in the title for display (a remix is another song); `without_brackets` is for
//! LRCLIB's retry.

use std::path::{Component, Path};

use super::scan::is_disc_folder;

/// What the name and the folders say; None where they say nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Guess {
    /// Always something (the cleaned file name at worst).
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
}

/// The guess for the file at `path`, in the music folder `root` (None: the folders say nothing).
/// `title_tag` is the file's own title, if any: an artist is only read from the name when the
/// name's title is that title (otherwise `Song - Live.mp3` tagged "Song" would be by "Song").
pub fn guess(path: &Path, root: Option<&Path>, title_tag: Option<&str>) -> Guess {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut g = parse_name(&stem);
    if title_tag.is_some_and(|t| fold(t) != fold(&g.title)) {
        g.artist = None;
        g.album = None;
    }
    if let Some((artist, album)) = folders(path, root) {
        g.artist.get_or_insert(artist);
        g.album.get_or_insert(album);
    }
    g
}

/// The file name alone (no extension).
pub fn parse_name(stem: &str) -> Guess {
    let clean_stem = squeeze(stem);
    let mut parts = split(&clean_stem);
    let g = Guess { title: clean_stem, ..Guess::default() };
    if parts.is_empty() {
        return g;
    }
    let mut track = None;
    if parts.len() >= 2 {
        if let Some(n) = number_only(&parts[0]) {
            // `04 - Rest`
            track = Some(n);
            parts.remove(0);
        } else if let Some((n, rest)) = strip_track(&parts[0]) {
            // `01 Daft Punk - Around`: the number sticks to the artist.
            track = Some(n);
            parts[0] = rest.to_string();
        }
    }
    finish(g, track, &parts)
}

fn finish(mut g: Guess, mut track: Option<u32>, parts: &[String]) -> Guess {
    let last = parts.len() - 1;
    // The last part may carry the number: `Artist - Album - 04 Title`, `04 Title`.
    let mut last_text = parts[last].clone();
    if let Some((n, rest)) = strip_track(&parts[last]) {
        track.get_or_insert(n);
        last_text = rest.to_string();
    }
    match parts.len() {
        1 => g.title = last_text,
        2 => {
            g.artist = Some(parts[0].clone());
            g.title = last_text;
        }
        3 if number_only(&parts[1]).is_some() => {
            track = track.or(number_only(&parts[1]));
            g.artist = Some(parts[0].clone());
            g.title = last_text;
        }
        3 if strip_track(&parts[last]).is_some() => {
            g.artist = Some(parts[0].clone());
            g.album = Some(parts[1].clone());
            g.title = last_text;
        }
        _ => {
            g.artist = Some(parts[0].clone());
            let mut rest: Vec<&str> = parts[1..last].iter().map(String::as_str).collect();
            rest.push(&last_text);
            g.title = rest.join(" - ");
        }
    }
    g.track = track;
    if g.title.is_empty() {
        g.title = parts[last].clone();
    }
    g
}

/// `Artist/Album/` above the file (below `root`), without a trailing disc folder.
fn folders(path: &Path, root: Option<&Path>) -> Option<(String, String)> {
    let rel = path.parent()?.strip_prefix(root?).ok()?;
    let mut dirs: Vec<&Path> = Vec::new();
    for c in rel.components() {
        if let Component::Normal(n) = c {
            dirs.push(Path::new(n));
        }
    }
    if dirs.last().is_some_and(|d| is_disc_folder(d)) {
        dirs.pop();
    }
    let [.., artist, album] = dirs.as_slice() else { return None };
    let (artist, album) = (squeeze(&artist.to_string_lossy()), squeeze(&album.to_string_lossy()));
    (!artist.is_empty() && !album.is_empty()).then_some((artist, album))
}

/// `_` to spaces, runs of white space to one, trimmed.
fn squeeze(s: &str) -> String {
    s.replace('_', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fold(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Splits on ` - `, ` – ` and ` — `, dropping empty parts.
fn split(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '-' | '–' | '—') && i > 0 && chars[i - 1] == ' ' && chars.get(i + 1) == Some(&' ') {
            parts.push(cur.trim().to_string());
            cur.clear();
            i += 2;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    parts.push(cur.trim().to_string());
    parts.retain(|p| !p.is_empty());
    parts
}

/// "04", "4", "4." → 4 (1 to 3 digits, nothing else, above 0).
fn number_only(s: &str) -> Option<u32> {
    let d = s.trim().trim_end_matches('.');
    (!d.is_empty() && d.len() <= 3 && d.bytes().all(|b| b.is_ascii_digit())).then(|| d.parse().ok()).flatten().filter(|n| *n > 0)
}

/// A track number at the start: `04 Title` (zero-padded), `4. Title`, `4) Title`.
fn strip_track(s: &str) -> Option<(u32, &str)> {
    let s = s.trim();
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let (num, rest) = s.split_at(digits);
    let n: u32 = num.parse().ok().filter(|n| *n > 0)?;
    let title = if let Some(r) = rest.strip_prefix(['.', ')']) {
        // "1.5 Mix" is no track number.
        if r.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        r
    } else if rest.starts_with(' ') && num.starts_with('0') {
        rest
    } else {
        return None;
    };
    let title = title.trim();
    (!title.is_empty()).then_some((n, title))
}

/// The title without its trailing `(…)` and `[…]` groups ("Song (X Remix) [Video]" → "Song");
/// None when there is none or nothing would be left.
pub fn without_brackets(title: &str) -> Option<String> {
    let mut t = title.trim();
    loop {
        let (open, close) = match t.chars().next_back() {
            Some(')') => ('(', ')'),
            Some(']') => ('[', ']'),
            _ => break,
        };
        let mut depth = 0usize;
        let mut cut = None;
        for (i, c) in t.char_indices().rev() {
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    cut = Some(i);
                    break;
                }
            }
        }
        let Some(i) = cut else { break };
        t = t[..i].trim_end();
    }
    (!t.is_empty() && t != title.trim()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(stem: &str) -> Guess {
        parse_name(stem)
    }

    fn full(title: &str, artist: Option<&str>, album: Option<&str>, track: Option<u32>) -> Guess {
        Guess { title: title.into(), artist: artist.map(Into::into), album: album.map(Into::into), track }
    }

    #[test]
    fn a_remix_with_no_artist_is_a_title_only() {
        let got = guess(Path::new("/m/Music/Powerade (NOISETIME x Hähnchenteile Remix).wav"), Some(Path::new("/m/Music")), None);
        assert_eq!(got, full("Powerade (NOISETIME x Hähnchenteile Remix)", None, None, None));
    }

    #[test]
    fn artist_album_track_and_title_in_one_name() {
        let got = guess(Path::new("/m/Music/BATPIIICORE - BATPIIICORE VOL.2 - 04 KRILYA.flac"), Some(Path::new("/m/Music")), None);
        assert_eq!(got, full("KRILYA", Some("BATPIIICORE"), Some("BATPIIICORE VOL.2"), Some(4)));
    }

    #[test]
    fn the_name_forms() {
        assert_eq!(g("Daft Punk - Around the World"), full("Around the World", Some("Daft Punk"), None, None));
        assert_eq!(g("Daft Punk – Around the World"), full("Around the World", Some("Daft Punk"), None, None));
        assert_eq!(g("Daft Punk — Around the World"), full("Around the World", Some("Daft Punk"), None, None));
        assert_eq!(g("07 - Around the World"), full("Around the World", None, None, Some(7)));
        assert_eq!(g("07. Around the World"), full("Around the World", None, None, Some(7)));
        assert_eq!(g("7) Around the World"), full("Around the World", None, None, Some(7)));
        assert_eq!(g("07 Around the World"), full("Around the World", None, None, Some(7)));
        assert_eq!(g("07 - Daft Punk - Around the World"), full("Around the World", Some("Daft Punk"), None, Some(7)));
        assert_eq!(g("Daft Punk - 07 - Around the World"), full("Around the World", Some("Daft Punk"), None, Some(7)));
        assert_eq!(g("01 Daft Punk - Around the World"), full("Around the World", Some("Daft Punk"), None, Some(1)));
        assert_eq!(g("Daft Punk - Homework - 07 Around the World"), full("Around the World", Some("Daft Punk"), Some("Homework"), Some(7)));
        assert_eq!(g("Daft_Punk_-_Around_the_World"), full("Around the World", Some("Daft Punk"), None, None));
        assert_eq!(g("  Daft   Punk  -   Around  "), full("Around", Some("Daft Punk"), None, None));
    }

    #[test]
    fn numbers_that_belong_to_the_title_stay() {
        assert_eq!(g("7 Rings"), full("7 Rings", None, None, None));
        assert_eq!(g("99 Problems"), full("99 Problems", None, None, None));
        assert_eq!(g("1999"), full("1999", None, None, None));
        assert_eq!(g("Prince - 1999"), full("1999", Some("Prince"), None, None));
        assert_eq!(g("1.5 Mix"), full("1.5 Mix", None, None, None));
        assert_eq!(g("A-ha - Take On Me"), full("Take On Me", Some("A-ha"), None, None));
        assert_eq!(g("Jay-Z"), full("Jay-Z", None, None, None));
    }

    #[test]
    fn extra_parts_join_into_the_title_and_brackets_stay() {
        assert_eq!(g("Artist - Song - Live in Paris"), full("Song - Live in Paris", Some("Artist"), None, None));
        assert_eq!(g("Artist - Song (Radio Edit) [HQ]"), full("Song (Radio Edit) [HQ]", Some("Artist"), None, None));
        assert_eq!(g("-"), full("-", None, None, None));
        assert_eq!(g(""), full("", None, None, None));
    }

    #[test]
    fn folders_give_artist_and_album_when_the_name_has_none() {
        let root = Some(Path::new("/m/Music"));
        let got = guess(Path::new("/m/Music/Daft Punk/Homework/07 Around the World.mp3"), root, None);
        assert_eq!(got, full("Around the World", Some("Daft Punk"), Some("Homework"), Some(7)));
        // A disc folder is skipped.
        let got = guess(Path::new("/m/Music/Pink Floyd/The Wall/CD2/01 Hey You.flac"), root, None);
        assert_eq!(got, full("Hey You", Some("Pink Floyd"), Some("The Wall"), Some(1)));
        // One folder alone says nothing sure; the root itself is never a folder.
        assert_eq!(guess(Path::new("/m/Music/Mixes/Song.mp3"), root, None).artist, None);
        assert_eq!(guess(Path::new("/m/Music/The Wall/CD1/01 Hey You.flac"), root, None).artist, None);
        assert_eq!(guess(Path::new("/m/Music/Song.mp3"), root, None).artist, None);
        assert_eq!(guess(Path::new("/m/Music/Daft Punk/Homework/Song.mp3"), None, None).artist, None);
        // The name's artist wins over the folder's; the folder still gives the album.
        let got = guess(Path::new("/m/Music/Various/Hits/Prince - 1999.mp3"), root, None);
        assert_eq!(got, full("1999", Some("Prince"), Some("Hits"), None));
    }

    #[test]
    fn the_names_artist_is_only_trusted_when_the_title_agrees_with_the_tag() {
        let path = Path::new("/m/Music/Daft Punk - Around the World.mp3");
        assert_eq!(guess(path, None, Some("Around the World")).artist.as_deref(), Some("Daft Punk"));
        assert_eq!(guess(path, None, Some("around the world")).artist.as_deref(), Some("Daft Punk"));
        let live = Path::new("/m/Music/Song - Live.mp3");
        assert_eq!(guess(live, None, Some("Song")).artist, None);
    }

    #[test]
    fn brackets_come_off_for_the_retry_only() {
        assert_eq!(without_brackets("Powerade (NOISETIME x Hähnchenteile Remix)").as_deref(), Some("Powerade"));
        assert_eq!(without_brackets("Song [Official Video]").as_deref(), Some("Song"));
        assert_eq!(without_brackets("Song (feat. X) [HQ]").as_deref(), Some("Song"));
        assert_eq!(without_brackets("Song (Pt. 1) (Live)").as_deref(), Some("Song"));
        assert_eq!(without_brackets("Song (a (b))").as_deref(), Some("Song"));
        assert_eq!(without_brackets("(Intro)"), None, "nothing would be left");
        assert_eq!(without_brackets("Song"), None);
        assert_eq!(without_brackets("(Don't) Stop (Believing)").as_deref(), Some("(Don't) Stop"));
        assert_eq!(without_brackets("Song (unclosed"), None);
        assert_eq!(without_brackets("Song unopened)"), None);
    }
}
