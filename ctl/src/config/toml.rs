//! A small reader for the part of TOML that `uzu.toml` uses: `[section]` headers, `key = value`
//! lines, and values that are strings, whole numbers, `true`/`false` or arrays of those.
//! `#` starts a comment. Arrays may run over several lines.
//!
//! It is written by hand (not a TOML crate) for one reason: a mistake on one line must cost
//! only that line. A strict parser gives up at the first error and reports no line numbers
//! worth having; this one records a problem and goes on with the next line.

/// One value from the file.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<Value>),
}

impl Value {
    /// What the value is, for messages: "a string", "a number", …
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Str(_) => "a string",
            Value::Int(_) => "a whole number",
            Value::Bool(_) => "true or false",
            Value::List(_) => "a list",
        }
    }
}

/// One `key = value` line with the section it sits in (`""` before the first header).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub section: String,
    pub key: String,
    pub value: Value,
    /// 1-based line of the key.
    pub line: usize,
}

/// A line that could not be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub line: usize,
    pub message: String,
}

/// The section headers seen (with their lines), the entries and the problems.
#[derive(Default, Debug)]
pub struct Document {
    pub entries: Vec<Entry>,
    pub sections: Vec<(String, usize)>,
    pub problems: Vec<Problem>,
}

pub fn parse(text: &str) -> Document {
    let mut p = Parser { s: text.chars().collect(), i: 0, line: 1 };
    let mut doc = Document::default();
    let mut section = String::new();
    loop {
        p.skip_blank();
        let Some(c) = p.peek() else { break };
        let line = p.line;
        if c == '[' {
            match p.header() {
                Ok(name) => {
                    doc.sections.push((name.clone(), line));
                    section = name;
                }
                Err(message) => {
                    doc.problems.push(Problem { line: p.line, message });
                    p.skip_line();
                }
            }
        } else {
            match p.entry() {
                Ok((key, value)) => doc.entries.push(Entry { section: section.clone(), key, value, line }),
                Err(message) => {
                    doc.problems.push(Problem { line: p.line, message });
                    p.skip_line();
                }
            }
        }
    }
    doc
}

