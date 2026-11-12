//! Argument parsing helpers: seek and volume values, and finding a playlist or album by number
//! or name.

use uzumaki_ctl::{CollectionInfo, SeekTo, VolumeTo};

/// A plain non-negative decimal number; rejects signs, `inf`, exponents.
fn number(s: &str) -> Option<f64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    s.parse().ok()
}

/// `30`, `1:30`, `1:02:03` -> seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    parts.iter().try_fold(0.0, |acc, p| Some(acc * 60.0 + number(p)?))
}

pub fn parse_seek(s: &str) -> Result<SeekTo, String> {
    let bad = || format!("can't read '{s}' as a position (try 30, 1:30, +10, -10, +1:00, 50%)");
    if let Some(p) = s.strip_suffix('%') {
        return number(p).filter(|v| *v <= 100.0).map(SeekTo::Percent).ok_or_else(bad);
    }
    if let Some(rest) = s.strip_prefix('+') {
        return parse_time(rest).map(SeekTo::Relative).ok_or_else(bad);
    }
    if let Some(rest) = s.strip_prefix('-') {
        return parse_time(rest).map(|t| SeekTo::Relative(-t)).ok_or_else(bad);
    }
    parse_time(s).map(SeekTo::Absolute).ok_or_else(bad)
}

/// `60`, `+5`, `-5` (percent; the protocol is 0..1).
pub fn parse_volume(s: &str) -> Result<VolumeTo, String> {
    let bad = || format!("can't read '{s}' as a volume (try 60, +5, -5)");
    let body = s.strip_suffix('%').unwrap_or(s);
    if let Some(rest) = body.strip_prefix('+') {
        return number(rest).map(|v| VolumeTo::Change(v / 100.0)).ok_or_else(bad);
    }
    if let Some(rest) = body.strip_prefix('-') {
        return number(rest).map(|v| VolumeTo::Change(-v / 100.0)).ok_or_else(bad);
    }
    number(body).filter(|v| *v <= 100.0).map(|v| VolumeTo::Set(v / 100.0)).ok_or_else(bad)
}

/// A song number (1-based) -> its 0-based index.
pub fn parse_song_number(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n - 1),
        _ => Err(format!("'{s}' is not a song number (they start at 1)")),
    }
}

/// `my road trip 5` -> (`my road trip`, Some(index 4)). With one word the number is the list's.
pub fn split_song_number(words: &[String]) -> Result<(Vec<String>, Option<usize>), String> {
    if let Some((last, head)) = words.split_last() {
        if !head.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
            return Ok((head.to_vec(), Some(parse_song_number(last)?)));
        }
    }
    Ok((words.to_vec(), None))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Found {
    /// Index into the list.
    One(usize),
    None,
    /// Indexes of every match at the best level.
    Many(Vec<usize>),
}

/// A list number (1-based), else case-insensitive title: exact, then prefix, then substring.
pub fn resolve(items: &[CollectionInfo], query: &str) -> Found {
    let q = query.trim().to_lowercase();
    if let Ok(n) = q.parse::<usize>() {
        if (1..=items.len()).contains(&n) {
            return Found::One(n - 1);
        }
    }
    let titles: Vec<String> = items.iter().map(|c| c.title.to_lowercase()).collect();
    let levels: [&dyn Fn(&str) -> bool; 3] = [&|t| t == q, &|t| t.starts_with(&q), &|t| t.contains(&q)];
    for level in levels {
        let hits: Vec<usize> = (0..items.len()).filter(|&i| level(&titles[i])).collect();
        match hits.len() {
            0 => continue,
            1 => return Found::One(hits[0]),
            _ => {
                // Several match: a lone pinned one wins (the user put it in reach on purpose).
                let pinned: Vec<usize> = hits.iter().copied().filter(|&i| items[i].pinned.is_some()).collect();
                return if pinned.len() == 1 { Found::One(pinned[0]) } else { Found::Many(hits) };
            }
        }
    }
    Found::None
}

/// The first title (in list order) matching `query` case-insensitively: exact, then prefix,
/// then substring.
pub fn first_title<S: AsRef<str>>(titles: &[S], query: &str) -> Option<usize> {
    let q = query.trim().to_lowercase();
    let lower: Vec<String> = titles.iter().map(|t| t.as_ref().to_lowercase()).collect();
    let levels: [&dyn Fn(&str) -> bool; 3] = [&|t| t == q, &|t| t.starts_with(&q), &|t| t.contains(&q)];
    levels.iter().find_map(|level| lower.iter().position(|t| level(t)))
}

