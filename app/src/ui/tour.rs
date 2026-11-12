//! The onboarding's shortcuts tour (its last step).
//!
//! One card per shortcut, built from `keys::BINDINGS` with the user's own chords (a removed
//! shortcut drops its card) and Rune's keycaps for the platform ("Ctrl" "Shift" "B" on Linux,
//! "⇧" "⌘" "B" on macOS). Above the words, a small picture of the app (sidebar, content, right
//! sidebar, player bar) does what the key does when it is pressed; the keycap lights, a tick
//! flashes and a tick shows. The effect stays, and the key toggles as it does in the app (the
//! right sidebar opens and closes, and so on), as often as the user likes. The card moves on only
//! with Next (Return); once a key was tried, Next gets a soft glow as a hint. The last card
//! lists the rest (number keys, Settings, every shortcut) and ends the tour with Finish.
//!
//! Local only mode: no lyrics (the Lyrics and Up Next card is Up Next only) and the number keys
//! name the local tabs (1–4).
//!
//! The picture is one element tree whose sizes follow `Mini`; a key press changes `Mini` once
//! and Rune springs the rest (no per-frame view runs). It stays mounted from card to card, so
//! it morphs into each card's start. Reduce Motion: its changes land at once
//! (`instant_changes`); entrances are Rune's short fades.

use rune::{Cursor, El, Transition, col, el, icon, row, text};

use super::icons;
use super::keys::{self, Action};
use crate::state::State;
use crate::theme::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    PlayPause,
    Sidebar,
    RightPanel,
    /// Lyrics and Up Next (local only: Up Next).
    Panels,
    NowPlaying,
    Find,
    /// Next and previous song.
    Skip,
    /// The closing card: number keys, Settings, every shortcut.
    More,
}

const ORDER: [Card; 8] = [Card::PlayPause, Card::Sidebar, Card::RightPanel, Card::Panels, Card::NowPlaying, Card::Find, Card::Skip, Card::More];

impl Card {
    /// The shortcuts shown (and tried) on the card. The closing card takes the number keys too.
    pub fn actions(self, local: bool) -> &'static [Action] {
        match self {
            Card::PlayPause => &[Action::PlayPause],
            Card::Sidebar => &[Action::ToggleSidebar],
            Card::RightPanel => &[Action::ToggleRightPanel],
            Card::Panels if local => &[Action::ShowQueue],
            Card::Panels => &[Action::ShowLyrics, Action::ShowQueue],
            Card::NowPlaying => &[Action::NowPlaying],
            Card::Find => &[Action::Find],
            Card::Skip => &[Action::Next, Action::Previous],
            Card::More => &[Action::Settings, Action::Shortcuts],
        }
    }

    /// Whether pressing `a` tries this card.
    pub fn takes(self, a: Action, local: bool) -> bool {
        matches!((self, a), (Card::More, Action::Slot(_))) || self.actions(local).contains(&a)
    }

    fn id(self) -> &'static str {
        match self {
            Card::PlayPause => "play",
            Card::Sidebar => "sidebar",
            Card::RightPanel => "right",
            Card::Panels => "panels",
            Card::NowPlaying => "now-playing",
            Card::Find => "find",
            Card::Skip => "skip",
            Card::More => "more",
        }
    }

    fn title(self, local: bool) -> &'static str {
        match self {
            Card::PlayPause => "Play and pause",
            Card::Sidebar => "The sidebar",
            Card::RightPanel => "The right sidebar",
            Card::Panels if local => "Up Next",
            Card::Panels => "Lyrics and Up Next",
            Card::NowPlaying => "Now Playing",
            Card::Find => "Find",
            Card::Skip => "Next and previous",
            Card::More => "A few more",
        }
    }

    fn line(self, local: bool) -> &'static str {
        match self {
            Card::PlayPause => "Plays or pauses from anywhere in the app, except while you type.",
            Card::Sidebar => "Folds the sidebar into a slim rail, and back.",
            Card::RightPanel if local => "Opens the panel on the right, with what plays next.",
            Card::RightPanel => "Opens the panel on the right, for lyrics and what plays next.",
            Card::Panels if local => "Opens the right sidebar on what plays next.",
            Card::Panels => "Opens the right sidebar on the lyrics, or on what plays next.",
            Card::NowPlaying if local => "The song full-screen. The same keys close it.",
            Card::NowPlaying => "The song full-screen, with its lyrics. The same keys close it.",
            Card::Find => "Filters the list on the page. Press it again to search.",
            Card::Skip => "The next song, or back to the start of this one.",
            Card::More => "Pages by number, and every shortcut in one place.",
        }
    }
}

