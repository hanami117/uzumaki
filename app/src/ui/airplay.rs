//! The AirPlay picker: a button in the player bar and in Now Playing, and its popover.
//!
//! The popover lists this computer and the AirPlay speakers on the network (found only while it
//! is open). One of them has the tick: where the audio goes. Picking a speaker shows a spinner
//! until it plays; the playing speaker gets its own volume slider; a speaker that wants a
//! password or a PIN gets a field. Tab moves through the rows, Enter picks, Esc closes.
//!
//! The button draws nothing that moves and reads only `State::airplay`: idle, it costs the view
//! one more icon.

use rune::{Cursor, Cx, El, Icon, Key, KeyEvent, LoopSpec, Phase, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, text, text_input};
use uzumaki_airplay::discover::{Kind, Output};
use uzumaki_airplay::{Cast, Need};

use super::icons;
use super::widgets::{slider, small_button};
use crate::state::State;
use crate::symbols::{self, Sym};
use crate::state::airplay::Ask;
use crate::theme::*;

/// The popover's width.
const W: f32 = 300.0;
const ROW_H: f32 = 44.0;
/// More speakers than this (a large building's network) are cut, with a note.
const MAX_ROWS: usize = 12;

macro_rules! line_icon {
    ($name:ident, $id:literal, $d:literal) => {
        static $name: Icon = Icon {
            name: $id,
            view: 24.0,
            body: concat!(
                r#"<path fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" d=""#,
                $d,
                r#""/>"#
            ),
        };
    };
}

// The audio-output glyph: a triangle under two rings (rings round (12, 11), open below).
line_icon!(AIRPLAY, "cad-airplay", "M12 15.5l4.2 5.5H7.8zM8.82 14.18a4.5 4.5 0 1 1 6.36 0M5.99 17.01a8.5 8.5 0 1 1 12.02 0");
line_icon!(COMPUTER, "cad-out-computer", "M5 5h14a1 1 0 0 1 1 1v9.5H4V6a1 1 0 0 1 1-1zM2 19h20");
line_icon!(DESKTOP, "cad-out-desktop", "M4 4.5h16a1 1 0 0 1 1 1V15a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5.5a1 1 0 0 1 1-1zM9 20h6M12 16v4");
line_icon!(HOMEPOD, "cad-out-homepod", "M12 3.5c4.2 0 6.5 3.4 6.5 8.5s-2.3 8.5-6.5 8.5S5.5 17.1 5.5 12 7.8 3.5 12 3.5zM9 6.6c.9-.5 1.9-.8 3-.8s2.1.3 3 .8");
line_icon!(HOMEPOD_MINI, "cad-out-homepod-mini", "M12 6c4.1 0 7.2 2.7 7.2 6.6S16.1 19 12 19s-7.2-2.5-7.2-6.4S7.9 6 12 6zM8.6 8.700c1-.6 2.2-.9 3.4-.9s2.4.3 3.4.9");
line_icon!(APPLE_TV, "cad-out-appletv", "M5.5 8h13A2.5 2.5 0 0 1 21 10.500v3A2.5 2.5 0 0 1 18.5 16h-13A2.5 2.5 0 0 1 3 13.500v-3A2.5 2.5 0 0 1 5.5 8zM17.5 12h.01");
line_icon!(TV, "cad-out-tv", "M4 4.5h16a1 1 0 0 1 1 1V15a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5.500a1 1 0 0 1 1-1zM7.5 20h9");
line_icon!(SPEAKER, "cad-out-speaker", "M7.5 3.500h9a1 1 0 0 1 1 1v15a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-15a1 1 0 0 1 1-1zM12 17.500a3.2 3.2 0 1 0 0-6.4 3.2 3.2 0 0 0 0 6.400zM12 7.300h.01");

/// A device's glyph: its SF Symbol on the Mac (a stereo pair of HomePods gets the pair), else its line icon.
#[derive(Clone, Copy)]
struct Glyph {
    sym: Sym,
    line: &'static Icon,
}

