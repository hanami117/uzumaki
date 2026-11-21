//! The AirPlay picker: a button in the player bar and in Now Playing, and its popover.
//!
//! With Settings → Speakers → Chromecast on, the same popover lists the Cast devices too, under
//! their own heading (one output at a time: picking one ends the AirPlay speakers, and the other
//! way round); the device in use gets its volume slider, and the button shows its glyph.
//!
//! The popover lists this computer and the AirPlay speakers on the network (found only while it
//! is open), each with a checkbox, as Apple's Music does: tick several speakers and they play
//! together, in step (multi-room); this computer is either alone or off (picking it unticks every
//! speaker, unticking the last speaker picks it). A speaker connecting shows a spinner in its box;
//! each speaker in use gets its own volume slider under its row, and with several a master slider
//! under the title moves them all in proportion. A speaker that wants a password or a PIN gets a
//! field. Tab moves through the rows, Enter toggles, Esc closes.
//!
//! The button draws nothing that moves and reads only `State::airplay`: idle, it costs the view
//! one more icon.

use rune::{Cursor, Cx, El, Icon, Key, KeyEvent, LoopSpec, Phase, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, text, text_input};
use std::time::Duration;

use uzumaki_airplay::discover::{Kind, Output};
use uzumaki_airplay::{Cast, Need};
use uzumaki_cast::{CastDevice, State as CastState};

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
    /// The speakers' volumes when the master slider's drag began.
    master_from: Option<Vec<(String, u8)>>,
    /// The "no speaker found" hint was drawn (`poll` redraws once when it is due).
    hinted: bool,
}

/// How long the picker looks before it says why no speaker may show up.
pub const NO_SPEAKER_HINT_AFTER: Duration = Duration::from_secs(6);

/// The hint under "Looking for speakers…" once nothing turned up for a while.
pub const NO_SPEAKER_HINT: &str = "No speakers found yet. They must be on the same network as this computer. A VPN \
     kill switch or a firewall can hide them: allow multicast to 224.0.0.251 (mDNS, UDP port 5353).";

/// Nothing turned up for `NO_SPEAKER_HINT_AFTER` while the picker looks.
fn no_speaker_hint(s: &State) -> bool {
    s.ui.airplay.open && s.airplay.devices.is_empty() && s.airplay.browsing_since.is_some_and(|t| crate::clock::since(t) >= NO_SPEAKER_HINT_AFTER)
}

/// Opens the picker above the button `anchor` (or closes it, on a second press) and starts or
/// stops looking for speakers.
pub fn toggle(s: &mut State, anchor: &'static str) {
    if !shown(s) {
        return;
    }
    if s.ui.airplay.open && s.ui.airplay.anchor == anchor {
        return close(s);
    }
    s.ui.volume_open = false;
    s.ui.airplay.open = true;
    s.ui.airplay.anchor = anchor;
    s.airplay_browse(true);
    s.cast_browse(true);
}

pub fn close(s: &mut State) {
    s.ui.airplay.open = false;
    s.ui.airplay.field.focused = false;
    s.airplay_browse(false);
    s.cast_browse(false);
}

/// The button exists: this install has AirPlay, or Chromecast is on.
fn shown(s: &State) -> bool {
    s.airplay.available || s.cast_enabled()
}

