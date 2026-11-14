//! Draws every screen from an [`App`]. Pure: the same state and clock give the same frame.
//!
//! The frame is the same on all four screens: a rounded box with the player on top (the cover and
//! the song on the left, labelled status fields on the right), the screen tabs written into the
//! rule under it, a context line, the screen's body, and the key hints in the bottom border.

use std::time::Instant;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use ratatui_image::Image;
use uzumaki_ctl::{PlayState, RepeatMode};

use super::app::{App, COMPACT_BELOW, Screen, chrome_top};
use super::cover;
use super::showcase::{self, Layout};
use super::sort::{Key, View};
use super::table::{self, Mark, Row, Table, bold, coral, dim, frame, frame_focused, paint, playing, status, theme, title, warning};
use super::{browse, lyrics, search};
use crate::fmt::{fmt_time, pad_right, plural, repeat_name, text_width, truncate};

/// Width of the status column, its left `│` included.
const STATUS_W: u16 = 34;
/// Narrower than this there is no status column; the fields fold into the compact rows.
const WIDE_FROM: u16 = 70;

/// "11 h 2 min", "38 min", "45 s".
pub fn short_length(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m) = (t / 3600, t / 60 % 60);
    match (h, m) {
        (0, 0) => format!("{t} s"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// What the number column of the playing row shows.
pub fn state_glyph(state: Option<PlayState>) -> &'static str {
    match state {
        Some(PlayState::Playing) => "▶",
        Some(PlayState::Paused | PlayState::Loading) => "‖",
        Some(PlayState::Stopped) | None => "■",
    }
}

/// The player's glyph, word and style for a state.
pub(super) fn state_word(state: PlayState) -> (&'static str, &'static str, Style) {
    match state {
        PlayState::Playing => ("▶", "Playing", playing()),
        PlayState::Paused => ("‖", "Paused", paint(theme().paused)),
        PlayState::Loading => ("…", "Loading", dim()),
        PlayState::Stopped => ("■", "Stopped", dim()),
    }
}

// ------------------------------------------------------------------------------ frame pieces

/// Writes `text` at (`x`, `y`), clipped to `max` columns and to the buffer (never panics).
pub(super) fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, max: usize, style: Style) {
    let area = buf.area;
    if y >= area.height || x >= area.width || max == 0 {
        return;
    }
    let max = max.min((area.width - x) as usize);
    buf.set_stringn(area.x + x, area.y + y, text, max, style);
}

/// A border row: `left`, dashes, `right`.
pub(super) fn hline(buf: &mut Buffer, y: u16, w: u16, (left, right): (&str, &str)) {
    put(buf, 0, y, left, 1, frame());
    if w > 2 {
        put(buf, 1, y, &"─".repeat(w as usize - 2), w as usize - 2, frame());
    }
    put(buf, w - 1, y, right, 1, frame());
}

/// The side borders of a content row.
fn sides(buf: &mut Buffer, y: u16, w: u16) {
    put(buf, 0, y, "│", 1, frame());
    put(buf, w - 1, y, "│", 1, frame());
}

/// `1:12 / 2:24`, or dashes with no song.
pub(super) fn time_text(app: &App, now: Instant) -> String {
    let Some(st) = app.status.as_ref().filter(|st| st.song.is_some()) else { return "--:-- / --:--".into() };
    let dur = if st.duration > 0.0 { fmt_time(st.duration) } else { "?".into() };
    format!("{} / {dur}", fmt_time(app.position(now)))
}

/// The compact right side, from the full text to just the time: `1:12 / 2:24  vol 95%  [r] [z]`.
fn fold_text(app: &App, now: Instant, level: usize) -> String {
    let mut out = time_text(app, now);
    if level == 0 {
        if let Some(st) = &app.status {
            out.push_str(&format!("  vol {}%", (st.volume * 100.0).round() as i64));
        }
    }
    if level <= 1 {
        let (shuffle, repeat) = app.flags();
        let mut flags = Vec::new();
        match repeat {
            RepeatMode::All => flags.push("[r]"),
            RepeatMode::One => flags.push("[1]"),
            RepeatMode::Off => {}
        }
        if shuffle {
            flags.push("[z]");
        }
        if app.autoplay() == Some(true) {
            flags.push("[\u{221e}]");
        }
        if !flags.is_empty() {
            out.push_str(&format!("  {}", flags.join(" ")));
        }
    }
    out
}