struct Parser {
    s: Vec<char>,
    i: usize,
    line: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    /// Spaces and tabs only.
    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r')) {
            self.i += 1;
        }
    }

    /// Whitespace, newlines and comments.
    fn skip_blank(&mut self) {
        loop {
            match self.peek() {
                Some(' ' | '\t' | '\r' | '\n') => {
                    self.bump();
                }
                Some('#') => self.skip_comment(),
                _ => break,
            }
        }
    }

    fn skip_comment(&mut self) {
        while !matches!(self.peek(), None | Some('\n')) {
            self.i += 1;
        }
    }

    /// Drops the rest of the current line (after a problem).
    fn skip_line(&mut self) {
        while let Some(c) = self.peek() {
            self.bump();
            if c == '\n' {
                break;
            }
        }
    }

    fn header(&mut self) -> Result<String, String> {
        self.bump(); // [
        if self.peek() == Some('[') {
            return Err("lists of tables ([[name]]) are not supported; use [name]".into());
        }
        let mut name = String::new();
        loop {
            match self.peek() {
                Some(']') => {
                    self.bump();
                    break;
                }
                Some('\n') | None => return Err("a section header needs a closing ']'".into()),
                Some(c) => {
                    name.push(c);
                    self.i += 1;
                }
            }
        }
        let name = name.trim().to_string();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
            return Err(format!("'[{name}]' is not a section name (letters, digits, '_' and '-' only)"));
        }
        self.end_of_line()?;
        Ok(name)
    }

    /// Only a comment may follow on the line.
    fn end_of_line(&mut self) -> Result<(), String> {
        self.skip_space();
        match self.peek() {
            None | Some('\n') => Ok(()),
            Some('#') => {
                self.skip_comment();
                Ok(())
            }
            Some(_) => Err("something follows the value on the same line".into()),
        }
    }

    fn entry(&mut self) -> Result<(String, Value), String> {
        let mut key = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                key.push(c);
                self.i += 1;
            } else {
                break;
            }
        }
        if key.is_empty() {
            let shown: String = self.s[self.i..].iter().take_while(|c| **c != '\n').take(20).collect();
            return Err(format!("can't read this line ('{}'); expected key = value or [section]", shown.trim_end()));
        }
        self.skip_space();
        match self.peek() {
            Some('=') => {
                self.i += 1;
            }
            Some('.') => return Err(format!("dotted keys ('{key}.…') are not supported; use a [section]")),
            _ => return Err(format!("'{key}' needs '= value' after it")),
        }
        self.skip_space();
        let value = self.value()?;
        self.end_of_line()?;
        Ok((key, value))
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            None | Some('\n' | '#') => Err("expected a value after '='".into()),
            Some('"') => self.basic_string().map(Value::Str),
            Some('\'') => self.literal_string().map(Value::Str),
            Some('[') => self.list(),
            Some(_) => self.bare(),
        }
    }

    fn basic_string(&mut self) -> Result<String, String> {
        self.bump(); // "
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => return Err("a string needs a closing '\"'".into()),
                Some('"') => {
                    self.bump();
                    return Ok(out);
                }
                Some('\\') => {
                    self.bump();
                    match self.peek() {
                        Some('n') => out.push('\n'),
                        Some('t') => out.push('\t'),
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('u') => {
                            self.bump();
                            out.push(self.unicode(4)?);
                            continue;
                        }
                        Some('U') => {
                            self.bump();
                            out.push(self.unicode(8)?);
                            continue;
                        }
                        Some(c) if c != '\n' => return Err(format!("'\\{c}' is not a known escape in a string (use \\\\ for a backslash)")),
                        _ => return Err("a string needs a closing '\"'".into()),
                    }
                    self.bump();
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn unicode(&mut self, digits: usize) -> Result<char, String> {
        let mut hex = String::new();
        for _ in 0..digits {
            match self.peek() {
                Some(c) if c.is_ascii_hexdigit() => {
                    hex.push(c);
                    self.bump();
                }
                _ => break,
            }
        }
        u32::from_str_radix(&hex, 16).ok().filter(|_| hex.len() == digits).and_then(char::from_u32).ok_or_else(|| format!("'\\u{hex}' is not a character"))
    }

    fn literal_string(&mut self) -> Result<String, String> {
        self.bump(); // '
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => return Err("a string needs a closing \"'\"".into()),
                Some('\'') => {
                    self.bump();
                    return Ok(out);
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn list(&mut self) -> Result<Value, String> {
        self.bump(); // [
        let mut items = Vec::new();
        loop {
            self.skip_blank();
            match self.peek() {
                None => return Err("a list needs a closing ']'".into()),
                Some(']') => {
                    self.bump();
                    return Ok(Value::List(items));
                }
                Some(',') => return Err("a list has a ',' with no value before it".into()),
                Some('[') => return Err("lists inside lists are not supported".into()),
                Some(_) => items.push(self.value()?),
            }
            self.skip_blank();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {}
                None => return Err("a list needs a closing ']'".into()),
                Some(_) => return Err("list values are separated by ','".into()),
            }
        }
    }

    /// A number, `true` or `false`; anything else is most likely a string without quotes.
    fn bare(&mut self) -> Result<Value, String> {
        let mut word = String::new();
        while let Some(c) = self.peek() {
            if matches!(c, ',' | ']' | '#') || c.is_whitespace() {
                break;
            }
            word.push(c);
            self.i += 1;
        }
        match word.as_str() {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            _ => {}
        }
        if let Ok(n) = word.replace('_', "").parse::<i64>() {
            return Ok(Value::Int(n));
        }
        if word.parse::<f64>().is_ok() {
            return Err(format!("'{word}': only whole numbers are used here"));
        }
        Err(format!("can't read the value '{word}'; text needs quotes, like \"{word}\""))
    }
}