/// The field of a question that just arrived gets the keyboard (and starts empty), and the
/// "no speaker found" hint shows when it is due. Returns whether the view must run.
pub fn poll(s: &mut State, tx: &mut Tx) -> bool {
    // Closed by a change of page (`UiState::route_changed`): stop looking for speakers too.
    if !s.ui.airplay.open && s.airplay.browsing {
        s.airplay_browse(false);
    }
    if !s.ui.airplay.open && s.cast.browsing {
        s.cast_browse(false);
    }
    let hint = no_speaker_hint(s);
    let hint_changed = hint != s.ui.airplay.hinted;
    s.ui.airplay.hinted = hint;
    if !hint && s.ui.airplay.open && s.airplay.devices.is_empty()
        && let Some(t) = s.airplay.browsing_since
    {
        tx.wake_after(NO_SPEAKER_HINT_AFTER.saturating_sub(crate::clock::since(t)) + Duration::from_millis(20));
    }
    if s.ui.airplay.asked == s.airplay.ask {
        return hint_changed;
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
    if !shown(s) {
        return None;
    }
    let lit = s.airplay.active() || s.cast.active() || (s.ui.airplay.open && s.ui.airplay.anchor == key);
    let label = match (&s.airplay.cast, &s.cast.state) {
        (_, CastState::Connecting { name } | CastState::Launching { name } | CastState::Loading { name }) => format!("Chromecast: connecting to {name}"),
        (_, CastState::Buffering { name } | CastState::Playing { name, .. } | CastState::Paused { name }) => format!("Chromecast: playing on {name}"),
        (Cast::Off, CastState::Off) => "AirPlay".to_string(),
        (Cast::Connecting { .. }, _) => format!("AirPlay: connecting to {}", s.airplay.names()),
        (Cast::On { .. }, _) => format!("AirPlay: playing on {}", s.airplay.names()),
    };
    // On a Cast device the button wears that device's glyph, so the bar says where the sound is.
    let glyph = match s.cast.state.name() {
        Some(name) => cast_glyph(s.cast.devices.iter().find(|d| d.name == name)),
        None => Glyph { sym: Sym::AirPlay, line: &AIRPLAY },
    };
    Some(el().key(key).role("button").label(label).focusable(true).size(36.0, 36.0).radius(18.0).center()
        .hover(ov(0.08)).press(ov(0.12)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| toggle(s, key))
        .child(glyph_el(glyph, if lit { PAL.accent } else { idle }, 18.0)))
}

/// What a row shows at its right end: its checkbox.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    Off,
    On,
    Connecting,
}

fn checkbox(mark: Mark) -> El {
    let b = el().size(18.0, 18.0).radius(5.0).center();
    match mark {
        Mark::Off => b.border(1.5, PAL.muted),
        Mark::On => b.fill(PAL.accent).child(icon(&icons::CHECK, 13.0).color(rune::Color::WHITE)),
        Mark::Connecting => b.child(icon(&icons::SWIRL, 16.0).color(PAL.accent).looping(LoopSpec::rotation(1.6))),
    }
}

fn output_row(key: String, glyph: Glyph, name: &str, sub: Option<String>, mark: Mark, playing: bool) -> El {
    let mut words = col().grow(1.0).min_w(0.0).gap(1.0).justify_center()
        .child(text(name.to_string()).font(TextStyle::new(13.5, if mark != Mark::Off { 600.0 } else { 400.0 })).color(PAL.text).truncate());
    if let Some(sub) = sub {
        words = words.child(text(sub).font(SMALL12).color(PAL.muted).truncate());
    }
    let state = match mark {
        Mark::Off => "not selected",
        Mark::On => "selected",
        Mark::Connecting => "connecting",
    };
    row().key(key).role("menuitemcheckbox").label(format!("{name}, {state}")).focusable(true).w_full().h(ROW_H).px(10.0).gap(10.0).center_y().radius(8.0)
        .hover(ov(0.09)).press(ov(0.14)).cursor(Cursor::Pointer)
        // As Apple's picker: the device in a round badge, filled with the accent where the sound is.
        .child(
            el().size(32.0, 32.0).radius(16.0).center().fill(if playing { PAL.accent } else { ov(0.10) })
                .child(glyph_el(glyph, if playing { rune::Color::WHITE } else { PAL.text }, 21.0)),
        )
        .child(words)
        .child(checkbox(mark))
}

/// A slider key that lives as long as the app (one per speaker, kept once).
fn slider_key(id: &str) -> &'static str {
    static KEYS: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let want = format!("airplay-volume-{id}");
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = keys.iter().find(|k| **k == want) {
        return k;
    }
    let k: &'static str = Box::leak(want.into_boxed_str());
    keys.push(k);
    k
}

fn volume_line(key: String, slider_el: El, v: u8, indent: f32) -> El {
    row().key(key).w_full().h(30.0).pl(indent).pr(12.0).gap(10.0).center_y()
        .child(icon(icons::volume(f64::from(v) / 100.0), 15.0).color(PAL.muted))
        .child(el().grow(1.0).child(slider_el))
        .child(row().w(26.0).justify_end().child(text(v.to_string()).font(SMALL12.features("'tnum' 1")).color(PAL.muted)))
}

/// One speaker's own volume, under its row.
fn volume_row(s: &State, id: &str) -> El {
    let v = s.airplay.prefs.volume_of(id);
    let owned = id.to_string();
    let slider_el = slider(slider_key(id), f32::from(v) / 100.0, PAL.text2, move |s, f, phase, cancelled, _tx| {
        if !(phase == Phase::Up && cancelled) {
            s.airplay_speaker_volume(&owned, (f * 100.0).round() as u8, phase == Phase::Up);
        }
    });
    volume_line(format!("airplay-volume-row-{id}"), slider_el, v, 48.0)
}