/// The longest fold of the status that leaves `min_left` columns of `width` for the song.
fn fold_fit(app: &App, now: Instant, width: usize, min_left: usize) -> String {
    (0..3).map(|l| fold_text(app, now, l)).find(|t| text_width(t) + min_left <= width).unwrap_or_default()
}

/// The unfinished part of the bar: the plain foreground unless `progress_bg` says otherwise.
fn progress_rest() -> Style {
    paint(theme().progress_bg)
}

/// `━━━╸────`: filled, head, rest.
pub fn bar_text(frac: f64, width: usize) -> (String, String) {
    let n = ((frac.clamp(0.0, 1.0) * width as f64) as usize).min(width);
    if n >= width {
        return ("━".repeat(width), String::new());
    }
    (format!("{}╸", "━".repeat(n)), "─".repeat(width - n - 1))
}

/// The progress bar, `width` columns, in the `progress` colour while playing and plain otherwise; with `brackets`
/// it sits inside `[ ]` (and the brackets are part of the width).
pub(super) fn bar_spans(app: &App, width: usize, now: Instant, brackets: bool) -> Vec<Span<'static>> {
    let inner = if brackets { width.saturating_sub(2) } else { width };
    let dur = app.status.as_ref().filter(|s| s.song.is_some()).map_or(0.0, |s| s.duration);
    let mut spans = Vec::new();
    if brackets {
        spans.push(Span::styled("[", frame()));
    }
    if dur <= 0.0 || inner == 0 {
        spans.push(Span::styled("─".repeat(inner), progress_rest()));
    } else {
        let (done, rest) = bar_text(app.position(now) / dur, inner);
        let filled = if app.status.as_ref().is_some_and(|s| s.state == PlayState::Playing) { paint(theme().progress) } else { Style::default() };
        spans.push(Span::styled(done, filled));
        spans.push(Span::styled(rest, progress_rest()));
    }
    if brackets {
        spans.push(Span::styled("]", frame()));
    }
    spans
}

/// Title (bold, coral while playing), artist, album (dim); or "Nothing playing".
pub(super) fn song_lines(app: &App, width: usize) -> [Line<'static>; 3] {
    let st = app.status.as_ref();
    match st.and_then(|st| st.song.as_ref()) {
        Some(song) => {
            let title = if st.is_some_and(|s| s.state == PlayState::Playing) { title().patch(playing()) } else { title() };
            [
                Line::styled(truncate(&song.title, width), title),
                Line::from(truncate(&song.artist, width)),
                Line::styled(truncate(&song.album, width), dim()),
            ]
        }
        None => [
            Line::styled(if st.is_none() { "Connecting…" } else { "Nothing playing" }, dim()),
            Line::default(),
            Line::default(),
        ],
    }
}