/// The cards for the user's shortcuts now: one whose every shortcut was removed is left out.
/// The closing card always shows (it names Settings → Keyboard).
pub fn cards(s: &State) -> Vec<Card> {
    let local = s.local_only();
    ORDER
        .into_iter()
        .filter(|c| *c == Card::More || c.actions(local).iter().any(|a| keys::action_caps(s, *a).is_some()))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Lyrics,
    Queue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    Settings,
    Keyboard,
}

/// What the small picture of the app shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mini {
    pub playing: bool,
    pub folded: bool,
    pub right: Option<Pane>,
    pub now_playing: bool,
    pub filter: bool,
    /// The right sidebar's tab last shown (a plain toggle opens it again on that one).
    pub tab: Option<Pane>,
    /// Songs skipped (+1 next, −1 previous): the title slides over.
    pub song: i32,
    /// The last skip's direction, for the slide.
    pub dir: i8,
    /// A number key lit this sidebar entry (1–9).
    pub slot: Option<u8>,
    pub sheet: Option<Sheet>,
}

impl Mini {
    /// Each card starts from the plain app, a song playing (paused for Play and pause).
    fn start(card: Card) -> Self {
        Mini { playing: card != Card::PlayPause, ..Default::default() }
    }

    fn show(&mut self, p: Pane) {
        self.right = if self.right == Some(p) { None } else { Some(p) };
        self.tab = Some(p);
    }

    /// What the app would do for `a`.
    pub fn apply(&mut self, a: Action, local: bool) {
        match a {
            Action::PlayPause => self.playing = !self.playing,
            Action::ToggleSidebar => self.folded = !self.folded,
            // As the app: a plain toggle keeps the tab; a tab key opens on it, or closes when
            // the sidebar already shows it.
            Action::ToggleRightPanel => {
                self.right = match self.right {
                    Some(_) => None,
                    None => Some(self.tab.unwrap_or(if local { Pane::Queue } else { Pane::Lyrics })),
                };
            }
            Action::ShowLyrics => self.show(Pane::Lyrics),
            Action::ShowQueue => self.show(Pane::Queue),
            Action::NowPlaying => self.now_playing = !self.now_playing,
            Action::Find => self.filter = !self.filter,
            Action::Next => {
                self.song += 1;
                self.dir = 1;
            }
            Action::Previous => {
                self.song -= 1;
                self.dir = -1;
            }
            Action::Slot(n) => {
                self.slot = Some(n);
                self.sheet = None;
            }
            Action::Settings => {
                self.sheet = if self.sheet == Some(Sheet::Settings) { None } else { Some(Sheet::Settings) };
                self.slot = None;
            }
            Action::Shortcuts => {
                self.sheet = Some(Sheet::Keyboard);
                self.slot = None;
            }
            _ => {}
        }
    }
}

/// The tour's own state, inside the onboarding.
#[derive(Clone, Debug, Default)]
pub struct Tour {
    /// Index into `cards`.
    pub card: usize,
    /// The card's shortcut that was pressed last (the tick shows once there is one).
    pub hit: Option<Action>,
    /// The keycap of `hit` is lit: set by a press, cleared `FLASH` later (`poll`).
    pub lit: bool,
    /// Poll time the flash was first seen; unset by each press.
    pub lit_at: Option<f64>,
    pub mini: Mini,
}

/// How long a keycap stays coral after a press.
pub const FLASH: f64 = 0.35;

impl Tour {
    /// On card `i` (`card`), from its start.
    pub fn at(i: usize, card: Card) -> Self {
        Tour { card: i, hit: None, lit: false, lit_at: None, mini: Mini::start(card) }
    }

    /// The card's shortcut `a` was pressed.
    pub fn press(&mut self, a: Action, local: bool) {
        self.hit = Some(a);
        self.lit = true;
        self.lit_at = None;
        self.mini.apply(a, local);
    }
}

// ---------------------------------------------------------------- view

