//! `uzu.toml`: reading, falling back, warnings and hints, colours, keys and status formats.
//! (What the screens do with a config is in `tui/tests.rs`.)

use std::path::PathBuf;

use crate::config::reader;
use toml::Value;
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

fn get(doc: &reader::Document, section: &str, key: &str) -> Option<Value> {
    doc.entries.iter().find(|e| e.section == section && e.key == key).map(|e| e.value.clone())
}

fn problem_lines(text: &str) -> Vec<usize> {
    reader::parse(text).problems.iter().map(|p| p.line).collect()
}

#[test]
fn reads_sections_strings_numbers_bools_and_lists() {
    let doc = reader::parse("top = 1\n[a]\ns = \"x y\"  # note\nl = 'lit\\eral'\nn = 42\nb = true\nlist = [\"a\", 'b',\n  \"c\", # inline\n]\nempty = []\n\n[b-c]\nz = false\n");
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    assert_eq!(get(&doc, "", "top"), Some(Value::Integer(1)));
    assert_eq!(get(&doc, "a", "s"), Some(Value::String("x y".into())));
    assert_eq!(get(&doc, "a", "l"), Some(Value::String("lit\\eral".into())));
    assert_eq!(get(&doc, "a", "n"), Some(Value::Integer(42)));
    assert_eq!(get(&doc, "a", "b"), Some(Value::Boolean(true)));
    assert_eq!(get(&doc, "a", "list"), Some(Value::Array(vec!["a".into(), "b".into(), "c".into()])));
    assert_eq!(get(&doc, "a", "empty"), Some(Value::Array(vec![])));
    assert_eq!(get(&doc, "b-c", "z"), Some(Value::Boolean(false)));
    assert_eq!(doc.sections, vec![("a".to_string(), 2), ("b-c".to_string(), 12)]);
    // The line of an entry is the line of its key, also for a list that runs on; entries are in file order.
    assert_eq!(doc.entries.iter().find(|e| e.key == "list").unwrap().line, 7);
    let lines: Vec<usize> = doc.entries.iter().map(|e| e.line).collect();
    assert!(lines.windows(2).all(|w| w[0] < w[1]), "{lines:?}");
}

#[test]
fn string_escapes() {
    let doc = reader::parse("[a]\ns = \"tab\\t quote\\\" back\\\\ dash \\u2014 end\"\n");
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    assert_eq!(doc.entries[0].value, Value::String("tab\t quote\" back\\ dash — end".into()));
}

#[test]
fn what_the_old_reader_refused_now_reads() {
    // Dotted keys: `keys.next = "n"` at the top is the section [keys] with a key `next`.
    let doc = reader::parse("keys.next = \"n\"\ntheme.accent = \"coral\"\n");
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    assert_eq!(get(&doc, "keys", "next"), Some(Value::String("n".into())));
    assert_eq!(doc.sections.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["keys", "theme"]);
    // Floats, inline tables, hex numbers, multi-line strings.
    let doc = reader::parse("[a]\nf = 1.5\nt = { x = 1 }\nh = 0xff\nm = \"\"\"\nline\"\"\"\n");
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    assert_eq!(get(&doc, "a", "f"), Some(Value::Float(1.5)));
    assert_eq!(get(&doc, "a", "h"), Some(Value::Integer(255)));
    assert!(matches!(get(&doc, "a", "t"), Some(Value::Table(_))));
    // A section can be given by a dotted header too.
    let doc = reader::parse("[theme]\naccent = \"red\"\n[theme.extra]\nx = 1\n");
    assert_eq!(get(&doc, "theme", "accent"), Some(Value::String("red".into())));
    assert!(matches!(get(&doc, "theme", "extra"), Some(Value::Table(_))));
}

#[test]
fn a_bad_line_costs_only_that_line() {
    let doc = reader::parse("[a]\nx = 1\nthis is nonsense\nq = 2\n");
    assert_eq!(doc.entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["x", "q"]);
    assert_eq!(problem_lines("[a]\nx = 1\nthis is nonsense\nq = 2\n"), [3]);
    // An unclosed string stops at its line.
    let doc = reader::parse("[a]\nx = 1\ny = \"open\nz = 2\n");
    assert_eq!(doc.entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["x", "z"]);
    assert_eq!(problem_lines("[a]\nx = 1\ny = \"open\nz = 2\n"), [3]);
    // The crate's guess for a broken line is not used: no `a = coral` with a "fixed" value.
    let doc = reader::parse("[a]\nc = coral\nn = 1 2\ne =\nok = 1\n");
    assert_eq!(doc.entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["ok"]);
    assert_eq!(doc.problems.iter().map(|p| p.line).collect::<Vec<_>>(), [2, 3, 4]);
}

#[test]
fn an_unclosed_list_does_not_swallow_the_file() {
    let doc = reader::parse("[a]\nx = 1\nz = [1, 2\nw = 3\n[b]\nq = 1\n");
    assert_eq!(doc.entries.iter().map(|e| (e.section.as_str(), e.key.as_str())).collect::<Vec<_>>(), [("a", "x"), ("a", "w"), ("b", "q")]);
    assert_eq!(doc.problems.len(), 1, "{:?}", doc.problems);
    assert_eq!(doc.problems[0].line, 3);
    assert!(doc.problems[0].message.contains("unclosed"), "{}", doc.problems[0].message);
}