/// The status column's six lines, `width` columns each.
fn status_lines(app: &App, width: usize, now: Instant) -> Vec<Line<'static>> {
    let label = |l: &str| Span::raw(pad_right(l, 9));
    let (shuffle, repeat) = app.flags();
    let volume = app.status.as_ref().map_or("--".to_string(), |st| format!("{}%", (st.volume * 100.0).round() as i64));
    let status = match app.status.as_ref() {
        Some(st) => {
            let (glyph, word, style) = state_word(st.state);
            Span::styled(format!("{glyph} {word}"), style)
        }
        None => Span::styled("… Connecting", dim()),
    };
    let rows = app.rows();
    let queue = if rows.is_empty() {
        "empty".to_string()
    } else {
        {
        let own = rows.iter().filter(|r| r.kind == table::RowKind::Song);
        format!("{} · {}", plural(own.clone().count(), "song"), short_length(own.map(|r| r.song.duration).sum()))
    }
    };
    let bar = bar_spans(app, width, now, true);
    vec![
        match app.autoplay() {
            // Only where the app offers it (an older app, local files and stations don't).
            Some(on) => Line::from(vec![label("Volume:"), Span::raw(pad_right(&volume, 6)), Span::raw(if on { "AutoPlay: \u{221e}" } else { "AutoPlay: off" })]),
            None => Line::from(vec![label("Volume:"), Span::raw(volume)]),
        },
        Line::from(vec![label("Status:"), status]),
        Line::from(vec![label("Repeat:"), Span::raw(pad_right(repeat_name(repeat), 8)), Span::raw(format!("Shuffle: {}", if shuffle { "on" } else { "off" }))]),
        Line::from(vec![label("Time:"), Span::raw(time_text(app, now))]),
        Line::from(bar),
        Line::from(vec![label("Queue:"), Span::raw(truncate(&queue, width.saturating_sub(9)))]),
    ]
}

/// The two compact player rows: song and folded status, then the bar. `width` columns.
fn compact_lines(app: &App, width: usize, now: Instant) -> [Line<'static>; 2] {
    let right = fold_fit(app, now, width, 10);
    let left_w = width.saturating_sub(text_width(&right) + 2);
    let st = app.status.as_ref();
    let mut spans = Vec::new();
    match st.and_then(|st| st.song.as_ref().map(|s| (st.state, s))) {
        Some((state, song)) => {
            let (glyph, _, gstyle) = state_word(state);
            let title = if state == PlayState::Playing { title().patch(playing()) } else { title() };
            let avail = left_w.saturating_sub(2);
            let name = truncate(&song.title, avail);
            let room = avail.saturating_sub(text_width(&name) + 3);
            spans.push(Span::styled(format!("{glyph} "), gstyle));
            spans.push(Span::styled(name, title));
            if !song.artist.is_empty() && room > 0 {
                spans.push(Span::raw(format!(" — {}", truncate(&song.artist, room))));
            }
        }
        None => spans.push(Span::styled(if st.is_none() { "Connecting…" } else { "Nothing playing" }, dim())),
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used + text_width(&right)))));
    spans.push(Span::raw(right));
    [Line::from(spans), Line::from(bar_spans(app, width, now, false))]
}

/// The player's rows between the top border and the tab rule (the status column is apart).
/// Wide: the song on the left (the status column is drawn apart). Narrow and tall: the song,
/// the folded status and the bar. Short: the two compact rows.
fn player_left(app: &App, width: usize, now: Instant, compact: bool, wide: bool) -> Vec<Line<'static>> {
    if compact {
        return compact_lines(app, width, now).into();
    }
    let hint = Line::styled("uzumaki · ? help", dim());
    let [title, artist, album] = song_lines(app, width);
    if wide {
        return vec![Line::default(), title, artist, album, Line::default(), hint];
    }
    // Under the song: the folded status behind its state glyph, then the bar.
    let st = app.status.as_ref();
    let glyph = st.map_or(("…", dim()), |st| {
        let (g, _, s) = state_word(st.state);
        (g, s)
    });
    let text = fold_fit(app, now, width, 2);
    let status = Line::from(vec![Span::styled(format!("{} ", glyph.0), glyph.1), Span::raw(truncate(&text, width.saturating_sub(2)))]);
    vec![title, artist, album, status, Line::from(bar_spans(app, width, now, false)), hint]
}