fn kind_glyph(kind: Kind, pair: bool) -> Glyph {
    let (sym, line) = match kind {
        Kind::HomePod if pair => (Sym::HomePodPair, &HOMEPOD),
        Kind::HomePodMini if pair => (Sym::HomePodPair, &HOMEPOD_MINI),
        Kind::HomePod => (Sym::HomePod, &HOMEPOD),
        Kind::HomePodMini => (Sym::HomePodMini, &HOMEPOD_MINI),
        Kind::AppleTv => (Sym::AppleTv, &APPLE_TV),
        Kind::Tv => (Sym::Tv, &TV),
        Kind::Speaker => (Sym::Speaker, &SPEAKER),
        Kind::Computer => (Sym::Computer, &COMPUTER),
    };
    Glyph { sym, line }
}

/// The glyph in `color`: the cached symbol, or the line icon (off the Mac, or before the cache is warm).
fn glyph_el(g: Glyph, color: rune::Color, line_size: f32) -> El {
    match symbols::get(g.sym, color) {
        Some(px) => el().size(symbols::CANVAS, symbols::CANVAS).pixels_fit(&px, rune::SurfaceFit::Fit),
        None => el().size(symbols::CANVAS, symbols::CANVAS).center().child(icon(g.line, line_size).color(color)),
    }
}

/// "This Mac" on a Mac, "This Computer" elsewhere (as Apple's picker says it).
pub fn this_computer() -> &'static str {
    if cfg!(target_os = "macos") { "This Mac" } else { "This Computer" }
}

/// The picker's view state.
#[derive(Default)]
pub struct Picker {
    pub open: bool,
    /// The key of the button it was opened from (the popover sits above it).
    anchor: &'static str,
    /// The password or PIN being typed.
    pub field: TextEdit,
    /// The question the field was last cleared and focused for.
    asked: Option<Ask>,
}

/// Opens the picker above the button `anchor` (or closes it, on a second press) and starts or
/// stops looking for speakers.
pub fn toggle(s: &mut State, anchor: &'static str) {
    if !s.airplay.available {
        return;
    }
    if s.ui.airplay.open && s.ui.airplay.anchor == anchor {
        return close(s);
    }
    s.ui.volume_open = false;
    s.ui.airplay.open = true;
    s.ui.airplay.anchor = anchor;
    s.airplay_browse(true);
}

pub fn close(s: &mut State) {
    s.ui.airplay.open = false;
    s.ui.airplay.field.focused = false;
    s.airplay_browse(false);
}

/// The field of a question that just arrived gets the keyboard (and starts empty). Returns
/// whether the view must run.
pub fn poll(s: &mut State) -> bool {
    // Closed by a change of page (`UiState::route_changed`): stop looking for speakers too.
    if !s.ui.airplay.open && s.airplay.browsing {
        s.airplay_browse(false);
    }
    if s.ui.airplay.asked == s.airplay.ask {
        return false;
    }
    s.ui.airplay.asked = s.airplay.ask.clone();
    s.ui.airplay.field = TextEdit::default();
    s.ui.airplay.field.focused = s.airplay.ask.is_some() && s.ui.airplay.open;
    true
}

/// The field, for the caret's blink (`caret.rs`).
pub(super) fn edit(p: &Picker) -> &TextEdit {
    &p.field
}

/// The AirPlay button: coral while a speaker plays (or connects) and while the picker is open
/// from it. `idle` is its colour otherwise (the bar's and Now Playing's differ). None when this
/// install has no cliairplay helper (Linux for now): no dead button.
pub fn button(s: &State, key: &'static str, idle: rune::Color) -> Option<El> {
    if !s.airplay.available {
        return None;
    }
    let lit = s.airplay.active() || (s.ui.airplay.open && s.ui.airplay.anchor == key);
    let label = match &s.airplay.cast {
        Cast::Off => "AirPlay".to_string(),
        Cast::Connecting { name, .. } => format!("AirPlay: connecting to {name}"),
        Cast::On { name, .. } => format!("AirPlay: playing on {name}"),
    };
    Some(el().key(key).role("button").label(label).focusable(true).size(36.0, 36.0).radius(18.0).center()
        .hover(ov(0.08)).press(ov(0.12)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| toggle(s, key))
        .child(glyph_el(Glyph { sym: Sym::AirPlay, line: &AIRPLAY }, if lit { PAL.accent } else { idle }, 18.0)))
}