#[test]
fn a_bad_header_costs_its_section_not_the_file() {
    let doc = reader::parse("[a b]\nx = 1\n[c]\ny = 2\n");
    assert_eq!(doc.problems.iter().map(|p| p.line).collect::<Vec<_>>(), [1]);
    assert_eq!(doc.sections, vec![("c".to_string(), 3)]);
    assert_eq!(get(&doc, "c", "y"), Some(Value::Integer(2)));
    // [[name]] is a list of tables: said once, at its line, and its lines are ignored.
    let doc = reader::parse("[[keys]]\nnext = \"n\"\n[theme]\naccent = \"red\"\n");
    assert_eq!(doc.problems.len(), 1);
    assert!(doc.problems[0].message.contains("[[keys]]"), "{:?}", doc.problems);
    assert!(get(&doc, "keys", "next").is_none());
    assert!(get(&doc, "theme", "accent").is_some());
}

#[test]
fn duplicates_keep_the_first() {
    let doc = reader::parse("[a]\nx = 1\nx = 2\n");
    assert_eq!(get(&doc, "a", "x"), Some(Value::Integer(1)));
    assert_eq!(doc.problems.len(), 1);
    assert_eq!(doc.problems[0].line, 3);
    assert!(doc.problems[0].message.contains("duplicate key"), "{}", doc.problems[0].message);
}

#[test]
fn a_number_too_big_is_a_problem_of_its_own() {
    let doc = reader::parse("[a]\nx = 99999999999999999999\ny = 1\n");
    assert_eq!(doc.problems.iter().map(|p| p.line).collect::<Vec<_>>(), [2]);
    assert_eq!(get(&doc, "a", "y"), Some(Value::Integer(1)));
}

#[test]
fn nothing_loops_or_panics_on_odd_input() {
    for text in ["", "\n\n", "[", "]", "=", "= 1", "a = [", "a = [[", "a = \"\\", "a = '", "\u{feff}", "a = \"\\u12", "#", "[a]]", "a = [\"x\" \"y\"]", "\0\0", "a = \"\u{1F600}\"", "ключ = 1", "a = {", "[a]\nb = [\n[c]\nd = [\n", "[[a]]\n[[a]]\n", "a.b.c.d = 1\na.b = 2"] {
        let _ = reader::parse(text);
        let _ = Config::parse(text);
    }
}