/// The tour step: the count, the picture, the card's words and keys, and Next.
/// `back`: the last move went back (cards slide the other way).
pub fn view(s: &State, t: &Tour, win: [f32; 2], back: bool, reduced: bool, next: El) -> El {
    let list = cards(s);
    let i = t.card.min(list.len() - 1);
    let card = list[i];
    let local = s.local_only();
    let w = (win[0] - 64.0).clamp(280.0, 460.0);
    // The picture gives way on short windows; the closing card's list needs more room.
    // The closing card shows no key's effect, so a very short window leaves its picture out.
    let (room, least) = match card {
        Card::More if cfg!(target_os = "macos") => (522.0, 0.0),
        Card::More => (500.0, 0.0),
        _ => (400.0, 100.0),
    };
    let h = (win[1] - room).clamp(least, (w * 0.52).round());
    let picture = h >= 72.0;

    let sign = if back { -1.0 } else { 1.0 };
    let enter = Transition::offset(24.0 * sign, 0.0).combined(Transition::opacity());
    let exit = Transition::offset(-24.0 * sign, 0.0).combined(Transition::opacity());

    let mut words = col().key(format!("onb/tour/card/{}", card.id())).items_center().w(w).transition(enter);
    if card == Card::More {
        words = words
            .child(title(card, local))
            .child(text(card.line(local)).font(BODY14).color(PAL.text2).mt(6.0).wrap())
            .child(more(s, t, w, reduced));
    } else {
        words = words
            .child(keys_row(s, t, card, local, reduced))
            .child(title(card, local).mt(14.0))
            .child(text(card.line(local)).key("onb/tour/line").font(BODY14).color(PAL.text2).mt(6.0).wrap());
    }

    col().key("onb/tour").items_center()
        .child(
            text(format!("SHORTCUTS \u{b7} {} OF {}", i + 1, list.len())).key("onb/tour/count")
                .font(SECTION11).color(PAL.nav_text)
                .role("status").label(format!("Shortcut {} of {}", i + 1, list.len())),
        )
        .children(picture.then(|| mini(t.mini, local, w, h, reduced).mt(12.0)))
        .child(col().key("onb/tour/words").items_center().mt(18.0).child_exit(exit).child(words))
        .child(el().key("onb/tour/next-wrap").mt(18.0).child(if t.hit.is_some() { next.shadow(0.0, 18.0, PAL.accent.alpha(0.4)) } else { next }))
}

fn title(card: Card, local: bool) -> El {
    text(card.title(local)).key("onb/tour/title").font(TextStyle22).color(PAL.text)
}

#[allow(non_upper_case_globals)]
const TextStyle22: rune::TextStyle = rune::TextStyle::new(22.0, 600.0).tracking(-0.3);

/// One key, drawn as a keycap; lit once its shortcut was pressed.
fn keycap(label: &str, lit: bool, reduced: bool) -> El {
    let cap = el().min_w(30.0).h(30.0).px(9.0).radius(7.0).center()
        .fill(if lit { PAL.accent } else { ov(0.08) })
        .border(1.0, if lit { PAL.accent } else { PAL.field_border })
        .child(text(label.to_string()).font(rune::TextStyle::new(13.0, 600.0)).color(if lit { PAL.ink } else { PAL.text }));
    if reduced { cap.instant_changes() } else { cap }
}