/// What a row shows at its right end.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    None,
    Tick,
    Spinner,
}

fn output_row(key: String, glyph: Glyph, name: &str, sub: Option<String>, mark: Mark) -> El {
    let on = mark == Mark::Tick;
    let mut words = col().grow(1.0).min_w(0.0).gap(1.0).justify_center()
        .child(text(name.to_string()).font(TextStyle::new(13.5, if on { 600.0 } else { 400.0 })).color(PAL.text).truncate());
    if let Some(sub) = sub {
        words = words.child(text(sub).font(SMALL12).color(PAL.muted).truncate());
    }
    let mark = match mark {
        Mark::None => el().size(18.0, 18.0),
        Mark::Tick => el().size(18.0, 18.0).center().child(icon(&icons::CHECK, 16.0).color(PAL.accent)),
        Mark::Spinner => el().size(18.0, 18.0).center().child(icon(&icons::SWIRL, 16.0).color(PAL.accent).looping(LoopSpec::rotation(1.6))),
    };
    row().key(key).role("menuitemradio").label(name.to_string()).focusable(true).w_full().h(ROW_H).px(10.0).gap(10.0).center_y().radius(8.0)
        .hover(ov(0.09)).press(ov(0.14)).cursor(Cursor::Pointer)
        // As Apple's picker: the device in a round badge, filled with the accent where the sound is.
        .child(
            el().size(32.0, 32.0).radius(16.0).center().fill(if on { PAL.accent } else { ov(0.10) })
                .child(glyph_el(glyph, if on { rune::Color::WHITE } else { PAL.text }, 21.0)),
        )
        .child(words)
        .child(mark)
}

/// The playing speaker's own volume, under its row.
fn volume_row(s: &State) -> El {
    let v = s.airplay.volume();
    row().key("airplay-volume-row").w_full().h(30.0).pl(48.0).pr(12.0).gap(10.0).center_y()
        .child(icon(icons::volume(f64::from(v) / 100.0), 15.0).color(PAL.muted))
        .child(el().grow(1.0).child(slider("airplay-volume", f32::from(v) / 100.0, PAL.text2, |s, f, phase, cancelled, _tx| {
            if !(phase == Phase::Up && cancelled) {
                s.airplay_volume((f * 100.0).round() as u8, phase == Phase::Up);
            }
        })))
        .child(row().w(26.0).justify_end().child(text(v.to_string()).font(SMALL12.features("'tnum' 1")).color(PAL.muted)))
}

fn submit(s: &mut State) {
    let text = s.ui.airplay.field.text.clone();
    s.airplay_answer(&text);
}

/// The password or PIN field, under the row of the speaker that asked.
fn ask_rows(s: &State, ask: &Ask) -> El {
    let (hint, placeholder) = match (ask.need, ask.wrong) {
        (Need::Pin, _) => (format!("Enter the code that “{}” shows on its screen.", ask.name), "Code"),
        (Need::Password, false) => (format!("“{}” needs its AirPlay password.", ask.name), "Password"),
        (Need::Password, true) => ("That password was not accepted. Try again.".to_string(), "Password"),
    };
    let input = text_input(&s.ui.airplay.field, TextStyle::new(14.0, 400.0))
        .key("airplay-code").grow(1.0).single_line().color(PAL.text).placeholder(placeholder, PAL.dim)
        .on_edit(|s: &mut State| &mut s.ui.airplay.field)
        .on_submit(|s: &mut State, _tx: &mut Tx| submit(s))
        // The field would swallow Esc (it just drops focus), so close from here.
        .on_input_key(|s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
            if super::keys::track_focus(s, ev, tx) {
                return true;
            }
            if ev.key == Key::Escape {
                close(s);
                tx.blur();
                return true;
            }
            false
        });
    let input = if ask.need == Need::Password { input.secure() } else { input };
    let input = super::caret::ink(input, s);
    col().key("airplay-ask").w_full().pl(48.0).pr(10.0).pb(8.0).gap(8.0)
        .child(text(hint).font(SMALL12).color(if ask.wrong { PAL.danger_text } else { PAL.muted }).wrap())
        .child(
            row().w_full().gap(8.0).center_y()
                .child(
                    row().key("airplay-code-box").role("textbox").grow(1.0).min_w(0.0).h(32.0).px(10.0).center_y().radius(8.0)
                        .fill(PAL.field).border(1.0, PAL.field_border).cursor(Cursor::Text).child(input),
                )
                .child(small_button("airplay-code-ok", "Connect").fill(ov(0.10)).on_press(submit)),
        )
}