/// Several speakers: the master volume, under the title. It moves every speaker in proportion.
fn master_row(s: &State) -> El {
    let v = s.airplay.master();
    let slider_el = slider("airplay-master", f32::from(v) / 100.0, PAL.text, |s, f, phase, cancelled, _tx| {
        let from = match s.ui.airplay.master_from.clone() {
            Some(f) => f,
            None => {
                let f = s.airplay_volumes();
                s.ui.airplay.master_from = Some(f.clone());
                f
            }
        };
        if phase == Phase::Up && cancelled {
            // Back to where the drag began.
            for (id, was) in &from {
                s.airplay_speaker_volume(id, *was, false);
            }
        } else {
            s.airplay_master_volume((f * 100.0).round() as u8, &from, phase == Phase::Up);
        }
        if phase == Phase::Up {
            s.ui.airplay.master_from = None;
        }
    });
    volume_line("airplay-master-row".into(), slider_el, v, 12.0)
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

/// The speakers to list: those found, with the ones in use first in line even when the network
/// hasn't answered for them yet (the picker was just opened, or `uzu` chose them by name).
fn listed(s: &State) -> Vec<Output> {
    let mut list = s.airplay.devices.clone();
    let mut current: Vec<(String, String)> = s.airplay.speakers.iter().map(|sp| (sp.id.clone(), sp.name.clone())).collect();
    if current.is_empty()
        && let Some((id, name)) = s.airplay.cast.device()
    {
        current.push((id.to_string(), name.to_string()));
    }
    current.extend(s.airplay.ask.as_ref().map(|a| (a.id.clone(), a.name.clone())));
    let mut n = 0;
    for (id, name) in current {
        if list.iter().any(|o| o.id == id || o.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        list.insert(n, Output { id, name, kind: Kind::Speaker, pair: false, password: false });
        n += 1;
    }
    list
}

/// A listed speaker's place in the group: off, connecting, or playing.
fn mark_of(s: &State, out: &Output) -> Mark {
    let found = s.airplay.speakers.iter().find(|sp| sp.id == out.id || sp.name.eq_ignore_ascii_case(&out.name));
    match (found, &s.airplay.cast) {
        (Some(sp), _) if sp.playing => Mark::On,
        (Some(_), _) => Mark::Connecting,
        (None, Cast::On { id, .. }) if s.airplay.speakers.is_empty() && *id == out.id => Mark::On,
        (None, Cast::Connecting { id, name }) if s.airplay.speakers.is_empty() && (*id == out.id || name.eq_ignore_ascii_case(&out.name)) => Mark::Connecting,
        _ => Mark::Off,
    }
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

    let title = match (s.airplay.available, s.cast_enabled()) {
        (false, true) => "Chromecast",
        _ => "AirPlay",
    };
    let mut panel = col().key("airplay-pop").role("menu").label(title).w(W).padding(6.0).radius(12.0)
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
    panel = panel.child(heading("airplay-title", title));
    let list = if s.airplay.available { listed(s) } else { Vec::new() };
    let in_use = list.iter().filter(|o| mark_of(s, o) != Mark::Off).count();
    if in_use > 1 {
        panel = panel.child(master_row(s));
    }
    let here_on = s.airplay.cast == Cast::Off && !s.cast.active();
    panel = panel.child(
        output_row("airplay-this".into(), here, this_computer(), None, if here_on { Mark::On } else { Mark::Off }, here_on).on_press(|s: &mut State| {
            s.airplay_pick(None);
            s.cast_stop();
        }),
    );
    if !s.airplay.available && s.cast_enabled() {
        return Some(layer(cast_rows(s, panel, false)));
    }
    if !s.airplay.available {
        return Some(layer(panel.child(super::menu::sep()).child(note(
            "airplay-unavailable",
            false,
            "AirPlay speakers aren't available: this install has no cliairplay helper.",
        ))));
    }
    if !list.is_empty() {
        panel = panel.child(super::menu::sep());
    }
    for out in list.iter().take(MAX_ROWS) {
        let asking = s.airplay.ask.as_ref().filter(|a| a.id == out.id || a.name.eq_ignore_ascii_case(&out.name));
        let mark = mark_of(s, out);
        let state = (mark == Mark::Connecting).then_some("Connecting\u{2026}");
        let sub = match (state, out.pair, out.password && asking.is_none()) {
            (Some(st), _, _) => Some(st.to_string()),
            (None, true, _) => Some("Stereo pair".to_string()),
            (None, false, true) => Some("Password".to_string()),
            _ => None,
        };
        let id = out.id.clone();
        panel = panel.child(
            output_row(format!("airplay-dev-{}", out.id), kind_glyph(out.kind, out.pair), &out.name, sub, mark, mark == Mark::On)
                .on_press(move |s: &mut State| s.airplay_toggle(&id)),
        );
        if mark != Mark::Off {
            panel = panel.child(volume_row(s, &out.id));
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
        if no_speaker_hint(s) {
            panel = panel.child(note("airplay-none-found", false, NO_SPEAKER_HINT));
        }
    }
    if s.cast_enabled() {
        panel = cast_rows(s, panel, true);
    }
    Some(layer(panel))
}

/// A section's small grey title.
fn heading(key: &'static str, title: &str) -> El {
    el().key(key).w_full().px(10.0).pt(4.0).pb(6.0).child(text(title.to_string()).font(TextStyle::new(12.0, 600.0)).color(PAL.muted))
}

/// A Cast device's glyph: a speaker for Google/Nest speakers and Cast Groups, else a TV (a
/// Chromecast, a Google TV, a TV with it built in).
fn cast_glyph(d: Option<&CastDevice>) -> Glyph {
    let speaker = d.is_some_and(|d| {
        let m = d.model.to_ascii_lowercase();
        d.group || ["speaker", "nest audio", "nest mini", "home mini", "google home", "audio"].iter().any(|k| m.contains(k))
    });
    kind_glyph(if speaker { Kind::Speaker } else { Kind::Tv }, false)
}

/// The Cast devices to list: those found, with the one in use first even before the network
/// answered for it (`uzu cast <name>`, or the picker was just opened).
fn cast_listed(s: &State) -> Vec<CastDevice> {
    let mut list = s.cast.devices.clone();
    if let Some(name) = s.cast.state.name()
        && !list.iter().any(|d| d.name == name)
    {
        let mut d = CastDevice::at(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0, name);
        d.id = format!("in-use:{name}");
        list.insert(0, d);
    }
    list
}

/// Where a listed Cast device stands: off, connecting, or playing (paused counts as playing).
fn cast_mark(s: &State, d: &CastDevice) -> Mark {
    if s.cast.state.name() != Some(d.name.as_str()) {
        return Mark::Off;
    }
    match s.cast.state {
        CastState::Off => Mark::Off,
        CastState::Connecting { .. } | CastState::Launching { .. } | CastState::Loading { .. } | CastState::Buffering { .. } => Mark::Connecting,
        CastState::Playing { .. } | CastState::Paused { .. } => Mark::On,
    }
}

/// The Cast section: its heading (`titled`: under the AirPlay speakers), each device with a
/// "Chromecast" line, the one in use with its volume.
fn cast_rows(s: &State, mut panel: El, titled: bool) -> El {
    panel = panel.child(super::menu::sep());
    if titled {
        panel = panel.child(heading("cast-title", "Chromecast"));
    }
    let list = cast_listed(s);
    for d in list.iter().take(MAX_ROWS) {
        let mark = cast_mark(s, d);
        let sub = match (&s.cast.state, mark) {
            (CastState::Connecting { .. }, Mark::Connecting) => "Connecting\u{2026}".to_string(),
            (CastState::Launching { .. } | CastState::Loading { .. }, Mark::Connecting) => "Starting\u{2026}".to_string(),
            (CastState::Buffering { .. }, Mark::Connecting) => "Buffering\u{2026}".to_string(),
            (CastState::Paused { .. }, Mark::On) => "Chromecast \u{b7} Paused".to_string(),
            (CastState::Playing { latency_ms: Some(ms), .. }, Mark::On) => format!("Chromecast \u{b7} {:.1} s behind", f64::from(*ms) / 1000.0),
            _ if d.group => "Chromecast \u{b7} Group".to_string(),
            _ => "Chromecast".to_string(),
        };
        let id = d.id.clone();
        let on = mark != Mark::Off;
        panel = panel.child(
            output_row(format!("cast-dev-{}", d.id), cast_glyph(Some(d)), &d.name, Some(sub), mark, mark == Mark::On).on_press(move |s: &mut State| {
                if on {
                    s.cast_stop(); // the one in use again: back to this computer
                } else {
                    s.cast_pick(&id);
                }
            }),
        );
        if on && let Some(v) = s.cast.volume {
            let slider_el = slider("cast-volume", f32::from(v) / 100.0, PAL.text2, |s, f, phase, cancelled, _tx| {
                if !(phase == Phase::Up && cancelled) {
                    s.cast_volume((f * 100.0).round() as u8);
                }
            });
            panel = panel.child(volume_line("cast-volume-row".into(), slider_el, v, 48.0));
        }
    }
    if list.len() > MAX_ROWS {
        panel = panel.child(note("cast-more", false, &format!("and {} more", list.len() - MAX_ROWS)));
    }
    if list.is_empty() {
        panel = panel.child(note("cast-looking", true, "Looking for Chromecast devices\u{2026}"));
    }
    panel
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

/// The picker's states, over whatever the scene before set up. `what`: `closed`, `looking`, `looking-none`,
/// `list`, `connecting`, `on`, `group`, `group-joining`, `group-failed`, `pin`, `password`,
/// `password-wrong`, `unavailable`, `failed`.
pub fn scene(s: &mut State, what: &str) {
    symbols::load_now();
    if let Some(cast) = what.strip_prefix("cast-") {
        return scene_cast(s, cast);
    }
    s.airplay.available = what != "unavailable";
    s.airplay.prefs.volume = Some(35);
    s.ui.airplay.open = !matches!(what, "closed" | "failed" | "on-closed" | "unavailable" | "group-failed");
    s.ui.airplay.anchor = if s.ui.now_playing { "np-airplay" } else { "airplay" };
    s.airplay.browsing = s.ui.airplay.open;
    s.airplay.browsing_since = s.airplay.browsing.then(crate::clock::now);
    if what == "looking-none" {
        // Looking for a while already, and nothing answered.
        s.airplay.browsing_since = crate::clock::now().checked_sub(NO_SPEAKER_HINT_AFTER + Duration::from_secs(1));
    }
    if !matches!(what, "looking" | "looking-none" | "unavailable") {
        s.airplay.devices = scene_devices();
    }
    let (id, name) = ("aa".to_string(), "Bedroom".to_string());
    let speaker = |id: &str, name: &str, playing: bool| uzumaki_airplay::Speaker { id: id.into(), name: name.into(), playing };
    match what {
        "connecting" => s.airplay.cast = Cast::Connecting { id, name },
        "on" | "on-closed" => s.airplay.cast = Cast::On { id, name, latency_ms: Some(480) },
        "group" | "group-joining" => {
            s.airplay.speakers = vec![speaker("aa", "Bedroom", true), speaker("cc", "Living Room", what == "group")];
            s.airplay.prefs.volumes.insert("aa".into(), 35);
            s.airplay.prefs.volumes.insert("cc".into(), 60);
            s.airplay.cast = Cast::On { id, name, latency_ms: Some(480) };
        }
        "group-failed" => {
            s.airplay.speakers = vec![speaker("cc", "Living Room", true)];
            s.airplay.cast = Cast::On { id: "cc".into(), name: "Living Room".into(), latency_ms: Some(480) };
            s.toast = Some(format!("{} Still playing on Living Room.", uzumaki_airplay::Why::Busy.message("Bedroom")));
        }
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

fn scene_cast_devices() -> Vec<CastDevice> {
    let dev = |n: u8, name: &str, model: &str, group: bool| {
        let mut d = CastDevice::at(std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, n)), 8009, name);
        d.id = format!("cast{n}");
        d.model = model.into();
        d.group = group;
        d
    };
    vec![dev(173, "Smart TV Pro", "Smart TV Pro", false), dev(40, "Kitchen speaker", "Nest Audio", false), dev(41, "Downstairs", "Google Cast Group", true)]
}

/// The picker with Chromecast on (`cast-` scenes): `list`, `looking`, `connecting`, `on`,
/// `paused`, `on-closed`, `failed`, and `only-*` for a system without AirPlay (Linux).
fn scene_cast(s: &mut State, what: &str) {
    let (only, what) = match what.strip_prefix("only-") {
        Some(w) => (true, w),
        None => (false, what),
    };
    s.settings.chromecast = true;
    s.airplay.available = !only;
    s.ui.airplay.open = !matches!(what, "on-closed" | "failed");
    s.ui.airplay.anchor = if s.ui.now_playing { "np-airplay" } else { "airplay" };
    if !only {
        s.airplay.devices = scene_devices();
    }
    if what != "looking" {
        s.cast.devices = scene_cast_devices();
    }
    let name = "Smart TV Pro".to_string();
    match what {
        "connecting" => s.cast.state = CastState::Loading { name },
        "on" | "on-closed" => {
            s.cast.state = CastState::Playing { name, latency_ms: Some(2400) };
            s.cast.volume = Some(26);
        }
        "paused" => {
            s.cast.state = CastState::Paused { name };
            s.cast.volume = Some(26);
        }
        "failed" => s.toast = Some(format!("{} Playing on {}.", uzumaki_cast::Why::Lost.message("Smart TV Pro"), this_computer())),
        _ => {}
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
    fn with_no_speaker_after_a_while_the_picker_says_why_that_may_be() {
        crate::clock::freeze();
        let mut s = State::offline();
        s.airplay.available = true;
        toggle(&mut s, "airplay");
        assert!(s.airplay.browsing_since.is_some());
        assert!(!no_speaker_hint(&s), "not at once");
        crate::clock::advance(NO_SPEAKER_HINT_AFTER);
        assert!(no_speaker_hint(&s));
        assert!(poll(&mut s, &mut Tx::default()), "the view runs once to show it");
        assert!(!poll(&mut s, &mut Tx::default()));
        // A speaker answers: no hint. Closed and opened again: the wait starts over.
        s.airplay.devices = scene_devices();
        assert!(!no_speaker_hint(&s));
        s.airplay.devices.clear();
        close(&mut s);
        assert!(s.airplay.browsing_since.is_none() && !no_speaker_hint(&s));
        toggle(&mut s, "airplay");
        assert!(!no_speaker_hint(&s));
        assert!(NO_SPEAKER_HINT.contains("VPN") && NO_SPEAKER_HINT.contains("224.0.0.251") && NO_SPEAKER_HINT.contains("5353"));
        assert!(no_speaker_hint(&open("looking-none")) && !no_speaker_hint(&open("looking")));
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
    fn checkboxes_add_and_remove_speakers_and_this_computer_stays_alone() {
        let mut s = open("list");
        let ids = |s: &State| s.airplay.ids();
        s.airplay_toggle("aa");
        assert_eq!(ids(&s), ["aa"], "the first tick: that speaker alone");
        assert_eq!(mark_of(&s, &s.airplay.devices[0]), Mark::Connecting);
        s.airplay_toggle("cc");
        assert_eq!(ids(&s), ["aa", "cc"], "the second joins it");
        s.airplay_toggle("nobody");
        assert_eq!(ids(&s).len(), 2, "gone from the list: nothing");
        s.airplay_toggle("aa");
        assert_eq!(ids(&s), ["cc"], "unticked: the other plays on");
        s.airplay_toggle("cc");
        assert_eq!(s.airplay.cast, Cast::Off, "the last one out: this computer");
        assert!(ids(&s).is_empty());
        // This computer unticks every speaker at once.
        s.airplay_toggle("aa");
        s.airplay_toggle("bb");
        s.airplay_pick(None);
        assert!(ids(&s).is_empty() && s.airplay.cast == Cast::Off);
    }

    #[test]
    fn a_group_shows_a_master_slider_and_one_per_speaker() {
        let s = open("group");
        let list = listed(&s);
        let marks: Vec<Mark> = list.iter().map(|o| mark_of(&s, o)).collect();
        assert_eq!(marks.iter().filter(|m| **m == Mark::On).count(), 2);
        assert_eq!(s.airplay.master(), 60, "the loudest speaker");
        let joining = open("group-joining");
        assert!(listed(&joining).iter().any(|o| mark_of(&joining, o) == Mark::Connecting));
        assert_eq!(slider_key("aa"), slider_key("aa"), "one key per speaker, kept");
        assert!(std::ptr::eq(slider_key("aa"), slider_key("aa")));
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
        assert!(poll(&mut s, &mut Tx::default()));
        assert!(s.ui.airplay.field.focused && s.ui.airplay.field.text.is_empty());
        s.ui.airplay.field = TextEdit::new("12");
        assert!(!poll(&mut s, &mut Tx::default()), "typing isn't interrupted");
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
        s.airplay_speaker_volume("aa", 80, true);
        assert_eq!(s.airplay.prefs.volume_of("aa"), 80);
        assert_eq!(s.airplay.volume(), 80, "the next speaker starts there");
        assert_eq!(s.playback.volume, app, "the app's volume is untouched");
        s.airplay_speaker_volume("aa", 250, true);
        assert_eq!(s.airplay.prefs.volume_of("aa"), 100);
    }
}