#[test]
fn dotted_keys_and_floats_go_through_the_same_checks() {
    // Dotted keys are the section's keys.
    let cfg = Config::parse("keys.next = \"x\"\ntheme.palette = \"mono\"\nformat.status_idle = \"idle\"\n");
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    assert_eq!(cfg.status_idle, "idle");
    assert_eq!(cfg.theme, crate::config::theme::Theme::mono());
    // A float where a whole number goes is that key's warning only.
    let cfg = Config::parse("[layout]\nartist_width = 2.5\ntitle_width = 7\n");
    assert_eq!(lines(&cfg), ["line 2: 'artist_width' needs a whole number (got a number with decimals)"]);
    assert_eq!(cfg.layout.columns.title, 7);
    // A table where a value goes, and a [[list of tables]] for a section.
    let cfg = Config::parse("[layout]\nlyrics = { a = 1 }\nshow_album = false\n[[keys]]\nnext = \"x\"\n");
    assert_eq!(lines(&cfg).len(), 2, "{:?}", lines(&cfg));
    assert!(!cfg.layout.columns.show_album);
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
fn a_repeated_key_keeps_the_first() {
    let cfg = Config::parse("[format]\nstatus_idle = \"a\"\nstatus_idle = \"b\"\n");
    assert_eq!(lines(&cfg), ["line 3: duplicate key; the first one counts"]);
    assert_eq!(cfg.status_idle, "a");
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

// -------------------------------------------------------------------------------------- theme

use crate::config::theme::{CORAL, Theme, parse_colour};
use ratatui::style::Color;

#[test]
fn colours_in_every_form() {
    assert_eq!(parse_colour("#ff7a8a"), Ok(Some(Color::Rgb(255, 122, 138))));
    assert_eq!(parse_colour("#F7A"), Ok(Some(Color::Rgb(255, 119, 170))));
    assert_eq!(parse_colour("Red"), Ok(Some(Color::Red)));
    assert_eq!(parse_colour("light-blue"), Ok(Some(Color::LightBlue)));
    assert_eq!(parse_colour("light blue"), Ok(Some(Color::LightBlue)));
    assert_eq!(parse_colour("bright-green"), Ok(Some(Color::LightGreen)));
    assert_eq!(parse_colour("grey"), Ok(Some(Color::Gray)));
    assert_eq!(parse_colour("dark_gray"), Ok(Some(Color::DarkGray)));
    assert_eq!(parse_colour("coral"), Ok(Some(CORAL)));
    assert_eq!(parse_colour("208"), Ok(Some(Color::Indexed(208))));
    assert_eq!(parse_colour("none"), Ok(None));
    assert_eq!(parse_colour("default"), Ok(None));
}

#[test]
fn bad_colours_explain_themselves() {
    assert!(parse_colour("#12").unwrap_err().contains("#rrggbb or #rgb"));
    assert!(parse_colour("#gggggg").unwrap_err().contains("#rrggbb"));
    assert!(parse_colour("rd").unwrap_err().contains("did you mean 'red'?"), "{:?}", parse_colour("rd"));
    assert!(parse_colour("lightblu").unwrap_err().contains("light-blue"));
    assert!(parse_colour("banana").unwrap_err().contains("use #rrggbb"));
    assert!(parse_colour("300").is_err());
}

#[test]
fn no_theme_section_is_today_s_colours() {
    let t = Config::parse("").theme;
    assert_eq!(t, Theme::default());
    assert_eq!(t.accent, Some(Color::Rgb(255, 122, 138)));
    assert_eq!((t.number, t.album, t.time, t.paused), (Some(Color::Green), Some(Color::Cyan), Some(Color::Magenta), Some(Color::Yellow)));
}

#[test]
fn roles_and_palettes() {
    let cfg = Config::parse("[theme]\naccent = \"#88c0d0\"\nnumber = 208\nalbum = \"none\"\ntime = \"light-yellow\"\npaused = \"red\"\n");
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    assert_eq!(cfg.theme, Theme { accent: Some(Color::Rgb(0x88, 0xc0, 0xd0)), number: Some(Color::Indexed(208)), album: None, time: Some(Color::LightYellow), paused: Some(Color::Red) });
    // The palette is the base whatever the order of the lines; roles go on top.
    let cfg = Config::parse("[theme]\naccent = \"blue\"\npalette = \"mono\"\n");
    assert_eq!(cfg.theme, Theme { accent: Some(Color::Blue), ..Theme::mono() });
    let cfg = Config::parse("[theme]\npalette = \"terminal\"\n");
    assert_eq!(cfg.theme.accent, Some(Color::Red));
    assert!(cfg.warnings.is_empty());
}

#[test]
fn a_bad_colour_keeps_the_old_one_and_warns() {
    let cfg = Config::parse("[theme]\naccent = \"rd\"\nnumber = true\ntime = 999\npalette = \"monoo\"\n");
    assert_eq!(cfg.theme, Theme::default());
    let w = lines(&cfg);
    assert_eq!(w.len(), 4, "{w:?}");
    assert!(w[0].starts_with("line 2: 'rd' is not a colour; did you mean 'red'?"), "{}", w[0]);
    assert!(w[1].contains("'number' needs a colour in quotes (got true or false)"), "{}", w[1]);
    assert!(w[2].contains("0 to 255"), "{}", w[2]);
    assert!(w[3].contains("unknown palette 'monoo'; did you mean 'mono'?"), "{}", w[3]);
}

#[test]
fn no_color_turns_the_colours_off_unless_the_file_says_otherwise() {
    let cfg = Config::parse("[theme]\naccent = \"red\"\n").with_no_color(true);
    assert_eq!(cfg.theme, Theme::mono());
    let cfg = Config::parse("[theme]\naccent = \"red\"\n").with_no_color(false);
    assert_eq!(cfg.theme.accent, Some(Color::Red));
    let cfg = Config::parse("[theme]\naccent = \"red\"\nrespect_no_color = false\n").with_no_color(true);
    assert_eq!(cfg.theme.accent, Some(Color::Red));
    let cfg = Config::parse("[theme]\nrespect_no_color = \"no\"\n");
    assert!(lines(&cfg)[0].contains("'respect_no_color' needs true or false"), "{:?}", lines(&cfg));
    // With no config at all, NO_COLOR still counts.
    assert_eq!(Config::default().with_no_color(true).theme, Theme::mono());
}

// --------------------------------------------------------------------------------------- keys

use crate::config::keys::{ACTIONS, Chord, Keymap, parse_key};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn chord(text: &str) -> Chord {
    parse_key(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn press(map: &Keymap, code: KeyCode, mods: KeyModifiers) -> Option<(KeyCode, KeyModifiers)> {
    map.translate(KeyEvent::new(code, mods)).map(|e| (e.code, e.modifiers))
}

fn ch(c: char) -> KeyCode {
    KeyCode::Char(c)
}

fn keymap(text: &str) -> (Keymap, Vec<String>) {
    let cfg = Config::parse(text);
    (cfg.keys.clone(), lines(&cfg))
}

#[test]
fn every_default_key_parses_and_belongs_to_one_action() {
    let mut seen: Vec<(Chord, &str)> = Vec::new();
    for a in ACTIONS {
        assert!(!a.keys.is_empty(), "{}", a.name);
        for k in a.keys {
            let c = chord(k);
            assert!(!seen.iter().any(|(s, _)| *s == c), "{k} is the default of two actions ({} and {})", a.name, seen.iter().find(|(s, _)| *s == c).unwrap().1);
            seen.push((c, a.name));
        }
    }
    let mut names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), ACTIONS.len());
}

#[test]
fn keys_in_every_spelling() {
    assert_eq!(chord("n"), Chord::plain(ch('n')));
    assert_eq!(chord("N"), Chord::plain(ch('N')));
    assert_eq!(chord("?"), Chord::plain(ch('?')));
    assert_eq!(chord("+"), Chord::plain(ch('+')));
    assert_eq!(chord("space"), Chord::plain(ch(' ')));
    assert_eq!(chord("Space"), Chord::plain(ch(' ')));
    assert_eq!(chord("ENTER"), Chord::plain(KeyCode::Enter));
    assert_eq!(chord("return"), Chord::plain(KeyCode::Enter));
    assert_eq!(chord("esc"), Chord::plain(KeyCode::Esc));
    assert_eq!(chord("pgdn"), Chord::plain(KeyCode::PageDown));
    assert_eq!(chord("PageUp"), Chord::plain(KeyCode::PageUp));
    assert_eq!(chord("del"), Chord::plain(KeyCode::Delete));
    assert_eq!(chord("f5"), Chord::plain(KeyCode::F(5)));
    assert_eq!(chord("comma"), Chord::plain(ch(',')));
    assert_eq!(chord("ctrl+n"), Chord { code: ch('n'), ctrl: true, alt: false });
    assert_eq!(chord("Ctrl+N"), chord("ctrl+n"));
    assert_eq!(chord("alt+x"), Chord { code: ch('x'), ctrl: false, alt: true });
    assert_eq!(chord("ctrl+alt+x"), Chord { code: ch('x'), ctrl: true, alt: true });
    assert_eq!(chord("shift+n"), Chord::plain(ch('N')));
    assert_eq!(chord("shift+tab"), Chord::plain(KeyCode::BackTab));
    assert_eq!(chord("ctrl++"), Chord { code: ch('+'), ctrl: true, alt: false });
    assert_eq!(chord("ctrl+space"), Chord { code: ch(' '), ctrl: true, alt: false });
}

#[test]
fn bad_keys_explain_themselves() {
    let err = |t: &str| parse_key(t).unwrap_err();
    assert!(err("").contains("empty"));
    assert!(err("ctrl+").contains("is not a key"), "{}", err("ctrl+"));
    assert!(err("entr").contains("did you mean 'enter'?"), "{}", err("entr"));
    assert!(err("pagedwon").contains("did you mean 'pagedown'?"), "{}", err("pagedwon"));
    assert!(err("banana").contains("one character, or a name like 'space'"), "{}", err("banana"));
    assert!(err("shift+1").contains("shift only goes with a letter"), "{}", err("shift+1"));
    assert!(err("f13").contains("is not a key"));
}

#[test]
fn no_keys_section_changes_nothing() {
    let (map, w) = keymap("");
    assert!(w.is_empty() && map.is_default());
    assert_eq!(press(&map, ch('n'), KeyModifiers::NONE), Some((ch('n'), KeyModifiers::NONE)));
    // Even a Ctrl key goes through exactly as before (the screens read Ctrl+N as n).
    assert_eq!(press(&map, ch('n'), KeyModifiers::CONTROL), Some((ch('n'), KeyModifiers::CONTROL)));
    assert_eq!(map.relabel("n  p"), "n  p");
    assert_eq!(map.relabel_hint("n/p next/prev"), "n/p next/prev");
}

#[test]
fn a_new_key_stands_for_the_action_and_the_old_one_is_dropped() {
    let (map, w) = keymap("[keys]\nnext = \"N\"\n");
    assert!(w.len() == 1 && w[0].contains("'N' used to be 'play_next'"), "{w:?}");
    // N (bound) → n, the key the screens answer to; the old n does nothing.
    assert_eq!(press(&map, ch('N'), KeyModifiers::SHIFT), Some((ch('n'), KeyModifiers::NONE)));
    assert_eq!(press(&map, ch('n'), KeyModifiers::NONE), None);
    // Others are untouched.
    assert_eq!(press(&map, ch('p'), KeyModifiers::NONE), Some((ch('p'), KeyModifiers::NONE)));
}

#[test]
fn several_keys_per_action_and_modifiers() {
    let (map, w) = keymap("[keys]\nnext = [\"ctrl+n\", \"alt+n\", \"]\"]\n");
    assert!(w.is_empty(), "{w:?}");
    for (code, mods) in [(ch('n'), KeyModifiers::CONTROL), (ch('n'), KeyModifiers::ALT), (ch(']'), KeyModifiers::NONE)] {
        assert_eq!(press(&map, code, mods), Some((ch('n'), KeyModifiers::NONE)), "{code:?} {mods:?}");
    }
    // An unbound Ctrl or Alt key does nothing once there is a keymap.
    assert_eq!(press(&map, ch('j'), KeyModifiers::CONTROL), None);
    assert_eq!(press(&map, ch('x'), KeyModifiers::ALT), None);
    // Plain unmentioned keys pass.
    assert_eq!(press(&map, ch('j'), KeyModifiers::NONE), Some((ch('j'), KeyModifiers::NONE)));
}

#[test]
fn an_empty_list_unbinds() {
    let (map, w) = keymap("[keys]\nquit = []\n");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(press(&map, ch('q'), KeyModifiers::NONE), None);
    assert_eq!(map.relabel("q  Ctrl+C"), "–  Ctrl+C");
}

#[test]
fn a_list_replaces_all_the_defaults_of_the_action() {
    let (map, _) = keymap("[keys]\ndown = [\"ctrl+n\"]\n");
    assert_eq!(press(&map, ch('j'), KeyModifiers::NONE), None);
    assert_eq!(press(&map, KeyCode::Down, KeyModifiers::NONE), None);
    assert_eq!(press(&map, ch('n'), KeyModifiers::CONTROL), Some((ch('j'), KeyModifiers::NONE)));
    // Adding the old ones back keeps them.
    let (map, _) = keymap("[keys]\ndown = [\"j\", \"down\", \"ctrl+n\"]\n");
    assert_eq!(press(&map, ch('j'), KeyModifiers::NONE), Some((ch('j'), KeyModifiers::NONE)));
    assert_eq!(press(&map, KeyCode::Down, KeyModifiers::NONE), Some((ch('j'), KeyModifiers::NONE)));
    assert_eq!(press(&map, ch('n'), KeyModifiers::CONTROL), Some((ch('j'), KeyModifiers::NONE)));
}

#[test]
fn swapping_two_actions_works_and_warns_of_nothing() {
    let (map, w) = keymap("[keys]\nnext = \"p\"\nprev = \"n\"\n");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(press(&map, ch('p'), KeyModifiers::NONE), Some((ch('n'), KeyModifiers::NONE)));
    assert_eq!(press(&map, ch('n'), KeyModifiers::NONE), Some((ch('p'), KeyModifiers::NONE)));
}

#[test]
fn the_same_key_for_two_actions_warns_and_the_first_keeps_it() {
    let (map, w) = keymap("[keys]\nshuffle = \"x\"\nrepeat = [\"x\", \"y\"]\n");
    assert_eq!(w, ["line 3: 'x' is already used for 'shuffle' (line 2); it stays there"]);
    assert_eq!(press(&map, ch('x'), KeyModifiers::NONE), Some((ch('s'), KeyModifiers::NONE)));
    assert_eq!(press(&map, ch('y'), KeyModifiers::NONE), Some((ch('r'), KeyModifiers::NONE)));
}

#[test]
fn taking_a_key_of_an_action_you_did_not_touch_warns() {
    let (map, w) = keymap("[keys]\nshuffle = \"n\"\n");
    assert_eq!(w, ["line 2: 'n' used to be 'next'; it now does 'shuffle', and 'next' loses that key"]);
    assert_eq!(press(&map, ch('n'), KeyModifiers::NONE), Some((ch('s'), KeyModifiers::NONE)));
}

#[test]
fn taking_a_key_that_means_something_on_some_screens_warns() {
    let (_, w) = keymap("[keys]\nshuffle = \"tab\"\n");
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("'tab' also switches panes"), "{}", w[0]);
    let (_, w) = keymap("[keys]\nshuffle = \"h\"\n");
    assert!(w[0].contains("'h' also switches panes"), "{}", w[0]);
}

#[test]
fn ctrl_c_cannot_be_bound() {
    let (map, w) = keymap("[keys]\nnext = [\"ctrl+c\", \"x\"]\n");
    assert_eq!(w, ["line 2: Ctrl+C always quits; it can't be bound to anything else"]);
    assert_eq!(press(&map, ch('x'), KeyModifiers::NONE), Some((ch('n'), KeyModifiers::NONE)));
}

#[test]
fn bad_names_and_bad_keys_get_hints_and_the_rest_still_counts() {
    let (map, w) = keymap("[keys]\nnxt = \"x\"\nprev = [\"ctrl+\", \"y\"]\nquit = [\"banana\"]\nrepeat = 5\nshuffle = true\nn = \"next\"\n");
    assert_eq!(w.len(), 6, "{w:?}");
    assert!(w[0].contains("unknown action 'nxt' in [keys]; did you mean 'next'?"), "{}", w[0]);
    assert!(w[1].contains("'ctrl+' is not a key"), "{}", w[1]);
    assert!(w[2].contains("'banana' is not a key"), "{}", w[2]);
    assert!(w[3].starts_with("line 5") || w[3].contains("needs keys in quotes"), "{}", w[3]);
    assert!(w.iter().any(|l| l.contains("write it as next = \"n\"")), "{w:?}");
    // The good key of the list works, and `quit` with only a bad key keeps its default.
    assert_eq!(press(&map, ch('y'), KeyModifiers::NONE), Some((ch('p'), KeyModifiers::NONE)));
    assert_eq!(press(&map, ch('q'), KeyModifiers::NONE), Some((ch('q'), KeyModifiers::NONE)));
    // A digit written as a number is a key.
    let (map, w) = keymap("[keys]\nrepeat = 9\n");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(press(&map, ch('9'), KeyModifiers::NONE), Some((ch('r'), KeyModifiers::NONE)));
}

#[test]
fn a_key_listed_twice_and_an_action_set_twice() {
    let (_, w) = keymap("[keys]\nnext = [\"x\", \"x\"]\n");
    assert_eq!(w, ["line 2: 'x' is listed twice for 'next'"]);
    let (map, w) = keymap("[keys]\nnext = \"x\"\nnext = \"y\"\n");
    assert_eq!(w, ["line 3: duplicate key; the first one counts"]);
    assert_eq!(press(&map, ch('x'), KeyModifiers::NONE), Some((ch('n'), KeyModifiers::NONE)));
    assert_eq!(press(&map, ch('y'), KeyModifiers::NONE), Some((ch('y'), KeyModifiers::NONE)));
}

#[test]
fn an_action_outside_keys_says_where_it_goes() {
    let cfg = Config::parse("[theme]\nnext = \"x\"\n");
    assert_eq!(lines(&cfg), ["line 2: 'next' belongs under [keys], not [theme]"]);
}

#[test]
fn help_and_hints_show_the_user_s_keys() {
    let (map, _) = keymap("[keys]\nnext = \"ctrl+n\"\ndown = [\"ctrl+j\", \"x\"]\n");
    assert_eq!(map.relabel("n  p"), "ctrl+n  p");
    assert_eq!(map.relabel("Up Down  j k"), "Up ctrl+j/x   k");
    assert_eq!(map.relabel("Space"), "Space");
    assert_eq!(map.relabel_hint("n/p next/prev"), "ctrl+n/p next/prev");
    assert_eq!(map.relabel_hint("Space play/pause"), "Space play/pause");
    assert_eq!(map.relabel_hint("q quit"), "q quit");
}

// ------------------------------------------------------------------------------------ formats

use crate::config::format::{self, Format};
use uzumaki_ctl::{PlayState, RepeatMode, SongInfo, Status};

fn playing() -> Status {
    Status {
        state: PlayState::Playing,
        song: Some(SongInfo { id: "s".into(), title: "Instant Crush".into(), artist: "Daft Punk".into(), album: "Random Access Memories".into(), duration: 239.0, ..Default::default() }),
        position: 72.0,
        duration: 239.0,
        volume: 0.82,
        shuffle: true,
        repeat: RepeatMode::All,
        source: "Anima".into(),
        index: Some(2),
        count: 12,
        queued: 3,
        connected: true,
        ..Default::default()
    }
}

fn show(text: &str) -> String {
    let (f, problems) = Format::parse(text);
    assert!(problems.is_empty(), "{text}: {problems:?}");
    f.render(&playing())
}

#[test]
fn the_example_from_the_plan() {
    assert_eq!(show("{artist} — {title} [{elapsed}/{duration}]"), "Daft Punk — Instant Crush [1:12/3:59]");
}

#[test]
fn every_placeholder() {
    assert_eq!(show("{artist}|{title}|{album}"), "Daft Punk|Instant Crush|Random Access Memories");
    assert_eq!(show("{elapsed} {duration} {remaining}"), "1:12 3:59 2:47");
    assert_eq!(show("{percent}% {volume}%"), "30% 82%");
    assert_eq!(show("{state} {icon}"), "playing ▶");
    assert_eq!(show("{shuffle} {repeat}"), "on all");
    assert_eq!(show("{source} {index}/{count} +{queued}"), "Anima 3/12 +3");
    // Every name in the list works (none is unknown).
    for name in format::FIELDS {
        assert!(Format::parse(&format!("{{{name}}}")).1.is_empty(), "{name}");
    }
}

#[test]
fn autoplay_shows_on_off_or_nothing() {
    let mut st = playing();
    assert_eq!(Format::parse("[{autoplay}]").0.render(&st), "[]", "an app that doesn't say");
    st.autoplay = Some(true);
    assert_eq!(Format::parse("[{autoplay}]").0.render(&st), "[on]");
    st.autoplay = Some(false);
    assert_eq!(Format::parse("{autoplay:2}").0.render(&st), "o…");
    assert_eq!(Format::parse("{autoplay}").0.render(&st), "off");
    // Through the file: the placeholder is known, so no warning.
    assert!(Config::parse("[format]\nstatus_format = \"{title} {autoplay}\"\n").warnings.is_empty());
}

#[test]
fn state_and_icon_follow_the_player() {
    let mut st = playing();
    for (state, word, icon) in [(PlayState::Paused, "paused", "‖"), (PlayState::Loading, "loading", "…"), (PlayState::Stopped, "stopped", "■")] {
        st.state = state;
        assert_eq!(Format::parse("{state} {icon}").0.render(&st), format!("{word} {icon}"));
    }
}

#[test]
fn widths_cut_with_an_ellipsis_and_braces_can_be_escaped() {
    assert_eq!(show("{title:7}|"), "Instan…|");
    assert_eq!(show("{title:40}|"), "Instant Crush|");
    assert_eq!(show("{{{title}}}"), "{Instant Crush}");
    assert_eq!(show("{ title : 7 }"), "Instan…");
    // Wide characters count by columns.
    let mut st = playing();
    st.song.as_mut().unwrap().title = "夜に駆ける".into();
    assert_eq!(Format::parse("{title:6}").0.render(&st), "夜に…");
}

#[test]
fn what_is_not_there_is_empty_not_an_error() {
    let st = Status::default();
    assert_eq!(Format::parse("{artist}|{title}|{index}|{count}|{percent}|{elapsed}|{source}").0.render(&st), "||||0|0:00|");
    let mut st = playing();
    st.index = None;
    st.count = 0;
    assert_eq!(Format::parse("[{index}/{count}]").0.render(&st), "[/]");
    st.duration = 0.0;
    assert_eq!(Format::parse("{percent} {remaining}").0.render(&st), "0 0:00");
    st.position = 500.0;
    st.duration = 100.0;
    assert_eq!(Format::parse("{percent} {remaining}").0.render(&st), "100 0:00");
}

#[test]
fn mistakes_in_a_format_get_hints_and_still_render() {
    let (f, p) = Format::parse("{artst} - {title}");
    assert_eq!(p, ["unknown name '{artst}' in the format; did you mean {artist}?"]);
    assert_eq!(f.render(&playing()), " - Instant Crush");
    let (_, p) = Format::parse("{banana}");
    assert!(p[0].contains("the names are artist, title, album"), "{p:?}");
    let (f, p) = Format::parse("a { b");
    assert!(p[0].contains("no closing '}'"), "{p:?}");
    assert_eq!(f.render(&playing()), "a { b");
    let (f, p) = Format::parse("a } b");
    assert!(p[0].contains("lone '}'"));
    assert_eq!(f.render(&playing()), "a } b");
    for bad in ["{title:0}", "{title:x}", "{title:}", "{title:-3}"] {
        let (f, p) = Format::parse(bad);
        assert!(p[0].contains("width after ':'"), "{bad}: {p:?}");
        assert_eq!(f.render(&playing()), "Instant Crush", "{bad}");
    }
}

#[test]
fn the_line_is_the_idle_text_when_nothing_plays() {
    let (f, _) = Format::parse("{artist} — {title}");
    assert_eq!(format::line(&f, "", &Status::default()), "");
    assert_eq!(format::line(&f, "♪ idle", &Status::default()), "♪ idle");
    assert_eq!(format::line(&f, "x", &playing()), "Daft Punk — Instant Crush");
    // Stopped with a song still in the status counts as nothing playing.
    let mut st = playing();
    st.state = PlayState::Stopped;
    assert_eq!(format::line(&f, "x", &st), "x");
    // Paused still shows the song.
    st.state = PlayState::Paused;
    assert_eq!(format::line(&f, "x", &st), "Daft Punk — Instant Crush");
}

#[test]
fn the_file_sets_the_format_and_the_idle_text() {
    let cfg = Config::parse("[format]\nstatus_format = \"{icon} {title:20}\"\nstatus_idle = \"nothing\"\n");
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    assert_eq!(cfg.line_format(), "{icon} {title:20}");
    assert_eq!(cfg.status_idle, "nothing");
    // No file, no format: the built-in one, and an empty idle text.
    let cfg = Config::default();
    assert_eq!(cfg.line_format(), format::DEFAULT);
    assert_eq!(cfg.status_idle, "");
    assert_eq!(Format::parse(format::DEFAULT).0.render(&playing()), "Daft Punk — Instant Crush");
}

#[test]
fn a_wrong_placeholder_in_the_file_is_a_warning_on_its_line() {
    let cfg = Config::parse("[format]\n# a comment\nstatus_format = \"{artst}\"\nstatus_idle = 5\n");
    assert_eq!(lines(&cfg), ["line 3: unknown name '{artst}' in the format; did you mean {artist}?", "line 4: 'status_idle' needs text in quotes (got a whole number)"]);
    // The format is kept (unknown names show nothing), so a bar still shows the rest.
    assert_eq!(cfg.line_format(), "{artst}");
}

#[test]
fn status_line_takes_the_files_format_unless_the_command_line_has_one() {
    let cfg = Config::parse("[format]\nstatus_format = \"{title}\"\nstatus_idle = \"zzz\"\n");
    let line = |args: &str| {
        let args: Vec<String> = args.split_whitespace().map(String::from).collect();
        crate::line_with(&crate::cli::parse(&args).unwrap().cmd, &cfg)
    };
    use crate::cli::Command;
    assert_eq!(line("status --line"), Command::Line { format: Some("{title}".into()), idle: Some("zzz".into()) });
    assert_eq!(line("status --format={artist}"), Command::Line { format: Some("{artist}".into()), idle: Some("zzz".into()) });
    let plain = Config::default();
    let cmd = crate::line_with(&crate::cli::parse(&["status".to_string(), "-l".to_string()]).unwrap().cmd, &plain);
    assert_eq!(cmd, Command::Line { format: Some(format::DEFAULT.into()), idle: Some(String::new()) });
}

// ------------------------------------------------------------------- layout and defaults

use crate::config::{Columns, LayoutCfg, Placement};
use crate::tui::sort::{Key, Sort, View};

#[test]
fn no_layout_or_defaults_is_today_s_behaviour() {
    let cfg = Config::parse("");
    assert_eq!(cfg.layout, LayoutCfg::default());
    assert_eq!(cfg.layout.start_screen, 0);
    assert_eq!(cfg.layout.start_view, None);
    assert_eq!(cfg.layout.lyrics, Placement::Auto);
    assert_eq!(cfg.layout.columns, Columns { artist: 3, title: 5, album: 3, show_artist: true, show_album: true });
    assert!(cfg.notifications);
    for v in [View::Playlists, View::Albums, View::Songs, View::Search] {
        assert_eq!(cfg.sorts.get(v), None);
    }
}

#[test]
fn layout_keys() {
    let cfg = Config::parse("[layout]\nstart_screen = \"Albums\"\nstart_view = \"solo\"\nlyrics = \"under\"\nartist_width = 2\ntitle_width = 8\nalbum_width = 1\nshow_album = false\n");
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    assert_eq!(cfg.layout, LayoutCfg { start_screen: 3, start_view: Some("solo".into()), lyrics: Placement::Under, columns: Columns { artist: 2, title: 8, album: 1, show_artist: true, show_album: false } });
    // A screen by number, and "normal" is no view.
    let cfg = Config::parse("[layout]\nstart_screen = 5\nstart_view = \"normal\"\n");
    assert_eq!((cfg.layout.start_screen, cfg.layout.start_view), (4, None));
}

#[test]
fn layout_mistakes_say_what_is_allowed() {
    let cfg = Config::parse("[layout]\nstart_screen = \"playlist\"\nstart_view = \"big\"\nlyrics = \"left\"\nartist_width = 0\ntitle_width = 99\nalbum_width = \"wide\"\nshow_artist = \"no\"\nstart_screen = 6\n");
    let w = lines(&cfg);
    assert!(w[0].contains("'playlist' is not a choice for 'start_screen'; did you mean \"playlists\"?"), "{}", w[0]);
    assert!(w[1].contains("'big' is not a choice for 'start_view'; use \"normal\", \"showcase\", \"solo\""), "{}", w[1]);
    assert!(w[2].contains("'left' is not a choice for 'lyrics'; use \"auto\", \"beside\", \"under\""), "{}", w[2]);
    assert!(w[3].contains("a whole number from 1 to 20 (got 0)"), "{}", w[3]);
    assert!(w[4].contains("(got 99)"), "{}", w[4]);
    assert!(w[5].contains("'album_width' needs a whole number (got a string)"), "{}", w[5]);
    assert!(w[6].contains("'show_artist' needs true or false"), "{}", w[6]);
    // Every bad value left the default alone.
    assert_eq!(cfg.layout, LayoutCfg::default());
}

#[test]
fn default_sorts() {
    let cfg = Config::parse("[defaults]\nsort_playlists = \"name\"\nsort_albums = \"Year desc\"\nsort_songs = \"artist\"\nsort_search = \"duration descending\"\nnotifications = false\n");
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    assert_eq!(cfg.sorts.get(View::Playlists), Some(Sort { key: Key::Name, desc: false }));
    assert_eq!(cfg.sorts.get(View::Albums), Some(Sort { key: Key::Year, desc: true }));
    assert_eq!(cfg.sorts.get(View::Songs), Some(Sort { key: Key::Artist, desc: false }));
    assert_eq!(cfg.sorts.get(View::Search), Some(Sort { key: Key::Duration, desc: true }));
    assert!(!cfg.notifications);
    let cfg = Config::parse("[defaults]\nsort_playlists = \"count desc\"\nsort_albums = \"newest first\"\nsort_songs = \"original\"\n");
    assert_eq!(cfg.sorts.get(View::Playlists), Some(Sort { key: Key::Count, desc: true }));
    assert_eq!(cfg.sorts.get(View::Albums), Some(Sort { key: Key::Original, desc: true }));
    assert_eq!(cfg.sorts.get(View::Songs), None);
}

#[test]
fn bad_default_sorts_list_what_the_view_can_sort_by() {
    let cfg = Config::parse("[defaults]\nsort_playlists = \"artist\"\nsort_albums = \"yaer\"\nsort_songs = \"year\"\nsort_search = 3\nnotifications = \"off\"\n");
    let w = lines(&cfg);
    assert!(w[0].contains("'artist' is not a way to sort playlists; use \"name\", \"song count\""), "{}", w[0]);
    assert!(w[1].contains("'yaer' is not a way to sort albums; did you mean \"year\"?"), "{}", w[1]);
    assert!(w[2].contains("not a way to sort songs; use \"title\", \"artist\", \"album\", \"duration\""), "{}", w[2]);
    assert!(w[3].contains("'sort_search' needs text in quotes"), "{}", w[3]);
    assert!(w[4].contains("'notifications' needs true or false"), "{}", w[4]);
    assert_eq!(cfg.sorts.get(View::Albums), None);
}

// ------------------------------------------------------------------ the default file, the docs

const DOCS: &str = include_str!("../../docs/UZU-CONFIG.md");
const README: &str = include_str!("../../README.md");

/// Takes the "# " off every commented setting (`# name = value`), leaving the prose comments.
fn uncomment(text: &str) -> String {
    text.lines()
        .map(|l| match l.strip_prefix("# ") {
            Some(rest) if rest.split_once(" = ").is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_')) => rest,
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_default_file_is_all_comments_and_changes_nothing() {
    let text = config::default_file();
    let cfg = Config::parse(&text);
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    assert!(cfg.keys.is_default());
    assert_eq!(cfg.theme, Theme::default());
    assert_eq!(cfg.layout, LayoutCfg::default());
    assert_eq!(cfg.status_format, None);
    assert!(reader::parse(&text).entries.is_empty(), "every setting is commented out");
}

#[test]
fn the_default_file_with_every_setting_switched_on_is_valid_and_is_the_defaults() {
    let text = uncomment(&config::default_file());
    let doc = reader::parse(&text);
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    let cfg = Config::parse(&text);
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    // One entry per action, and one per documented setting (theme 7, format 2, layout 8, defaults 5).
    assert_eq!(doc.entries.iter().filter(|e| e.section == "keys").count(), ACTIONS.len());
    assert_eq!(doc.entries.iter().filter(|e| e.section != "keys").count(), 7 + 2 + 8 + 5, "{:?}", doc.entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>());
    assert_eq!(cfg.theme, Theme::default());
    assert_eq!(cfg.layout, LayoutCfg::default());
    assert_eq!(cfg.line_format(), format::DEFAULT);
    assert!(cfg.notifications);
    // The keys are the defaults, so every key does what it did.
    for a in ACTIONS {
        for k in a.keys {
            let c = chord(k);
            assert_eq!(press(&cfg.keys, c.code, KeyModifiers::NONE), Some((chord(a.keys[0]).code, KeyModifiers::NONE)), "{} {k}", a.name);
        }
    }
}

#[test]
fn the_default_file_lists_every_key_every_section_and_every_name() {
    let text = config::default_file();
    for a in ACTIONS {
        assert!(text.contains(&format!("# {} = ", a.name)), "{}", a.name);
    }
    for s in config::SECTIONS {
        assert!(text.contains(&format!("\n[{s}]\n")), "[{s}]");
    }
    for name in format::FIELDS {
        assert!(text.contains(&format!("{{{name}}}")), "{{{name}}}");
    }
}

#[test]
fn the_docs_cover_every_action_name_and_the_sample_has_no_warnings() {
    for a in ACTIONS {
        assert!(DOCS.contains(&format!("| `{}`", a.name)), "docs/UZU-CONFIG.md misses the action {}", a.name);
    }
    for name in format::FIELDS {
        assert!(DOCS.contains(&format!("`{{{name}}}`")), "docs miss {{{name}}}");
    }
    for s in config::SECTIONS {
        assert!(DOCS.contains(&format!("## [{s}]")), "docs miss [{s}]");
    }
    // The sample config at the end of the guide is a real one.
    let tail = DOCS.split("## A themed example").nth(1).expect("the guide has a themed example");
    let sample = tail.split("```toml\n").nth(1).and_then(|t| t.split("```").next()).expect("a toml block");
    let cfg = Config::parse(sample);
    assert!(cfg.warnings.is_empty(), "{:?}", lines(&cfg));
    assert_eq!(cfg.theme.accent, Some(Color::Rgb(0x88, 0xc0, 0xd0)));
    assert!(!cfg.keys.is_default());
    assert_eq!(cfg.layout.start_screen, 1);
    assert_eq!(cfg.layout.lyrics, Placement::Under);
    assert_eq!(cfg.line_format(), "{icon} {artist} — {title:30} [{elapsed}/{duration}]");
    assert!(DOCS.contains("uzu config --default") && DOCS.contains("uzu status --line"));
}

#[test]
fn the_readme_and_the_help_point_to_the_config() {
    assert!(README.contains("docs/UZU-CONFIG.md") && README.contains("uzu.toml"));
    assert!(crate::cli::HELP.contains("config [--default|--path]") && crate::cli::HELP.contains("status --line"));
}