/// The rule under the player with the screen tabs written into it. The current tab is bold
/// coral, the others dim. With a status column, `sep` is where its `│` meets the rule.
fn tab_rule(buf: &mut Buffer, y: u16, w: u16, sep: Option<u16>, current: Screen) {
    hline(buf, y, w, ("├", "┤"));
    if let Some(x) = sep {
        put(buf, x, y, "┴", 1, frame());
    }
    let room = (w as usize).saturating_sub(4);
    let full: Vec<String> = Screen::ALL.iter().map(|s| format!(" {} {} ", s.number(), s.name())).collect();
    let total = |tabs: &[String]| tabs.iter().map(|t| text_width(t)).sum::<usize>() + 2 * (tabs.len() - 1);
    // Too wide: the other tabs shrink to their numbers.
    let tabs: Vec<String> = if total(&full) <= room {
        full
    } else {
        Screen::ALL.iter().map(|&s| if s == current { format!(" {} {} ", s.number(), s.name()) } else { format!(" {} ", s.number()) }).collect()
    };
    let mut x = 2usize;
    for (tab, &screen) in tabs.iter().zip(Screen::ALL.iter()) {
        if x >= w as usize - 1 {
            break;
        }
        let style = if screen == current { coral().add_modifier(Modifier::BOLD) } else { Style::default() };
        // The spaces around the name are plain, so the rule's dashes stop short of the text.
        put(buf, x as u16, y, tab, w as usize - 2 - x, Style::default());
        put(buf, x as u16 + 1, y, tab.trim(), w as usize - 2 - x, style);
        x += text_width(tab) + 2;
    }
}

/// The key hints of the screen (or of the typing mode), most important first.
fn hints(app: &App) -> &'static [&'static str] {
    if app.screen == Screen::Search && app.search.typing {
        return &["Enter search", "Esc cancel", "Ctrl+U clear", "Backspace delete"];
    }
    if app.filter_text().is_some() {
        return &["Enter keep", "Esc clear", "Backspace delete"];
    }
    match app.screen {
        Screen::Queue => &["Space play/pause", "n/p next/prev", "Enter play", "d remove", "J/K move", "/ filter", "F view", "q quit"],
        Screen::Playlists | Screen::Albums => &["Space play/pause", "n/p next/prev", "Enter open/play", "Tab pane", "P play all", "/ filter", ", sort", "R reload", "Esc back", "q quit"],
        Screen::Search => &["Space play/pause", "n/p next/prev", "Enter play", "/ new query", "a add", "N next", ", sort", "Esc back", "q quit"],
        Screen::Lyrics => &["Space play/pause", "n/p next/prev", "w words/lines", "j/k scroll", "g follow", "Esc back", "q quit"],
    }
}

/// The hints joined by ` · `, whole items only, in `width` columns.
pub(super) fn hint_text(items: &[&str], width: usize) -> String {
    let mut out = String::new();
    for item in items {
        let next = if out.is_empty() { item.to_string() } else { format!("{out} · {item}") };
        if text_width(&next) > width {
            if out.is_empty() {
                out = truncate(item, width);
            }
            break;
        }
        out = next;
    }
    out
}

/// What the line under the tabs says: what is typed, or the app being away, or a message
/// (they replace the usual text), else the screen's own line.
fn context_line(app: &App, width: usize, now: Instant) -> Line<'static> {
    if let Some(text) = app.filter_text() {
        return Line::from(truncate(&format!("/{text}▏"), width));
    }
    if app.screen == Screen::Search && app.search.typing {
        return Line::from(truncate(&format!("Search: {}▏", app.search.input), width));
    }
    if app.away {
        return Line::styled(truncate("Uzumaki went away, retrying…", width), warning());
    }
    if let Some(m) = app.message(now) {
        return Line::styled(truncate(m, width), app.message_style(now));
    }
    let (mut text, filter) = match app.screen {
        Screen::Queue => (if app.queue.source.is_empty() { "Queue".to_string() } else { format!("Playing from: {}", app.queue.source) }, app.filter.as_str()),
        Screen::Playlists | Screen::Albums => {
            let b = app.browse(app.screen);
            (b.title(), b.focused_filter())
        }
        Screen::Search => (app.search.title(), ""),
        Screen::Lyrics => (lyrics_title(app), ""),
    };
    if !filter.is_empty() {
        text.push_str(&format!("  /{filter}"));
    }
    Line::from(truncate(&text, width))
}

