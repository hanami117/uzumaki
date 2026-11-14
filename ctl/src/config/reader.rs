//! Reads the text of `uzu.toml` with the `toml` crate and flattens it into what the rest of
//! `config` works with: the `[section]` headers and, per section, `key = value` entries, each with
//! its line.
//!
//! The crate parses (so dotted keys, `[[tables]]`, floats and inline tables all read fine) and
//! tells us where every key is. What it can't read costs only itself: `parse_recoverable` skips
//! the broken part and keeps the rest, and each skipped part becomes a [`Problem`] with its line.
//! Whether a value is right for its key is not decided here; the sections do that, key by key.

use toml::Value;
use toml::de::DeTable;
use toml::de::DeValue;

/// One `key = value` with the section it sits in (`""` before the first header).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub section: String,
    pub key: String,
    pub value: Value,
    /// 1-based line of the key.
    pub line: usize,
}

/// Something in the text the reader could not use.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// 1-based; 0 when the crate gave no position.
    pub line: usize,
    pub message: String,
}

/// The sections seen (with their lines), the entries in file order, and the problems.
#[derive(Default, Debug)]
pub struct Document {
    pub entries: Vec<Entry>,
    pub sections: Vec<(String, usize)>,
    pub problems: Vec<Problem>,
}

/// What a value is, for messages: "text", "a whole number", …
pub fn kind(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "a string",
        Value::Integer(_) => "a whole number",
        Value::Float(_) => "a number with decimals",
        Value::Boolean(_) => "true or false",
        Value::Datetime(_) => "a date",
        Value::Array(_) => "a list",
        Value::Table(_) => "a table",
    }
}

/// A header that could not be read is replaced by this one, so the lines under it are dropped
/// with it instead of landing in the section above.
const SKIPPED: &str = "\u{1}skipped"; // as the header `["\u0001skipped"]` spells it

/// The most lines given up on in one file (each costs one more read of a small text).
const MAX_SKIPPED: usize = 100;

pub fn parse(text: &str) -> Document {
    let mut text = text.to_string();
    let mut doc = Document::default();
    let (table, errors) = loop {
        let (table, errors) = DeTable::parse_recoverable(&text);
        // What to give up on: the line of the first error, or the line a list starts on when it
        // swallowed the rest of the file (the crate can't recover from that one).
        let culprit = runaway(&text, &table)
            .map(|line| (line, "unclosed list, expected `]`".to_string()))
            .or_else(|| errors.iter().filter_map(|e| Some((line_in(&text, e.span()?.start), e.message().to_string()))).min_by_key(|(line, _)| *line));
        let Some((line, message)) = culprit else { break (table, errors) };
        let blanked = blank_line(&text, line);
        if doc.problems.len() >= MAX_SKIPPED || blanked == text {
            break (table, errors);
        }
        let message = if message == "duplicate key" { "duplicate key; the first one counts".to_string() } else { message };
        doc.problems.push(Problem { line, message });
        text = blanked;
    };
    let line_of = |offset: usize| line_in(&text, offset);
    // Only when the loop gave up: what is left over is still reported.
    for e in &errors {
        doc.problems.push(Problem { line: e.span().map_or(0, |s| line_of(s.start)), message: e.message().to_string() });
    }
    let bad: Vec<usize> = doc.problems.iter().map(|p| p.line).collect();
    // The map is sorted by key; the file order matters (the first of two actions on one key wins).
    let mut top: Vec<_> = table.get_ref().iter().collect();
    top.sort_by_key(|(k, _)| k.span().start);
    for (key, value) in top {
        let (name, line) = (key.get_ref().to_string(), line_of(key.span().start));
        match value.get_ref() {
            DeValue::Table(_) if name == SKIPPED => {}
            DeValue::Table(section) => {
                if !bad.contains(&line) {
                    doc.sections.push((name.clone(), line));
                }
                let mut entries: Vec<_> = section.iter().collect();
                entries.sort_by_key(|(k, _)| k.span().start);
                for (k, v) in entries {
                    let line = line_of(k.span().start);
                    if bad.contains(&line) {
                        continue; // the line has a problem of its own; what the crate made of it is a guess
                    }
                    match owned(v.get_ref()) {
                        Ok(value) => doc.entries.push(Entry { section: name.clone(), key: k.get_ref().to_string(), value, line }),
                        Err(message) => doc.problems.push(Problem { line, message }),
                    }
                }
            }
            // `[[name]]`: not a shape any section has.
            DeValue::Array(items) if !items.is_empty() && items.iter().all(|i| i.get_ref().is_table()) => {
                doc.sections.push((name.clone(), line));
                doc.problems.push(Problem { line, message: format!("'[[{name}]]' is a list of tables, and no section is one; use [{name}]") });
            }
            _ if bad.contains(&line) => {}
            other => match owned(other) {
                Ok(value) => doc.entries.push(Entry { section: String::new(), key: name, value, line }),
                Err(message) => doc.problems.push(Problem { line, message }),
            },
        }
    }
    doc.entries.sort_by_key(|e| e.line);
    doc.problems.sort_by_key(|p| p.line);
    doc
}

/// The crate's borrowed value as an owned [`Value`].
fn owned(value: &DeValue<'_>) -> Result<Value, String> {
    Ok(match value {
        DeValue::String(s) => Value::String(s.to_string()),
        DeValue::Integer(i) => Value::Integer(i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| format!("{} is too big a number", i))?),
        DeValue::Float(f) => Value::Float(f.as_str().parse().map_err(|_| format!("'{}' is not a number", f.as_str()))?),
        DeValue::Boolean(b) => Value::Boolean(*b),
        DeValue::Datetime(d) => Value::Datetime(*d),
        DeValue::Array(items) => Value::Array(items.iter().map(|i| owned(i.get_ref())).collect::<Result<_, _>>()?),
        DeValue::Table(t) => Value::Table(t.iter().map(|(k, v)| Ok((k.get_ref().to_string(), owned(v.get_ref())?))).collect::<Result<_, String>>()?),
    })
}

/// 1-based line of a byte offset.
fn line_in(text: &str, offset: usize) -> usize {
    1 + text.as_bytes()[..offset.min(text.len())].iter().filter(|b| **b == b'\n').count()
}

/// The text with line `line` (1-based) emptied, keeping the line count; a header becomes the
/// [`SKIPPED`] section.
fn blank_line(text: &str, line: usize) -> String {
    let lines: Vec<String> = text
        .split('\n')
        .enumerate()
        .map(|(i, l)| match (i + 1 == line, l.trim_start().starts_with('[')) {
            (false, _) => l.to_string(),
            (true, true) => "[\"\\u0001skipped\"]".to_string(),
            (true, false) => String::new(),
        })
        .collect();
    lines.join("\n")
}

/// The line of a key whose list or inline table runs to the end of the file unclosed, if any.
fn runaway(text: &str, table: &toml::Spanned<DeTable<'_>>) -> Option<usize> {
    let end = text.trim_end().len();
    let open = |v: &toml::Spanned<DeValue<'_>>| matches!(v.get_ref(), DeValue::Array(_)) && v.span().end >= end && !text[..end].ends_with(']');
    let line_of = |offset: usize| line_in(text, offset);
    for (k, v) in table.get_ref() {
        if open(v) {
            return Some(line_of(k.span().start));
        }
        if let DeValue::Table(section) = v.get_ref() {
            for (k, v) in section {
                if open(v) {
                    return Some(line_of(k.span().start));
                }
            }
        }
    }
    None
}
