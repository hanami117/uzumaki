//! Synced lyrics from Apple's TTML (`/v1/catalog/{sf}/songs/{id}/lyrics` → `attributes.ttml`).

/// One word (or syllable group) with its own timing, from Apple's syllable lyrics.
/// `text` keeps the space that follows the word, so the words concatenate to `Line.text`
/// (up to whitespace normalisation).
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// One line of lyrics.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Line {
    /// Seconds from the start of the song (0 when not synced).
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Sung by the second voice (`ttm:agent="v2"`): Apple aligns these to the right.
    pub second_voice: bool,
    /// Per-word timing; empty for line-timed lyrics.
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Lyrics {
    /// False when Apple has only plain text (`itunes:timing="None"`).
    pub synced: bool,
    pub lines: Vec<Line>,
}

impl Lyrics {
    /// Index of the line being sung at `t` seconds (the last line that started), if any.
    pub fn line_at(&self, t: f64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        self.lines.iter().rposition(|l| l.start <= t)
    }
}

/// Parses Apple's lyrics TTML. None when there are no lines.
pub fn parse_ttml(ttml: &str) -> Option<Lyrics> {
    let doc = roxmltree::Document::parse(ttml).ok()?;
    let attr = |n: roxmltree::Node<'_, '_>, name: &str| -> Option<String> {
        n.attributes().find(|a| a.name() == name).map(|a| a.value().to_string())
    };
    let root = doc.root_element();
    let timing = attr(root, "timing");
    let first_agent = root
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "agent")
        .and_then(|n| attr(n, "id"));

    let synced_hint = !timing.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("none"));
    let mut lines = Vec::new();
    let mut any_timed = false;
    for p in root.descendants().filter(|n| n.is_element() && n.tag_name().name() == "p") {
        let mut text = String::new();
        collect_text(p, &mut text);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        let spans: Vec<_> = p.descendants().filter(|n| n.is_element() && n.tag_name().name() == "span").collect();
        let time = |n: roxmltree::Node<'_, '_>, k: &str| attr(n, k).and_then(|v| parse_time(&v));
        let start = time(p, "begin").or_else(|| spans.iter().find_map(|s| time(*s, "begin")));
        let end = time(p, "end").or_else(|| spans.iter().rev().find_map(|s| time(*s, "end")));
        any_timed |= start.is_some();
        let start = start.unwrap_or(0.0);
        let end = end.unwrap_or(start).max(start);
        let agent = attr(p, "agent")
            .or_else(|| p.parent().filter(|d| d.is_element()).and_then(|d| attr(d, "agent")));
        let second_voice = match (&agent, &first_agent) {
            (Some(a), Some(f)) => a != f,
            (Some(a), None) => a != "v1",
            _ => false,
        };
        let words = if synced_hint { parse_words(p) } else { Vec::new() };
        lines.push(Line { start, end, text, second_voice, words });
    }
    if lines.is_empty() {
        return None;
    }
    let synced = match timing.as_deref() {
        Some(t) if t.eq_ignore_ascii_case("none") => false,
        Some(_) => true,
        None => any_timed,
    };
    if !synced {
        for l in &mut lines {
            l.start = 0.0;
            l.end = 0.0;
        }
    }
    Some(Lyrics { synced, lines })
}

/// Timed `<span begin end>` words of a line, each keeping the whitespace after it. Empty unless
/// at least one span is timed. Nested spans (translation/transliteration) are skipped.
fn parse_words(p: roxmltree::Node<'_, '_>) -> Vec<Word> {
    let attr_t = |n: roxmltree::Node<'_, '_>, k: &str| {
        n.attributes().find(|a| a.name() == k).and_then(|a| parse_time(a.value()))
    };
    let mut words: Vec<Word> = Vec::new();
    let mut any = false;
    for c in p.children() {
        if c.is_text() {
            // Whitespace between spans belongs to the previous word.
            let t = c.text().unwrap_or("");
            if t.chars().next().is_some_and(char::is_whitespace) {
                if let Some(w) = words.last_mut() {
                    if !w.text.ends_with(' ') {
                        w.text.push(' ');
                    }
                }
            }
        } else if c.is_element() && c.tag_name().name() == "span" {
            let mut text = String::new();
            collect_text(c, &mut text);
            if text.trim().is_empty() {
                continue;
            }
            let lead = text.chars().next().is_some_and(char::is_whitespace);
            let trail = text.chars().last().is_some_and(char::is_whitespace);
            if lead {
                if let Some(w) = words.last_mut() {
                    if !w.text.ends_with(' ') {
                        w.text.push(' ');
                    }
                }
            }
            let (b, e) = (attr_t(c, "begin"), attr_t(c, "end"));
            any |= b.is_some();
            let start = b.unwrap_or_else(|| words.last().map_or(0.0, |w| w.end));
            let end = e.unwrap_or(start).max(start);
            let mut t = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if trail {
                t.push(' ');
            }
            words.push(Word { start, end, text: t });
        }
    }
    if let Some(w) = words.last_mut() {
        w.text = w.text.trim_end().to_string();
    }
    if any { words } else { Vec::new() }
}