/// The context line of the lyrics screen: the song and how it is shown.
fn lyrics_title(app: &App) -> String {
    let l = &app.lyrics;
    let song = app.status.as_ref().filter(|st| st.state != PlayState::Stopped).and_then(|st| st.song.as_ref());
    let Some(song) = song else { return "Lyrics".into() };
    let mode = match l.status {
        _ if l.song.as_deref() != Some(song.id.as_str()) => "",
        uzumaki_ctl::LyricsStatus::Plain => " · not time-synced",
        uzumaki_ctl::LyricsStatus::Synced if l.manual.is_some() => " · scrolled, g follows",
        uzumaki_ctl::LyricsStatus::Synced if l.words() && l.lines.iter().any(|x| !x.words.is_empty()) => " · word by word",
        uzumaki_ctl::LyricsStatus::Synced => " · line by line",
        _ => "",
    };
    format!("Lyrics: {} — {}{mode}", song.title, song.artist)
}

/// The lyrics screen's body: the lines with the current one in the middle, or why there are none.
pub(super) fn draw_lyrics(f: &mut Frame, area: Rect, app: &App, now: Instant) {
    let l = &app.lyrics;
    let playing = playing_song(app);
    let current = playing.is_some() && l.song.as_deref() == playing && !l.lines.is_empty();
    if !current {
        let signed_in = app.status.as_ref().is_none_or(|st| st.signed_in);
        // The song just changed (or the screen just opened): its lyrics are being asked for.
        let text = if playing.is_some() && l.song.as_deref() != playing && !l.unsupported { "Loading lyrics…" } else { lyrics::empty_text(l, playing.is_some(), signed_in) };
        return lyrics::draw_message(f, area, text);
    }
    let (rows, _, top) = app.lyrics_layout(now);
    lyrics::draw(f, area, &rows, top);
}

/// Solo's dim last-row hint when lyrics were asked for (`l`) but aren't showing: why.
pub(super) fn solo_lyrics_hint(app: &App) -> Option<String> {
    if !app.solo() || !app.solo_lyrics {
        return None;
    }
    let playing = playing_song(app)?;
    let l = &app.lyrics;
    let plain = l.song.as_deref() == Some(playing) && l.status == uzumaki_ctl::LyricsStatus::Plain && !l.lines.is_empty();
    if app.solo_lyrics_shown() {
        // Plain lyrics on screen: the small label, as on screen 5's context line (and how to scroll long ones).
        let label = if app.solo_plain_scrolls() { lyrics::NOT_SYNCED_SCROLL } else { lyrics::NOT_SYNCED };
        return (plain && app.solo_geometry().is_some_and(|g| g.lyrics.is_some())).then(|| label.to_string());
    }
    let signed_in = app.status.as_ref().is_none_or(|st| st.signed_in);
    let text = if l.song.as_deref() != Some(playing) && !l.unsupported { "Loading lyrics…" } else { lyrics::empty_text(l, true, signed_in) };
    Some(text.to_string())
}

/// Keys that work on every screen.
const HELP_COMMON: &[(&str, &str)] = &[
    ("Space", "play / pause"),
    ("n  p", "next / previous song"),
    ("f  b", "seek +10 s / -10 s"),
    ("+  -", "volume +5% / -5%"),
    ("s  r", "shuffle / repeat (off, all, one)"),
    ("A", "AutoPlay (\u{221e}) on / off"),
    ("1 2 3 4 5", "queue/playlists/search/albums/lyrics"),
    ("i", "show / hide the cover"),
    ("F", "view: normal, showcase, solo (Esc: normal)"),
    ("l", "in solo: show / hide lyrics (w words/lines)"),
    ("?", "this help"),
    ("q  Ctrl+C", "quit"),
];

