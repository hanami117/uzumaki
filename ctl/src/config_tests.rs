//! `uzu.toml`: reading, falling back, warnings and hints, colours, keys and status formats.
//! (What the screens do with a config is in `tui/tests.rs`.)

use std::path::PathBuf;

use crate::config::toml::{self, Value};
use crate::config::{self, Config, did_you_mean};

fn lines(cfg: &Config) -> Vec<String> {
    cfg.warnings.iter().map(ToString::to_string).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("uzu-config-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------------------- the reader

#[test]
fn reads_sections_strings_numbers_bools_and_lists() {
    let doc = toml::parse("top = 1\n[a]\ns = \"x y\"  # note\nl = 'lit\\eral'\nn = 42\nb = true\nlist = [\"a\", 'b',\n  \"c\", # inline\n]\nempty = []\n\n[b-c]\nz = false\n");
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    let get = |section: &str, key: &str| doc.entries.iter().find(|e| e.section == section && e.key == key).map(|e| e.value.clone());
    assert_eq!(get("", "top"), Some(Value::Int(1)));
    assert_eq!(get("a", "s"), Some(Value::Str("x y".into())));
    assert_eq!(get("a", "l"), Some(Value::Str("lit\\eral".into())));
    assert_eq!(get("a", "n"), Some(Value::Int(42)));
    assert_eq!(get("a", "b"), Some(Value::Bool(true)));
    assert_eq!(get("a", "list"), Some(Value::List(vec![Value::Str("a".into()), Value::Str("b".into()), Value::Str("c".into())])));
    assert_eq!(get("a", "empty"), Some(Value::List(vec![])));
    assert_eq!(get("b-c", "z"), Some(Value::Bool(false)));
    assert_eq!(doc.sections, vec![("a".to_string(), 2), ("b-c".to_string(), 12)]);
    // The line of an entry is the line of its key, also for a list that runs on.
    assert_eq!(doc.entries.iter().find(|e| e.key == "list").unwrap().line, 7);
}

#[test]
fn string_escapes() {
    let doc = toml::parse("[a]\ns = \"tab\\t quote\\\" back\\\\ dash \\u2014 end\"\n");
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    assert_eq!(doc.entries[0].value, Value::Str("tab\t quote\" back\\ dash — end".into()));
}

#[test]
fn a_bad_line_costs_only_that_line() {
    let doc = toml::parse("[a]\nx = 1\nthis is nonsense\ny = \"open\nz = [1, 2\nw = 3\n");
    let ok: Vec<&str> = doc.entries.iter().map(|e| e.key.as_str()).collect();
    assert!(ok.contains(&"x"));
    let bad: Vec<usize> = doc.problems.iter().map(|p| p.line).collect();
    assert!(bad.contains(&3) && bad.contains(&4), "{bad:?}");
}

#[test]
fn problems_say_what_is_wrong() {
    let msg = |text: &str| toml::parse(text).problems.first().map(|p| p.message.clone()).unwrap_or_default();
    assert!(msg("a = coral").contains("needs quotes"), "{}", msg("a = coral"));
    assert!(msg("a = \"x").contains("closing"));
    assert!(msg("a =").contains("expected a value"));
    assert!(msg("a 1").contains("'= value'"));
    assert!(msg("a = 1.5").contains("whole numbers"));
    assert!(msg("[a").contains("closing ']'"));
    assert!(msg("[[a]]").contains("not supported"));
    assert!(msg("a.b = 1").contains("dotted"));
    assert!(msg("a = 1 2").contains("follows"));
    assert!(msg("a = \"\\q\"").contains("escape"));
    assert!(msg("a = [,]").contains("','"));
}

#[test]
fn nothing_loops_or_panics_on_odd_input() {
    for text in ["", "\n\n", "[", "]", "=", "= 1", "a = [", "a = [[", "a = \"\\", "a = '", "\u{feff}", "a = \"\\u12", "#", "[a]]", "a = [\"x\" \"y\"]", "\0\0", "a = \"\u{1F600}\"", "ключ = 1"] {
        let _ = toml::parse(text);
        let _ = Config::parse(text);
    }
}

// ---------------------------------------------------------------------------------- loading

#[test]
fn no_file_is_the_defaults_without_a_word() {
    let dir = scratch("missing");
    let cfg = Config::load_from(&dir.join("uzu.toml"));
    assert!(!cfg.found);
    assert!(cfg.warnings.is_empty());
    assert_eq!(cfg.status_line(), None);
    assert_eq!(cfg.path, Some(dir.join("uzu.toml")));
}

#[test]
fn an_empty_or_comment_only_file_is_fine() {
    for text in ["", "# just a note\n", "\n\n[theme]\n", "\u{feff}[theme]\n"] {
        let cfg = Config::parse(text);
        assert!(cfg.warnings.is_empty(), "{text:?}: {:?}", lines(&cfg));
    }
}

#[test]
fn an_unreadable_file_is_a_warning_not_a_crash() {
    let dir = scratch("unreadable");
    // A directory where the file should be.
    std::fs::create_dir_all(dir.join("uzu.toml")).unwrap();
    let cfg = Config::load_from(&dir.join("uzu.toml"));
    assert_eq!(cfg.warnings.len(), 1);
    assert!(cfg.warnings[0].message.starts_with("can't read the file"), "{}", cfg.warnings[0]);
}

#[test]
fn a_file_that_is_not_utf8_still_loads() {
    let dir = scratch("latin1");
    std::fs::write(dir.join("uzu.toml"), b"[format]\nstatus_idle = \"caf\xe9\"\n").unwrap();
    let cfg = Config::load_from(&dir.join("uzu.toml"));
    assert!(cfg.found);
    assert!(cfg.warnings[0].message.contains("UTF-8"));
}

#[test]
fn the_path_follows_xdg_first_and_ignores_a_relative_one() {
    let os = |s: &str| Some(std::ffi::OsString::from(s));
    assert_eq!(config::path_in(os("/tmp/x"), os("/home/me")), Some(PathBuf::from("/tmp/x/uzumaki/uzu.toml")));
    assert_eq!(config::path_in(None, os("/home/me")), Some(PathBuf::from("/home/me/.config/uzumaki/uzu.toml")));
    assert_eq!(config::path_in(os("relative"), os("/home/me")), Some(PathBuf::from("/home/me/.config/uzumaki/uzu.toml")));
    assert_eq!(config::path_in(None, None), None);
}

// ------------------------------------------------------------------------ warnings and hints

#[test]
fn unknown_keys_get_the_line_and_a_hint() {
    let cfg = Config::parse("[theme]\nacent = \"red\"\n");
    assert_eq!(lines(&cfg), ["line 2: unknown key 'acent' in [theme]; did you mean 'accent'?"]);
}

#[test]
fn a_key_in_the_wrong_section_says_where_it_lives() {
    let cfg = Config::parse("[theme]\nstatus_format = \"x\"\n");
    assert_eq!(lines(&cfg), ["line 2: 'status_format' belongs under [format], not [theme]"]);
    let cfg = Config::parse("status_format = \"x\"\n");
    assert_eq!(lines(&cfg), ["line 1: 'status_format' is outside any section; put it under [format]"]);
}

#[test]
fn a_key_nobody_knows_lists_the_keys_of_the_section() {
    let cfg = Config::parse("[format]\nbanana = 1\n");
    assert!(lines(&cfg)[0].contains("the keys there are status_format, status_idle"), "{:?}", lines(&cfg));
}

#[test]
fn unknown_sections_are_reported_once_and_ignored() {
    let cfg = Config::parse("[them]\na = 1\nb = 2\n");
    assert_eq!(lines(&cfg), ["line 1: unknown section [them]; did you mean [theme]? Its lines are ignored."]);
    let cfg = Config::parse("[zzz]\n");
    assert!(lines(&cfg)[0].contains("the sections are [keys], [theme]"), "{:?}", lines(&cfg));
}

#[test]
fn a_repeated_key_says_which_line_wins() {
    let cfg = Config::parse("[format]\nstatus_idle = \"a\"\nstatus_idle = \"b\"\n");
    assert_eq!(lines(&cfg), ["line 3: 'status_idle' is already set on line 2; this line wins"]);
}

#[test]
fn warnings_come_in_line_order_and_the_rest_still_counts() {
    let cfg = Config::parse("[zzz]\n[theme]\nacent = 1\n[format]\nstatus_idle = \"ok\"\nbad line\n");
    let order: Vec<usize> = cfg.warnings.iter().map(|w| w.line).collect();
    assert_eq!(order, [1, 3, 6]);
}

#[test]
fn the_status_line_shows_the_first_problem_and_counts_the_rest() {
    let mut cfg = Config::parse("[theme]\nacent = 1\nbad line\n");
    cfg.path = Some(PathBuf::from("/x/uzu.toml"));
    let text = cfg.status_line().unwrap();
    assert!(text.starts_with("uzu.toml line 2: unknown key 'acent'"), "{text}");
    assert!(text.ends_with("(+1 more; 'uzu config' lists them)"), "{text}");
    let one = Config::parse("[theme]\nacent = 1\n");
    assert!(!one.status_line().unwrap().contains("more"));
}

#[test]
fn did_you_mean_finds_typos_and_not_other_words() {
    let known = ["accent", "number", "album", "palette"];
    assert_eq!(did_you_mean("acent", &known), Some("accent"));
    assert_eq!(did_you_mean("Accent", &known), Some("accent"));
    assert_eq!(did_you_mean("palete", &known), Some("palette"));
    assert_eq!(did_you_mean("numbr", &known), Some("number"));
    assert_eq!(did_you_mean("banana", &known), None);
    assert_eq!(did_you_mean("", &known), None);
    assert_eq!(did_you_mean("alb", &known), Some("album"));
    // `-` and `_` are the same.
    assert_eq!(did_you_mean("seek-forward", &["seek_forward"]), Some("seek_forward"));
    assert_eq!(config::edit_distance("kitten", "sitting"), 3);
}