fn note(key: &'static str, spinner: bool, msg: &str) -> El {
    let mut r = row().key(key).w_full().min_h(ROW_H).px(12.0).py(8.0).gap(10.0).center_y();
    if spinner {
        r = r.child(el().size(28.0, 28.0).center().child(icon(&icons::SWIRL, 16.0).color(PAL.muted).looping(LoopSpec::rotation(1.6))));
    }
    r.child(text(msg.to_string()).font(SMALL13).color(PAL.muted).grow(1.0).min_w(0.0).wrap())
}

/// The speakers to list: those found, with the one in use first in line even when the network
/// hasn't answered for it yet (the picker was just opened, or `uzu` chose it by name).
fn listed(s: &State) -> Vec<Output> {
    let mut list = s.airplay.devices.clone();
    let current = s.airplay.cast.device().or(s.airplay.ask.as_ref().map(|a| (a.id.as_str(), a.name.as_str())));
    if let Some((id, name)) = current
        && !list.iter().any(|o| o.id == id || o.name.eq_ignore_ascii_case(name))
    {
        list.insert(0, Output { id: id.to_string(), name: name.to_string(), kind: Kind::Speaker, pair: false, password: false });
    }
    list
}

/// The popover above its button, or None while closed. `win` is the window size.
pub fn overlay(s: &State, cx: &Cx<State>, win: [f32; 2]) -> Option<El> {
    let p = &s.ui.airplay;
    if !p.open {
        return None;
    }
    // Above the button, centred on it, inside the window. Re-read only when the button moves.
    let rect = cx.window_layout_with(p.anchor, |r| [r[0].round(), r[1].round(), r[2].round()]);
    let (centre, top) = rect.map_or((win[0] - W / 2.0 - 14.0, win[1] - PLAYER_H), |r| (r[0] + r[2] / 2.0, r[1]));
    let left = (centre - W / 2.0).clamp(8.0, (win[0] - W - 8.0).max(8.0));
    let bottom = (win[1] - top + 8.0).max(8.0);

    let cast = &s.airplay.cast;
    let mut panel = col().key("airplay-pop").role("menu").label("AirPlay").w(W).padding(6.0).radius(12.0)
        .fill(pop_fill()).border(1.0, pop_border()).shadow(8.0, 28.0, shadow(0.5))
        .pin(Some(left), None, None, Some(bottom))
        .transition(Transition::opacity())
        .modal(true, close)
        // Enter picks the focused row (Rune presses it on Space; the window's own Enter handling
        // doesn't see keys inside a modal).
        .on_key(|_s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
            if ev.key == Key::Enter && ev.mods == rune::Mods::default() {
                if !tx.key_repeat() {
                    tx.send_key(KeyEvent { key: Key::Text(" ".into()), mods: rune::Mods::default() });
                }
                return true;
            }
            false
        })
        .on_press(|_s: &mut State| {});
    let here = if cfg!(target_os = "macos") { kind_glyph(Kind::Computer, false) } else { Glyph { sym: Sym::Computer, line: &DESKTOP } };
    panel = panel.child(el().key("airplay-title").w_full().px(10.0).pt(4.0).pb(6.0).child(text("AirPlay").font(TextStyle::new(12.0, 600.0)).color(PAL.muted)));
    panel = panel.child(
        output_row("airplay-this".into(), here, this_computer(), None, if *cast == Cast::Off { Mark::Tick } else { Mark::None })
            .on_press(|s: &mut State| s.airplay_pick(None)),
    );
    if !s.airplay.available {
        return Some(layer(panel.child(super::menu::sep()).child(note(
            "airplay-unavailable",
            false,
            "AirPlay speakers aren't available: this install has no cliairplay helper.",
        ))));
    }
    let list = listed(s);
    if !list.is_empty() {
        panel = panel.child(super::menu::sep());
    }
    for out in list.iter().take(MAX_ROWS) {
        let asking = s.airplay.ask.as_ref().filter(|a| a.id == out.id || a.name.eq_ignore_ascii_case(&out.name));
        let (mark, state) = match cast {
            Cast::On { id, .. } if *id == out.id => (Mark::Tick, None),
            Cast::Connecting { id, name } if *id == out.id || name.eq_ignore_ascii_case(&out.name) => (Mark::Spinner, Some("Connecting\u{2026}")),
            _ => (Mark::None, None),
        };
        let sub = match (state, out.pair, out.password && asking.is_none()) {
            (Some(st), _, _) => Some(st.to_string()),
            (None, true, _) => Some("Stereo pair".to_string()),
            (None, false, true) => Some("Password".to_string()),
            _ => None,
        };
        let id = out.id.clone();
        panel = panel.child(
            output_row(format!("airplay-dev-{}", out.id), kind_glyph(out.kind, out.pair), &out.name, sub, mark)
                .on_press(move |s: &mut State| s.airplay_pick(Some(&id))),
        );
        if mark == Mark::Tick {
            panel = panel.child(volume_row(s));
        }
        if let Some(ask) = asking {
            panel = panel.child(ask_rows(s, ask));
        }
    }
    if list.len() > MAX_ROWS {
        panel = panel.child(note("airplay-more", false, &format!("and {} more", list.len() - MAX_ROWS)));
    }
    if s.airplay.devices.is_empty() {
        panel = panel.child(note("airplay-looking", true, "Looking for speakers\u{2026}"));
    }
    Some(layer(panel))
}