fn help_for(screen: Screen) -> &'static [(&'static str, &'static str)] {
    match screen {
        Screen::Queue => &[
            ("Left  Right", "seek -10 s / +10 s"),
            ("Up Down  j k", "move the selection"),
            ("PgUp PgDn", "move a page"),
            ("Home End  g G", "first Up Next (again: top of history) / last row"),
            ("o", "select the playing song"),
            ("Enter", "play the selected song"),
            ("d  Delete", "remove it from Up Next"),
            ("J  K", "move it down / up"),
            ("/", "filter by title or artist"),
            ("Esc", "clear the filter, leave showcase/solo, or quit"),
        ],
        Screen::Playlists | Screen::Albums => &[
            ("Up Down  j k", "move in the focused pane"),
            ("PgUp PgDn  g G", "page / first / last"),
            ("Enter", "open the list / play from the song"),
            ("Left Right  h l", "switch pane (Tab too)"),
            ("P", "play the whole list"),
            ("/", "filter the focused pane"),
            (",", "sort the focused pane (a menu follows; o original, O newest first)"),
            ("R", "reload the list"),
            ("Esc", "clear filter, leave pane, go to queue"),
        ],
        Screen::Search => &[
            ("/", "type a new query (Enter runs it)"),
            (",", "sort the results (a menu follows; o original, O newest first)"),
            ("Ctrl+U", "clear the query while typing"),
            ("Up Down  j k", "move the selection"),
            ("Enter", "play the song now"),
            ("a", "add it to the queue"),
            ("N", "play it next"),
            ("Esc", "stop typing, or go to the queue"),
        ],
        Screen::Lyrics => &[
            ("w", "highlight word by word / line by line"),
            ("j k  Up Down", "scroll by hand (the wheel too)"),
            ("PgUp PgDn", "scroll a page"),
            ("g  Home", "follow the song again"),
            ("G  End", "scroll to the end"),
            ("Left Right", "seek -10 s / +10 s"),
            ("Esc", "go to the queue"),
        ],
    }
}

/// Where the help box goes: centred, 52 columns wide.
pub(super) fn help_rect(area: Rect, screen: Screen) -> Rect {
    let rows = help_for(screen).len() + HELP_COMMON.len();
    let w = 52.min(area.width);
    let h = (rows as u16 + 2).min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn help_overlay(f: &mut Frame, area: Rect, screen: Screen, app: &App) {
    let keys: Vec<_> = help_for(screen).iter().chain(HELP_COMMON).collect();
    let r = help_rect(area, screen);
    let lines: Vec<Line> = keys.iter().map(|(k, d)| Line::from(vec![Span::styled(pad_right(&truncate(&app.config.keys.relabel(k), 15), 16), coral()), Span::raw(*d)])).collect();
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(format!(" Keys: {} ", screen.name())).border_style(frame_focused())), r);
}

/// The sort popup, a rounded box at the bottom right of the body, above the bottom border.
fn sort_popup(f: &mut Frame, area: Rect, app: &App, view: View) {
    let current = app.sort_of(view);
    let mut rows: Vec<(String, String, bool)> = vec![("o/O".into(), "original · O: newest first".into(), current.is_none_or(|s| s.key == Key::Original))];
    for &(c, key) in view.modes() {
        let on = current.is_some_and(|s| s.key == key);
        rows.push((format!("{c}/{}", c.to_ascii_uppercase()), key.label().to_string(), on));
    }
    let mut lines: Vec<Line> = rows.iter().map(|(k, label, on)| Line::from(vec![Span::styled(pad_right(k, 5), coral()), Span::styled(label.clone(), if *on { bold() } else { Style::default() })])).collect();
    lines.push(Line::styled("a/z up · A/Z down", dim()));
    let inner = lines.iter().map(|l| l.width()).max().unwrap_or(0) + 2;
    // Never taller than the body, and a body too short to fit it just shows its top.
    let body_h = area.height.saturating_sub(1).saturating_sub(chrome_top(area.height));
    let (w, h) = (((inner + 2) as u16).min(area.width.saturating_sub(2)), ((lines.len() + 2) as u16).min(body_h));
    if w < 4 || h < 3 {
        return;
    }
    let r = Rect { x: area.width - 1 - w, y: area.height - 1 - h, width: w, height: h };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(frame_focused()).title(format!(" {} ", view.title()));
    f.render_widget(Clear, r);
    let content = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(lines).block(Block::default().padding(Padding::horizontal(1))), content);
}