/// Parses a `.lrc` file (the lyrics a local song keeps next to it): `[mm:ss.xx]text` lines, with
/// several stamps on one line allowed, `[offset:ms]` honoured, other `[tag:value]` lines and
/// enhanced word stamps (`<mm:ss.xx>`) skipped. Without any time stamp the text is plain
/// lyrics. None when there is no text. Each line ends where the next starts (the last one
/// lasts 5 s).
pub fn parse_lrc(text: &str) -> Option<Lyrics> {
    let text = text.trim_start_matches('\u{feff}');
    let mut offset = 0.0;
    let mut timed: Vec<(f64, String)> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(inner) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            let (tag, after) = inner;
            if let Some(t) = parse_time(tag).filter(|_| tag.contains(':') && tag.chars().next().is_some_and(|c| c.is_ascii_digit())) {
                stamps.push(t);
            } else if let Some(ms) = tag.strip_prefix("offset:") {
                offset = ms.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
                rest = after;
                continue;
            } else if stamps.is_empty() && tag.split_once(':').is_some_and(|(k, _)| k.chars().all(|c| c.is_ascii_alphabetic())) {
                // `[ar:Artist]`, `[ti:Title]`: not lyrics.
                rest = "";
                break;
            } else {
                break;
            }
            rest = after;
        }
        let mut line = String::new();
        let mut in_stamp = false;
        for c in rest.chars() {
            match c {
                '<' => in_stamp = true,
                '>' if in_stamp => in_stamp = false,
                _ if !in_stamp => line.push(c),
                _ => {}
            }
        }
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if stamps.is_empty() {
            if !line.is_empty() && !raw.trim().is_empty() && !raw.trim().starts_with('[') {
                plain.push(line);
            }
        } else {
            // A stamp with no text is a pause (it ends the line before it); keep it out of the list.
            for t in stamps {
                timed.push(((t + offset).max(0.0), line.clone()));
            }
        }
    }
    if timed.is_empty() {
        let lines: Vec<Line> = plain.into_iter().map(|text| Line { text, ..Line::default() }).collect();
        return (!lines.is_empty()).then_some(Lyrics { synced: false, lines });
    }
    timed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut lines: Vec<Line> = Vec::new();
    for (i, (start, text)) in timed.iter().enumerate() {
        let end = timed.get(i + 1).map_or(start + 5.0, |n| n.0);
        if text.is_empty() {
            continue;
        }
        lines.push(Line { start: *start, end, text: text.clone(), ..Line::default() });
    }
    (!lines.is_empty()).then_some(Lyrics { synced: true, lines })
}

/// The `.lrc` next to a local song's file, when there is one (up to 1 MB, UTF-8).
pub fn read_lrc_beside(song_file: &std::path::Path) -> Option<Lyrics> {
    let path = song_file.with_extension("lrc");
    if std::fs::metadata(&path).ok()?.len() > 1 << 20 {
        return None;
    }
    parse_lrc(&std::fs::read_to_string(path).ok()?)
}

/// Text of `n` with `<br/>` as a space (XML entities are already decoded by the parser).
fn collect_text(n: roxmltree::Node<'_, '_>, out: &mut String) {
    for c in n.children() {
        if c.is_text() {
            out.push_str(c.text().unwrap_or(""));
        } else if c.is_element() {
            if c.tag_name().name() == "br" {
                out.push(' ');
            } else {
                collect_text(c, out);
            }
        }
    }
}