/// Keycaps as a screen reader should say them: "Shift Command B", not "⇧ ⌘ B".
pub fn spoken(caps: &[String]) -> String {
    caps.iter()
        .map(|c| match c.as_str() {
            "\u{2318}" => "Command",
            "\u{21e7}" => "Shift",
            "\u{2325}" => "Option",
            "\u{2303}" => "Control",
            "\u{2190}" => "Left Arrow",
            "\u{2192}" => "Right Arrow",
            "\u{2191}" => "Up Arrow",
            "\u{2193}" => "Down Arrow",
            "\u{21a9}" | "\u{23ce}" => "Return",
            "\u{238b}" => "Escape",
            "\u{232b}" => "Delete",
            "," => "Comma",
            "/" => "Slash",
            "." => "Period",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A shortcut's keycaps side by side.
fn caps_el(key: String, caps: &[String], lit: bool, reduced: bool) -> El {
    row().key(key).gap(4.0).center_y().role("group").label(spoken(caps)).children(caps.iter().map(|c| keycap(c, lit, reduced).a11y_hidden()))
}

/// The card's shortcuts (joined by "or"), then "Try it" or, once pressed, a tick.
fn keys_row(s: &State, t: &Tour, card: Card, local: bool, reduced: bool) -> El {
    let mut r = row().key("onb/tour/keys").gap(10.0).center_y();
    let mut first = true;
    for a in card.actions(local) {
        let Some(caps) = keys::action_caps(s, *a) else { continue };
        if !first {
            r = r.child(text("or").font(SMALL13).color(PAL.dim));
        }
        first = false;
        r = r.child(caps_el(format!("onb/tour/caps/{}", a.id()), &caps, t.lit && t.hit == Some(*a), reduced));
    }
    let status = if t.hit.is_some() {
        el().key("onb/tour/tick").size(26.0, 26.0).radius(13.0).center().fill(PAL.accent).ml(4.0)
            .role("status").label("Done")
            .child(icon(&icons::CHECK, 14.0).color(PAL.ink))
            .transition(Transition::scale(0.6).combined(Transition::opacity()))
    } else {
        text("Try it").key("onb/tour/try").font(SMALL13).color(PAL.dim).ml(4.0)
    };
    r.child(status)
}

/// The closing card's list: number keys, Settings, every shortcut (and the menu bar on macOS).
fn more(s: &State, t: &Tour, w: f32, reduced: bool) -> El {
    let local = s.local_only();
    let mut list = col().key("onb/tour/more").w(w.min(420.0)).gap(8.0).mt(16.0);
    let line = |key: &'static str, caps: El, words: String, lit: bool| {
        row().key(key).w_full().h(40.0).px(12.0).gap(12.0).center_y().radius(10.0)
            .fill(if lit { PAL.accent.alpha(0.14) } else { ov(0.04) })
            .child(caps)
            .child(text(words).font(BODY14).color(PAL.text2).truncate().grow(1.0).min_w(0.0))
    };
    // Number keys: what 1… open now (pages, local tabs, or pins).
    let last = if local && s.settings.number_keys == crate::settings::NumberKeys::Pages { keys::LOCAL_PAGES.len() as u8 } else { 9 };
    let names: Vec<String> = (1..=last).filter_map(|n| keys::slot_target(s, n)).collect();
    if let (Some(a), Some(b), false) = (keys::slot_caps(s, 1), keys::slot_caps(s, last), names.is_empty()) {
        let lit = t.lit && matches!(t.hit, Some(Action::Slot(_)));
        let caps = row().gap(6.0).center_y()
            .child(caps_el("onb/tour/caps/slot-1".into(), &a, lit, reduced))
            .child(text("to").font(SMALL13).color(PAL.nav_text))
            .child(caps_el(format!("onb/tour/caps/slot-{last}"), &b, lit, reduced));
        let words = match s.settings.number_keys {
            crate::settings::NumberKeys::Pins => "Your pins, in order".to_string(),
            crate::settings::NumberKeys::Pages if names.len() > 3 && !local => format!("{}\u{2026}", names[..3].join(", ")),
            crate::settings::NumberKeys::Pages => names.join(", "),
        };
        list = list.child(line("onb/tour/more/numbers", caps, words, lit));
    }
    for (a, key, words) in [
        (Action::Settings, "onb/tour/more/settings", "Settings"),
        (Action::Shortcuts, "onb/tour/more/shortcuts", "Every shortcut, and where to change them"),
    ] {
        if let Some(caps) = keys::action_caps(s, a) {
            let lit = t.lit && t.hit == Some(a);
            list = list.child(line(key, caps_el(format!("onb/tour/caps/{}", a.id()), &caps, lit, reduced), words.to_string(), lit));
        }
    }
    if cfg!(target_os = "macos") {
        list = list.child(text("They\u{2019}re in the menu bar too.").key("onb/tour/menubar").font(SMALL13).color(PAL.dim).mt(2.0));
    }
    list
}

// ---------------------------------------------------------------- the small picture

/// A rounded bar standing in for a line of text.
fn bar(w: f32, h: f32, c: rune::Color) -> El {
    el().w(w).h(h).radius(h / 2.0).fill(c)
}

/// The app in small: sidebar, content, right sidebar and player bar, with Now Playing and a
/// Settings sheet over them. Sizes follow `m`; Rune springs between them.
fn mini(m: Mini, local: bool, w: f32, h: f32, reduced: bool) -> El {
    let bar_h = (h * 0.17).round().max(26.0);
    let side_w = if m.folded { (w * 0.075).round() } else { (w * 0.23).round() };
    let right_full = (w * 0.27).round();
    let right_w = if m.right.is_some() { right_full } else { 0.0 };
    let pin = Some(0.0);

    // Sidebar: a few entries; a number key lights its own.
    let entries = if local { 4 } else { 6 };
    let side = col().key("mini/side").w(side_w).h_full().fill(PAL.sidebar).pt(12.0).px(8.0).gap(7.0).clip_rounded(0.0)
        .children((0..entries).map(|n| {
            let lit = m.slot.is_some_and(|s| (s as usize).saturating_sub(1).min(entries - 1) == n);
            row().key(format!("mini/side/{n}")).h(14.0).w_full().px(4.0).gap(5.0).center_y().radius(4.0)
                .fill(if lit { PAL.accent.alpha(0.28) } else { ov(0.0) })
                .child(el().size(6.0, 6.0).radius(2.0).fill(if lit { PAL.accent } else { ov(0.28) }))
                .child(el().h(5.0).grow(1.0).min_w(0.0).radius(2.5).fill(ov(if lit { 0.4 } else { 0.14 })))
        }));

    // Content: the page's title and a grid of covers; Find drops a filter field in on top.
    let tile_rows = 2;
    let mut content = col().key("mini/content").grow(1.0).min_w(0.0).h_full().padding(12.0).gap(8.0).clip_rounded(0.0);
    if m.filter {
        content = content.child(
            row().key("mini/filter").w_full().h(16.0).px(6.0).gap(5.0).center_y().radius(8.0).fill(ov(0.06)).border(1.0, PAL.accent)
                .child(icon(&icons::SEARCH, 8.0).color(PAL.accent))
                .child(bar(28.0, 4.0, ov(0.3)))
                .transition(Transition::offset(0.0, -10.0).combined(Transition::opacity())),
        );
    }
    content = content.child(bar((w * 0.16).round(), 8.0, ov(0.24)).key("mini/heading"));
    for r in 0..tile_rows {
        content = content.child(
            row().key(format!("mini/tiles/{r}")).w_full().gap(7.0).grow(1.0).min_h(0.0)
                .children((0..4).map(|_| el().grow(1.0).h_full().min_w(0.0).radius(5.0).fill(ov(0.07)))),
        );
    }

    // Right sidebar: Lyrics | Up Next, then lines or queued songs.
    let pane = m.right.or(m.tab).unwrap_or(if local { Pane::Queue } else { Pane::Lyrics });
    let tabs = row().gap(5.0).children(
        [Pane::Lyrics, Pane::Queue].into_iter().filter(|p| !(local && *p == Pane::Lyrics)).map(|p| {
            let on = p == pane;
            el().h(10.0).w(30.0).radius(5.0).fill(if on { PAL.accent.alpha(0.5) } else { ov(0.1) })
        }),
    );
    let body = match pane {
        Pane::Lyrics => col().key("mini/right/lyrics").gap(7.0).mt(4.0).children(
            [0.8, 0.62, 0.9, 0.5, 0.72].into_iter().enumerate().map(|(n, f)| bar((right_full - 16.0) * f, 6.0, if n == 1 { ov(0.6) } else { ov(0.14) })),
        ),
        Pane::Queue => col().key("mini/right/queue").gap(7.0).mt(4.0).children((0..4).map(|n| {
            row().gap(6.0).center_y()
                .child(el().size(14.0, 14.0).radius(3.0).fill(ov(0.1)))
                .child(bar((right_full - 36.0) * [0.8, 0.6, 0.7, 0.5][n], 5.0, ov(0.16)))
        })),
    };
    let right = col().key("mini/right").w(right_w).h_full().fill(PAL.sidebar).clip_rounded(0.0)
        .child(col().w(right_full).padding(8.0).gap(8.0).child(tabs).child(col().child_exit(Transition::opacity()).child(body)));

    // Player bar: cover, the song (slides on a skip), the controls, the volume.
    let dir = if m.dir < 0 { -1.0 } else { 1.0 };
    let song = col().key(format!("mini/song/{}", m.song)).gap(4.0)
        .transition(Transition::offset(14.0 * dir, 0.0).combined(Transition::opacity()))
        .child(bar([46.0, 38.0, 54.0][m.song.rem_euclid(3) as usize], 6.0, ov(0.45)))
        .child(bar([30.0, 36.0, 26.0][m.song.rem_euclid(3) as usize], 5.0, ov(0.18)));
    let art = el().key("mini/art").size(bar_h - 12.0, bar_h - 12.0).radius(4.0)
        .fill([PAL.accent.alpha(0.55), PAL.accent.alpha(0.35), PAL.accent.alpha(0.75)][m.song.rem_euclid(3) as usize]);
    let play = el().key(if m.playing { "mini/pause" } else { "mini/play" }).size(18.0, 18.0).radius(9.0).center().fill(PAL.text)
        .child(icon(if m.playing { &icons::PAUSE } else { &icons::PLAY }, 9.0).color(PAL.bg))
        .transition(Transition::scale(0.7).combined(Transition::opacity()));
    let controls = row().key("mini/controls").gap(10.0).center_y().child_exit(Transition::opacity())
        .child(icon(&icons::PREV, 9.0).color(PAL.text2))
        .child(play)
        .child(icon(&icons::NEXT, 9.0).color(PAL.text2));
    let player = row().key("mini/bar").w_full().h(bar_h).px(8.0).gap(8.0).center_y().fill(PAL.player)
        .child(art)
        .child(el().key("mini/song").w(60.0).child_exit(Transition::offset(-14.0 * dir, 0.0).combined(Transition::opacity())).child(song))
        .child(el().grow(1.0))
        .child(controls)
        .child(el().grow(1.0))
        .child(bar(40.0, 4.0, ov(0.18)).mr(6.0));

    let mut root = col().key("onb/tour/mini").w(w).h(h).radius(14.0).fill(PAL.bg).border(1.0, PAL.field_border)
        .clip_rounded(14.0).shadow(6.0, 24.0, shadow(0.25))
        .role("img").label("A small picture of Uzumaki")
        .child(row().w_full().grow(1.0).min_h(0.0).child(side).child(content).child(right))
        .child(player);

    // Overlays: Now Playing slides up over everything; Settings opens as a sheet.
    let mut over = el().key("mini/over").pin(pin, pin, pin, pin).child_exit(Transition::offset(0.0, h).combined(Transition::opacity()));
    if m.now_playing {
        over = over.child(
            row().key("mini/np").pin(pin, pin, pin, pin).padding(18.0).gap(16.0).center().fill(PAL.np_base)
                .transition(Transition::offset(0.0, h).combined(Transition::opacity()))
                .child(el().size(h * 0.48, h * 0.48).radius(8.0).fill(PAL.accent.alpha(0.6)))
                .child(col().gap(9.0).children(
                    [0.9, 0.7, 1.0, 0.6].into_iter().enumerate().map(|(n, f)| bar(90.0 * f, 7.0, if n == 1 { ov(0.7) } else { ov(0.18) })),
                )),
        );
    }
    if let Some(sheet) = m.sheet {
        let rows = 5;
        let lit = if sheet == Sheet::Keyboard { 3 } else { 0 };
        over = over.child(
            el().key("mini/sheet-dim").pin(pin, pin, pin, pin).center().fill(rune::Color::BLACK.alpha(0.35))
                .child(
                    row().key("mini/sheet").w((w * 0.62).round()).h((h * 0.66).round()).radius(10.0).fill(PAL.player).border(1.0, PAL.field_border)
                        .padding(10.0).gap(10.0)
                        .transition(Transition::scale(0.94).combined(Transition::opacity()))
                        .child(col().w(70.0).gap(6.0).children((0..rows).map(|n| {
                            let on = n == lit;
                            row().h(12.0).w_full().px(4.0).gap(4.0).center_y().radius(4.0).fill(if on { PAL.accent.alpha(0.28) } else { ov(0.0) })
                                .child(if n == 3 { icon(&icons::KEYBOARD, 8.0).color(if on { PAL.accent } else { PAL.dim }) } else { el().size(6.0, 6.0).radius(2.0).fill(ov(0.25)) })
                                .child(el().h(4.0).grow(1.0).radius(2.0).fill(ov(0.16)))
                        })))
                        .child(col().grow(1.0).gap(8.0).children((0..4).map(|n| {
                            row().w_full().gap(6.0).center_y()
                                .child(bar(50.0 + 10.0 * (n % 2) as f32, 5.0, ov(0.2)))
                                .child(el().grow(1.0))
                                .child(el().size(18.0, 9.0).radius(3.0).fill(if sheet == Sheet::Keyboard { ov(0.16) } else { ov(0.1) }))
                        }))),
                )
                .transition(Transition::opacity()),
        );
    }
    root = root.child(over);
    let root = root.cursor(Cursor::Default);
    if reduced { root.instant_changes() } else { root }
}