/// Indexes of items whose title or subtitle contains `filter` (all when empty).
pub fn filter(items: &[CollectionInfo], filter: &str) -> Vec<usize> {
    let f = filter.trim().to_lowercase();
    (0..items.len()).filter(|&i| items[i].title.to_lowercase().contains(&f) || items[i].subtitle.to_lowercase().contains(&f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uzumaki_ctl::Kind;

    fn items(titles: &[&str]) -> Vec<CollectionInfo> {
        titles
            .iter()
            .map(|t| CollectionInfo { kind: Kind::Playlist, id: t.to_string(), title: t.to_string(), subtitle: String::new(), count: None, favorites: false, editable: false, year: None, pinned: None })
            .collect()
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn seek_values() {
        assert_eq!(parse_seek("30"), Ok(SeekTo::Absolute(30.0)));
        assert_eq!(parse_seek("1:30"), Ok(SeekTo::Absolute(90.0)));
        assert_eq!(parse_seek("1:02:03"), Ok(SeekTo::Absolute(3723.0)));
        assert_eq!(parse_seek("+10"), Ok(SeekTo::Relative(10.0)));
        assert_eq!(parse_seek("-10"), Ok(SeekTo::Relative(-10.0)));
        assert_eq!(parse_seek("+1:00"), Ok(SeekTo::Relative(60.0)));
        assert_eq!(parse_seek("-1:00"), Ok(SeekTo::Relative(-60.0)));
        assert_eq!(parse_seek("50%"), Ok(SeekTo::Percent(50.0)));
        for bad in ["", "abc", "101%", "1:2:3:4", "+", "1e3", "inf", "--5", "%"] {
            assert!(parse_seek(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn volume_values() {
        assert_eq!(parse_volume("60"), Ok(VolumeTo::Set(0.6)));
        assert_eq!(parse_volume("60%"), Ok(VolumeTo::Set(0.6)));
        assert_eq!(parse_volume("0"), Ok(VolumeTo::Set(0.0)));
        assert_eq!(parse_volume("+5"), Ok(VolumeTo::Change(0.05)));
        assert_eq!(parse_volume("-5"), Ok(VolumeTo::Change(-0.05)));
        for bad in ["", "loud", "150", "+", "nan"] {
            assert!(parse_volume(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolves_by_number_and_name() {
        let l = items(&["Anima ;)", "Road Trip", "Road Trip 2", "Roadhouse", "Chill"]);
        assert_eq!(resolve(&l, "2"), Found::One(1));
        assert_eq!(resolve(&l, "road trip"), Found::One(1)); // exact beats prefix
        assert_eq!(resolve(&l, "ROAD TRIP 2"), Found::One(2));
        assert_eq!(resolve(&l, "roadh"), Found::One(3));
        assert_eq!(resolve(&l, "hil"), Found::One(4)); // substring
        assert_eq!(resolve(&l, "road"), Found::Many(vec![1, 2, 3]));
        assert_eq!(resolve(&l, "nothing"), Found::None);
        assert_eq!(resolve(&l, "99"), Found::None);
    }

    #[test]
    fn a_lone_pinned_match_wins_when_ambiguous() {
        let mut l = items(&["Road Trip", "Road Trip 2", "Roadhouse"]);
        l[1].pinned = Some(0);
        assert_eq!(resolve(&l, "road"), Found::One(1));
        assert_eq!(resolve(&l, "road trip"), Found::One(0), "an exact title still beats a pinned prefix");
        l[2].pinned = Some(1);
        assert_eq!(resolve(&l, "road"), Found::Many(vec![0, 1, 2]), "two pinned matches stay ambiguous");
    }

    #[test]
    fn a_numeric_title_wins_when_out_of_range() {
        assert_eq!(resolve(&items(&["1989", "x"]), "1989"), Found::One(0));
    }

    #[test]
    fn trailing_song_number() {
        assert_eq!(split_song_number(&words("my road trip 5")), Ok((words("my road trip"), Some(4))));
        assert_eq!(split_song_number(&words("5")), Ok((words("5"), None)));
        assert_eq!(split_song_number(&words("3 5")), Ok((words("3"), Some(4))));
        assert_eq!(split_song_number(&words("road trip")), Ok((words("road trip"), None)));
        assert!(split_song_number(&words("road 0")).is_err());
    }

    #[test]
    fn first_title_prefers_exact_then_prefix_then_substring() {
        let t = ["Midnight City", "City Lights", "City", "Big City Life"];
        assert_eq!(first_title(&t, "city"), Some(2));
        assert_eq!(first_title(&t, "CITY L"), Some(1));
        assert_eq!(first_title(&t, "ity"), Some(0));
        assert_eq!(first_title(&t, "big"), Some(3));
        assert_eq!(first_title(&t, "nope"), None);
    }

    #[test]
    fn filter_keeps_positions() {
        let l = items(&["Alpha", "Beta", "Alphabet"]);
        assert_eq!(filter(&l, "alpha"), vec![0, 2]);
        assert_eq!(filter(&l, ""), vec![0, 1, 2]);
    }
}