/// The queue's table in `area` (its header row, then the songs; the scrollbar goes right of it).
pub(super) fn draw_queue(f: &mut Frame, area: Rect, app: &App) {
    let rows = app.rows();
    let vis = app.visible();
    let shown: Vec<Row> = vis.iter().map(|&i| Row { number: rows[i].number, song: rows[i].song, playing: rows[i].playing, kind: rows[i].kind }).collect();
    let empty = if rows.is_empty() {
        if app.status.is_none() { "Loading the queue…" } else { "Nothing in the queue" }
    } else if !app.filter.is_empty() {
        "No song matches the filter"
    } else {
        "Nothing up next"
    };
    let selected = vis.iter().position(|&i| i == app.cursor).map(|p| (p, Mark::Focus));
    table::draw(f, area, &Table { rows: &shown, offset: app.offset, selected, num_w: app.num_width(), empty, glyph: state_glyph(app.status.as_ref().map(|st| st.state)) });
}

/// The popups drawn over every view.
fn overlays(f: &mut Frame, app: &App) {
    let area = f.area();
    if let Some(view) = app.sorting {
        sort_popup(f, area, app, view);
    }
    if app.help {
        help_overlay(f, area, app.screen, app);
    }
}

pub fn draw(f: &mut Frame, app: &App, now: Instant) {
    table::set_look(app.config.theme, app.config.layout.columns);
    let area = f.area();
    let (w, h) = (area.width, area.height);
    // The screenshot views (`F`), on the queue screen; the showcase needs room, else this one.
    if app.screen == Screen::Queue && app.layout != Layout::Normal {
        let caps = app.cover.caps();
        if app.layout == Layout::Solo {
            showcase::draw_solo(f, app, now, &app.solo_geometry().unwrap_or_else(|| showcase::solo(w, h, caps)));
            return overlays(f, app);
        }
        if let Some(g) = showcase::showcase(w, h, caps) {
            showcase::draw_showcase(f, app, now, &g);
            return overlays(f, app);
        }
    }
    if w < 2 || h < 2 {
        return;
    }
    let compact = h < COMPACT_BELOW;
    let sep = (!compact && w >= WIDE_FROM).then(|| w - 1 - STATUS_W);
    let prows: u16 = if compact { 2 } else { 6 };
    let chrome = chrome_top(h);
    let inner = w as usize - 2;
    let buf = f.buffer_mut();

    hline(buf, 0, w, ("╭", "╮"));
    if let Some(x) = sep {
        put(buf, x, 0, "┬", 1, frame());
    }
    for y in 1..=prows {
        sides(buf, y, w);
        if let Some(x) = sep {
            put(buf, x, y, "│", 1, frame());
        }
    }
    tab_rule(buf, prows + 1, w, sep, app.screen);
    sides(buf, prows + 2, w);
    hline(buf, prows + 3, w, ("├", "┤"));
    for y in chrome..h {
        sides(buf, y, w);
    }

    // The player, the context line, then the body; each paints its own lines. The cover, when
    // it shows, takes the left of the song lines.
    let left_w = sep.map_or(inner.saturating_sub(4), |x| x as usize - 4);
    let cover_cols = cover_cols(app, compact, left_w);
    let (text_x, left_w) = match cover_cols {
        Some(cols) => (3 + cols + cover::GAP, left_w - usize::from(cols + cover::GAP)),
        None => (3, left_w),
    };
    if let Some(cols) = cover_cols {
        let slot = Rect { x: 3, y: 1, width: cols, height: cover::ROWS };
        // Under the help box a picture would show through (sixel, iTerm2): leave it out then.
        if !(app.help && help_rect(area, app.screen).intersects(slot)) {
            draw_cover(f, app, slot);
        }
    }
    let lines = player_left(app, left_w, now, compact, sep.is_some());
    for (i, line) in lines.into_iter().enumerate() {
        render_line(f, line, Rect { x: text_x, y: 1 + i as u16, width: left_w as u16, height: 1 });
    }
    if let Some(x) = sep {
        for (i, line) in status_lines(app, STATUS_W as usize - 3, now).into_iter().enumerate() {
            render_line(f, line, Rect { x: x + 2, y: 1 + i as u16, width: STATUS_W - 3, height: 1 });
        }
    }
    // `status_bg` fills the whole line, so it is the paragraph's style and not the text's.
    let status_row = Rect { x: 2, y: prows + 2, width: (inner.saturating_sub(2)) as u16, height: 1 };
    if status_row.width > 0 && status_row.y < f.area().height {
        f.render_widget(Paragraph::new(context_line(app, inner.saturating_sub(2), now)).style(status()), status_row);
    }

    // The body: a left pad, the content, a gap and a column for the scrollbar.
    let body_h = h.saturating_sub(1).saturating_sub(chrome);
    let content = Rect { x: 2, y: chrome, width: w.saturating_sub(5), height: body_h };
    if content.width > 0 && content.height > 0 {
        let playing = app.playing_id();
        match app.screen {
            Screen::Queue => draw_queue(f, content, app),
            Screen::Playlists | Screen::Albums => browse::draw(f, content, app.browse(app.screen), playing),
            Screen::Search => search::draw(f, content, &app.search, playing),
            Screen::Lyrics => draw_lyrics(f, content, app, now),
        }
    }

    // The bottom border carries the key hints (drawn last: it owns the last row).
    let buf = f.buffer_mut();
    hline(buf, h - 1, w, ("╰", "╯"));
    let text = if app.config.keys.is_default() || app.filter_text().is_some() || (app.screen == Screen::Search && app.search.typing) {
        hint_text(hints(app), (w as usize).saturating_sub(6))
    } else {
        let items: Vec<String> = hints(app).iter().map(|h| app.config.keys.relabel_hint(h)).collect();
        hint_text(&items.iter().map(String::as_str).collect::<Vec<_>>(), (w as usize).saturating_sub(6))
    };
    if !text.is_empty() {
        put(buf, 2, h - 1, &format!(" {text} "), w as usize - 4, dim());
    }
    overlays(f, app);
}