fn layer(panel: El) -> El {
    el().key("airplay-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(
            el().key("airplay-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .on_press(close)
                .on_secondary_press(|s: &mut State, _p, _tx: &mut Tx| close(s)),
        )
        .child(panel)
}

// ---- scenes (headless renders) ----

fn out(id: &str, name: &str, kind: Kind, pair: bool, password: bool) -> Output {
    Output { id: id.into(), name: name.into(), kind, pair, password }
}

fn scene_devices() -> Vec<Output> {
    vec![
        out("aa", "Bedroom", Kind::HomePodMini, false, false),
        out("bb", "Kitchen", Kind::Speaker, false, true),
        out("cc", "Living Room", Kind::HomePod, true, false),
        out("dd", "Living Room TV", Kind::AppleTv, false, false),
        out("ee", "Smart TV Pro", Kind::Tv, false, false),
    ]
}

/// The picker's states, over whatever the scene before set up. `what`: `closed`, `looking`,
/// `list`, `connecting`, `on`, `pin`, `password`, `password-wrong`, `unavailable`, `failed`.
pub fn scene(s: &mut State, what: &str) {
    symbols::load_now();
    s.airplay.available = what != "unavailable";
    s.airplay.prefs.volume = Some(35);
    s.ui.airplay.open = !matches!(what, "closed" | "failed" | "on-closed" | "unavailable");
    s.ui.airplay.anchor = if s.ui.now_playing { "np-airplay" } else { "airplay" };
    s.airplay.browsing = s.ui.airplay.open;
    if !matches!(what, "looking" | "unavailable") {
        s.airplay.devices = scene_devices();
    }
    let (id, name) = ("aa".to_string(), "Bedroom".to_string());
    match what {
        "connecting" => s.airplay.cast = Cast::Connecting { id, name },
        "on" | "on-closed" => s.airplay.cast = Cast::On { id, name, latency_ms: Some(480) },
        "pin" => s.airplay.ask = Some(Ask { id: "dd".into(), name: "Living Room TV".into(), need: Need::Pin, wrong: false }),
        "password" | "password-wrong" => {
            s.airplay.ask = Some(Ask { id: "bb".into(), name: "Kitchen".into(), need: Need::Password, wrong: what == "password-wrong" });
        }
        "failed" => s.toast = Some(format!("{} Playing on {}.", uzumaki_airplay::Why::Lost.message("Bedroom"), this_computer())),
        _ => {}
    }
    if what == "pin" {
        s.ui.airplay.asked = s.airplay.ask.clone();
        s.ui.airplay.field = TextEdit::new("12");
        s.ui.airplay.field.focused = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(what: &str) -> State {
        let mut s = State::offline();
        scene(&mut s, what);
        s
    }

    #[test]
    fn without_the_helper_there_is_no_button_no_search_and_uzu_says_so() {
        let mut s = State::offline();
        s.airplay.available = false;
        assert!(button(&s, "airplay", PAL.text2).is_none(), "no dead button");
        toggle(&mut s, "airplay");
        assert!(!s.ui.airplay.open && !s.airplay.browsing, "nothing opens, no mDNS");
        s.airplay_browse(true);
        s.airplay_ctl_browse(true);
        assert!(!s.airplay.browsing);
        assert_eq!(s.airplay_pick_named("Bedroom"), Err(crate::state::airplay::AIRPLAY_UNAVAILABLE.to_string()));
        assert!(crate::state::airplay::AIRPLAY_UNAVAILABLE.starts_with("AirPlay is not available"));
        s.airplay.available = true;
        assert!(button(&s, "airplay", PAL.text2).is_some());
    }

    #[test]
    fn opening_looks_for_speakers_and_closing_stops() {
        let mut s = State::offline();
        s.airplay.available = true;
        assert!(!s.airplay.browsing && !s.ui.airplay.open);
        toggle(&mut s, "airplay");
        assert!(s.airplay.browsing && s.ui.airplay.open);
        // The same button again closes; another button moves the popover to it.
        toggle(&mut s, "np-airplay");
        assert!(s.ui.airplay.open && s.ui.airplay.anchor == "np-airplay" && s.airplay.browsing);
        toggle(&mut s, "np-airplay");
        assert!(!s.airplay.browsing && !s.ui.airplay.open);
    }

    #[test]
    fn picking_moves_the_choice_and_this_computer_takes_it_back() {
        let mut s = open("list");
        s.airplay_pick(Some("aa"));
        assert_eq!(s.airplay.cast.device(), Some(("aa", "Bedroom")));
        s.airplay_pick(Some("nobody")); // gone from the list: nothing
        assert_eq!(s.airplay.cast.device(), Some(("aa", "Bedroom")));
        s.airplay_pick(None);
        assert_eq!(s.airplay.cast, Cast::Off);
    }

    #[test]
    fn the_speaker_in_use_is_listed_before_the_network_answers() {
        let mut s = open("on");
        s.airplay.devices.clear();
        let list = listed(&s);
        assert_eq!(list.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["Bedroom"]);
        s.airplay.devices = scene_devices();
        assert_eq!(listed(&s).len(), 5, "not twice once it is found");
    }

    #[test]
    fn a_question_focuses_an_empty_field_once() {
        let mut s = open("list");
        s.ui.airplay.field = TextEdit::new("old");
        s.airplay.ask = Some(Ask { id: "dd".into(), name: "Living Room TV".into(), need: Need::Pin, wrong: false });
        assert!(poll(&mut s));
        assert!(s.ui.airplay.field.focused && s.ui.airplay.field.text.is_empty());
        s.ui.airplay.field = TextEdit::new("12");
        assert!(!poll(&mut s), "typing isn't interrupted");
        assert_eq!(s.ui.airplay.field.text, "12");
        // Answered: the field goes and the row shows it is connecting.
        s.airplay_answer("1234");
        assert!(s.airplay.ask.is_none());
        assert_eq!(s.airplay.cast, Cast::Connecting { id: "dd".into(), name: "Living Room TV".into() });
    }

    #[test]
    fn the_volume_is_the_speakers_own() {
        let mut s = open("on");
        let app = s.playback.volume;
        s.airplay_volume(80, true);
        assert_eq!(s.airplay.volume(), 80);
        assert_eq!(s.playback.volume, app, "the app's volume is untouched");
        s.airplay_volume(250, true);
        assert_eq!(s.airplay.volume(), 100);
    }
}