/// "21.769", "3:07.508", "1:02:03.5", "12.5s" -> seconds.
fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix('s').unwrap_or(s);
    let mut total = 0.0;
    for part in s.split(':') {
        let v: f64 = part.parse().ok()?;
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" itunes:timing="#;

    fn doc(timing: &str, body: &str) -> String {
        format!(r#"{HEAD}"{timing}"><head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/></metadata></head><body>{body}</body></tt>"#)
    }

    #[test]
    fn line_timing() {
        let l = parse_ttml(&doc("Line", r#"<div begin="1.5" end="9"><p begin="1.5" end="4.25" ttm:agent="v1">alpha beta</p><p begin="0:05.000" end="9" ttm:agent="v1">gamma</p></div>"#)).unwrap();
        assert!(l.synced);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0], Line { start: 1.5, end: 4.25, text: "alpha beta".into(), second_voice: false, words: vec![] });
        assert_eq!(l.lines[1].start, 5.0);
    }

    #[test]
    fn word_timing_concatenates_spans() {
        let l = parse_ttml(&doc("Word", r#"<div><p begin="2" end="4"><span begin="2" end="3">la</span> <span begin="3" end="4">di</span><span begin="3" end="4">da</span></p></div>"#)).unwrap();
        assert_eq!(l.lines[0].text, "la dida");
        assert_eq!((l.lines[0].start, l.lines[0].end), (2.0, 4.0));
        let w = &l.lines[0].words;
        assert_eq!(w.len(), 3);
        assert_eq!((w[0].text.as_str(), w[0].start, w[0].end), ("la ", 2.0, 3.0));
        assert_eq!(w[1].text, "di");
        assert_eq!(w[2].text, "da");
        let joined: String = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(joined, l.lines[0].text);
    }

    #[test]
    fn line_timing_has_no_words() {
        let l = parse_ttml(&doc("Line", r#"<div><p begin="1" end="2">plain line</p></div>"#)).unwrap();
        assert!(l.lines[0].words.is_empty());
    }

    #[test]
    fn none_timing_is_plain() {
        let l = parse_ttml(&doc("None", "<div><p>one</p><p>two</p></div>")).unwrap();
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.line_at(100.0), None);
    }

    #[test]
    fn time_formats() {
        assert_eq!(parse_time("21.769"), Some(21.769));
        assert_eq!(parse_time("3:07.508"), Some(187.508));
        assert_eq!(parse_time("1:02:03.5"), Some(3723.5));
        assert_eq!(parse_time("12.5s"), Some(12.5));
        assert_eq!(parse_time("abc"), None);
    }

    #[test]
    fn entities_and_br() {
        let l = parse_ttml(&doc("Line", r#"<div><p begin="1" end="2">rock &amp; roll &#38; &#x41;&lt;&gt;&quot;&apos;<br/>next</p></div>"#)).unwrap();
        assert_eq!(l.lines[0].text, "rock & roll & A<>\"' next");
    }

    #[test]
    fn second_voice_from_p_or_div() {
        let l = parse_ttml(&doc("Line", r#"<div ttm:agent="v2"><p begin="1" end="2">a</p></div><div><p begin="3" end="4" ttm:agent="v2">b</p><p begin="5" end="6" ttm:agent="v1">c</p><p begin="7" end="8">d</p></div>"#)).unwrap();
        let v: Vec<bool> = l.lines.iter().map(|l| l.second_voice).collect();
        assert_eq!(v, [true, true, false, false]);
    }

    #[test]
    fn empty_and_garbage() {
        assert_eq!(parse_ttml(""), None);
        assert_eq!(parse_ttml("not xml <<"), None);
        assert_eq!(parse_ttml(&doc("Line", r#"<div><p begin="1" end="2">  </p></div>"#)), None);
    }

    #[test]
    fn line_at_picks_earlier_line() {
        let l = parse_ttml(&doc("Line", r#"<div><p begin="10" end="12">a</p><p begin="20" end="22">b</p></div>"#)).unwrap();
        assert_eq!(l.line_at(5.0), None);
        assert_eq!(l.line_at(10.0), Some(0));
        assert_eq!(l.line_at(15.0), Some(0));
        assert_eq!(l.line_at(25.0), Some(1));
    }
    #[test]
    fn lrc_synced_with_stamps_tags_and_offset() {
        let l = parse_lrc("\u{feff}[ar:Someone]\n[ti:Song]\n[offset:500]\n[00:12.00]first line\n[00:05.50][01:00.25]  chorus <00:05.60>word  here\n[00:30.00]\n\n").unwrap();
        assert!(l.synced);
        let got: Vec<(f64, f64, &str)> = l.lines.iter().map(|x| (x.start, x.end, x.text.as_str())).collect();
        assert_eq!(got, [(6.0, 12.5, "chorus word here"), (12.5, 30.5, "first line"), (60.75, 65.75, "chorus word here")]);
        assert_eq!(l.line_at(13.0), Some(1));
    }

    #[test]
    fn lrc_without_stamps_is_plain_and_junk_is_none() {
        let l = parse_lrc("[ar:X]\nline one\n\nline two\n").unwrap();
        assert!(!l.synced);
        assert_eq!(l.lines.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["line one", "line two"]);
        assert_eq!(parse_lrc(""), None);
        assert_eq!(parse_lrc("[ti:Only a tag]\n[00:01.00]\n"), None);
    }
}