/// The playing song's id (not when stopped).
pub(super) fn playing_song(app: &App) -> Option<&str> {
    app.status.as_ref().filter(|st| st.state != PlayState::Stopped).and_then(|st| st.song.as_ref()).map(|s| s.id.as_str())
}

/// The cover's columns, when it shows: turned on, a song playing, the full player (not the
/// compact one), and room left for the song lines. Otherwise the player is laid out as without it.
fn cover_cols(app: &App, compact: bool, left_w: usize) -> Option<u16> {
    let cols = app.cover.cols()?;
    let fits = left_w >= usize::from(cols + cover::GAP) + cover::MIN_TEXT;
    (!compact && fits && playing_song(app).is_some()).then_some(cols)
}

/// The cover in `slot`, centred; a note while it loads (or when the song has none).
fn draw_cover(f: &mut Frame, app: &App, slot: Rect) {
    match playing_song(app).and_then(|song| app.cover.image(song, ratatui::layout::Size::new(slot.width, slot.height))) {
        Some(p) => {
            let size = p.size();
            let (w, h) = (size.width.min(slot.width), size.height.min(slot.height));
            let r = Rect { x: slot.x + (slot.width - w) / 2, y: slot.y + (slot.height - h) / 2, width: w, height: h };
            f.render_widget(Image::new(p), r);
        }
        None => {
            let r = Rect { x: slot.x, y: slot.y + slot.height / 2, width: slot.width, height: 1 };
            f.render_widget(Paragraph::new(Line::styled("♪", frame())).centered(), r);
        }
    }
}

pub(super) fn render_line(f: &mut Frame, line: Line<'static>, r: Rect) {
    if r.width > 0 && r.y < f.area().height {
        f.render_widget(Paragraph::new(line), r);
    }
}
