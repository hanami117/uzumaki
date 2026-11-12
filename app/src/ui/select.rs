//! Row selection in song lists, and the handlers every song row shares: click selects,
//! double-click / Enter plays, right-click and "…" open the context menu.
//!
//! Rows are named by row key (`lists::keys_of`), not by song id: a playlist can hold the same
//! song twice, and an action on one copy must not reach the other.

use std::collections::HashSet;

use rune::{Cursor, Drag, El, Key, Mods, Phase, Tx, el, icon};

use super::{dnd, icons, lists, menu};
use crate::models::Song;
use crate::state::State;
use crate::theme::*;

/// Selected rows of one list. Keyed by list identity and row key (never display position), so
/// sorting and filtering don't disturb it. Rows only show it while `list` is the page's list.
#[derive(Default)]
pub struct Selection {
    pub list: String,
    /// Row keys (`lists::keys_of`) of the selected rows.
    pub keys: HashSet<String>,
    /// Where a Shift range starts (the last plain click or arrow move).
    anchor: Option<String>,
    /// The row the arrows move from: the last clicked or moved-to row.
    lead: Option<String>,
    /// Bumped on every change: virtual lists put it in their version so rows rebuild.
    pub rev: u64,
}

impl Selection {
    pub fn has(&self, list: &str, key: &str) -> bool {
        self.list == list && self.keys.contains(key)
    }

    /// Forgets everything (call when the route changes).
    pub fn clear(&mut self) {
        *self = Self { rev: self.rev + 1, ..Self::default() };
    }
}

/// Row background: accent at low alpha when selected.
pub fn row_fill(s: &State, list: &str, key: &str) -> rune::Color {
    if s.ui.sel.has(list, key) { PAL.accent.alpha(PAL.sel_alpha) } else { rune::Color::TRANSPARENT }
}

/// Row keys of the page's list in display order.
fn visible_keys(s: &State) -> Vec<String> {
    lists::visible_rows(s).into_iter().map(|(k, _)| k).collect()
}

/// A click on row `key`, with the modifiers held.
pub fn click(s: &mut State, list: &str, key: &str, mods: Mods) {
    if lists::list_id(s).as_deref() != Some(list) {
        return;
    }
    let toggle = mods.cmd || mods.ctrl;
    s.ui.sel.rev += 1;
    if s.ui.sel.list != list {
        s.ui.sel.clear();
        s.ui.sel.list = list.to_string();
    }
    if mods.shift && s.ui.sel.anchor.is_some() {
        let rows = visible_keys(s);
        let pos = |k: &str| rows.iter().position(|x| x == k);
        let (a, b) = (s.ui.sel.anchor.as_deref().and_then(pos), pos(key));
        if let (Some(a), Some(b)) = (a, b) {
            let sel = &mut s.ui.sel;
            if !toggle {
                sel.keys.clear();
            }
            sel.keys.extend(rows[a.min(b)..=a.max(b)].iter().cloned());
            sel.lead = Some(key.to_string());
            return;
        }
    }
    let sel = &mut s.ui.sel;
    if toggle {
        if !sel.keys.remove(key) {
            sel.keys.insert(key.to_string());
        }
    } else {
        sel.keys.clear();
        sel.keys.insert(key.to_string());
    }
    sel.anchor = Some(key.to_string());
    sel.lead = Some(key.to_string());
}

/// An arrow-key move in the page's song list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Rows down (positive) or up.
    By(isize),
    First,
    Last,
}

/// Moves the selection with the keyboard: to the row `mv` points at from the lead row (the
/// first or last row when nothing is selected yet). With `extend`, selects everything from the
/// anchor to there, as Shift-click does. Returns the new lead row's key, to scroll to.
pub fn move_lead(s: &mut State, mv: Move, extend: bool) -> Option<String> {
    let list = lists::list_id(s)?;
    let rows = visible_keys(s);
    let last = rows.len().checked_sub(1)?;
    let sel = &s.ui.sel;
    let lead = if sel.list == list { sel.lead.clone().or_else(|| sel.anchor.clone()) } else { None };
    let cur = lead.as_deref().and_then(|k| rows.iter().position(|x| x == k));
    let new = match (mv, cur) {
        (Move::First, _) => 0,
        (Move::Last, _) => last,
        (Move::By(d), Some(c)) => (c as isize + d).clamp(0, last as isize) as usize,
        (Move::By(d), None) => if d > 0 { 0 } else { last },
    };
    let new_key = rows[new].clone();
    let sel = &mut s.ui.sel;
    if sel.list != list {
        sel.clear();
        sel.list = list;
    }
    sel.rev += 1;
    if extend {
        let anchor = sel.anchor.clone().or(lead).unwrap_or_else(|| new_key.clone());
        let a = rows.iter().position(|x| *x == anchor).unwrap_or(new);
        sel.keys = rows[a.min(new)..=a.max(new)].iter().cloned().collect();
        sel.anchor = Some(anchor);
    } else {
        sel.keys = HashSet::from([new_key.clone()]);
        sel.anchor = Some(new_key.clone());
    }
    sel.lead = Some(new_key.clone());
    Some(new_key)
}

/// Key of the wrapper `wire` puts around every song row, for scrolling it into view.
pub fn mark_key(list: &str, key: &str) -> String {
    format!("sel:{list}:{key}")
}

pub fn select_all(s: &mut State) {
    let Some(list) = lists::list_id(s) else { return };
    let keys: HashSet<String> = visible_keys(s).into_iter().collect();
    let sel = &mut s.ui.sel;
    sel.anchor = None;
    sel.lead = None;
    sel.rev += 1;
    sel.list = list;
    sel.keys = keys;
}

/// The selected rows' songs in display order, one entry per row: a song selected in two rows
/// is listed twice (what Remove from Playlist needs to know).
pub fn selected_rows(s: &State) -> Vec<(usize, Song)> {
    if lists::list_id(s).as_deref() != Some(s.ui.sel.list.as_str()) {
        return Vec::new();
    }
    lists::visible_rows_at(s).into_iter().filter(|(_, k, _)| s.ui.sel.keys.contains(k)).map(|(i, _, x)| (i, x)).collect()
}

/// The selected songs in display order, each song once: selecting both copies of a song that a
/// playlist holds twice queues, adds or stars it once.
pub fn selected(s: &State) -> Vec<Song> {
    unique(selected_rows(s).into_iter().map(|(_, x)| x).collect())
}

/// `songs` without repeats of a song id, in order (first copy kept).
pub fn unique(songs: Vec<Song>) -> Vec<Song> {
    let mut seen = HashSet::new();
    songs.into_iter().filter(|x| seen.insert(x.id.clone())).collect()
}

/// Plays the page's list (as displayed, so filtered and sorted) from row `key`.
pub fn play_row(s: &mut State, list: &str, key: &str) {
    if lists::list_id(s).as_deref() != Some(list) {
        return;
    }
    let rows = lists::visible_rows(s);
    let Some(mut i) = rows.iter().position(|(k, _)| k == key) else { return };
    let mut songs: Vec<Song> = rows.into_iter().map(|(_, x)| x).collect();
    // Library and search lists can be huge: queue a window from the clicked row on.
    if songs.len() > 500 {
        songs = songs.split_off(i).into_iter().take(500).collect();
        i = 0;
    }
    let name = lists::name(s);
    s.play_list(&songs, i, &name);
}

/// Plays from the first selected row (Enter).
pub fn play_selected(s: &mut State) {
    let Some(list) = lists::list_id(s) else { return };
    if list != s.ui.sel.list {
        return;
    }
    let first = visible_keys(s).into_iter().find(|k| s.ui.sel.keys.contains(k));
    if let Some(key) = first {
        play_row(s, &list, &key);
    }
}

/// Right-click or "…": selects the row if it isn't selected, then opens the menu at `at`.
fn open_menu(s: &mut State, list: &str, key: &str, at: [f32; 2]) {
    if !s.ui.sel.has(list, key) {
        click(s, list, key, Mods::default());
    }
    menu::open(s, list, key, at);
}

fn on_row_key(s: &mut State, ev: &rune::KeyEvent, list: &str, id: &str) -> bool {
    if ev.mods.cmd || ev.mods.alt || ev.mods.ctrl {
        return false;
    }
    match &ev.key {
        Key::Enter => {
            if !s.ui.sel.has(list, id) {
                click(s, list, id, Mods::default());
            }
            play_selected(s);
            true
        }
        _ => false,
    }
}

/// Gives a song row its selection look and handlers, plus the hover "…" button. `key` is the
/// row's key in its list (`lists::row_key`). The row must be a `hover_group()` and have a key.
pub fn wire(s: &State, row: El, key: &str, list: &str) -> El {
    let (id1, id2, id3, id4, id5, id6) =
        (key.to_string(), key.to_string(), key.to_string(), key.to_string(), key.to_string(), key.to_string());
    let (l1, l2, l3, l4, l5, l6) =
        (list.to_string(), list.to_string(), list.to_string(), list.to_string(), list.to_string(), list.to_string());
    let more = el().key(format!("more-{key}")).role("button").label("More").focusable(true).size(28.0, 28.0).radius(14.0)
        .center().fill(PAL.nav_active).hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
        .group_opacity(0.0, 1.0).pin(None, Some(10.0), Some(76.0), None)
        .on_drag(move |s: &mut State, d: &mut Drag, _tx: &mut Tx| {
            if d.phase == Phase::Up && !d.cancelled() {
                let at = d.window_point();
                open_menu(s, &l5, &id5, at);
            }
        })
        .child(icon(&icons::MORE, 16.0).color(PAL.text2));
    // The hover overlay must be translucent or it hides the selection colour. The keyed wrapper
    // is what arrow keys scroll into view (`mark_key`).
    let mark = mark_key(list, key);
    let row = row.fill(row_fill(s, list, key)).hover(crate::theme::ov(0.05))
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            // The tail of a drag is not a click.
            if !s.ui.dnd.swallow_click() {
                click(s, &l1, &id1, tx.mods());
            }
            tx.instant();
        })
        .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| dnd::song_drag(s, d, tx, &l6, &id6))
        .on_double_press(move |s: &mut State| play_row(s, &l2, &id2))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| open_menu(s, &l3, &id3, [p.x, p.y]))
        .on_key(move |s: &mut State, ev, _tx: &mut Tx| on_row_key(s, ev, &l4, &id4))
        .child(more);
    el().key(mark).w_full().child(row)
}

/// The hover play button in a row's number / art column (row `key`): plays with a single click.
pub fn play_button(el: El, list: &str, key: &str) -> El {
    let (list, key) = (list.to_string(), key.to_string());
    el.cursor(Cursor::Pointer).on_press(move |s: &mut State| play_row(s, &list, &key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist_state() -> State {
        let mut s = State::offline();
        let scene = crate::scenes::SCENES.iter().find(|(n, _)| *n == "playlist").unwrap().1;
        scene(&mut s);
        s
    }

    fn ids(s: &State) -> Vec<String> {
        let mut v: Vec<String> = s.ui.sel.keys.iter().cloned().collect();
        v.sort();
        v
    }

    /// The visible order's ids, to name rows by position.
    fn order(s: &State) -> Vec<String> {
        lists::visible(s).into_iter().map(|x| x.id).collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn arrows_move_the_selection_and_shift_arrows_grow_and_shrink_it() {
        let mut s = playlist_state();
        let o = order(&s);
        let n = o.len();
        // Nothing selected: Down starts at the top, Up at the bottom.
        assert_eq!(move_lead(&mut s, Move::By(1), false).as_deref(), Some(o[0].as_str()));
        assert_eq!(ids(&s), [o[0].clone()]);
        move_lead(&mut s, Move::By(-1), false);
        assert_eq!(ids(&s), [o[0].clone()], "stops at the first row");
        move_lead(&mut s, Move::By(1), false);
        move_lead(&mut s, Move::By(1), false);
        assert_eq!(ids(&s), [o[2].clone()]);
        // Shift: a range from where the plain moves left off; back past it flips the side.
        move_lead(&mut s, Move::By(1), true);
        move_lead(&mut s, Move::By(1), true);
        assert_eq!(ids(&s), sorted(o[2..=4].to_vec()));
        move_lead(&mut s, Move::By(-1), true);
        assert_eq!(ids(&s), sorted(o[2..=3].to_vec()));
        move_lead(&mut s, Move::By(-1), true);
        move_lead(&mut s, Move::By(-1), true);
        assert_eq!(ids(&s), sorted(o[1..=2].to_vec()));
        // A plain move collapses to one row, next to where the lead was.
        move_lead(&mut s, Move::By(1), false);
        assert_eq!(ids(&s), [o[2].clone()]);
        // Home / End, and Shift+End from the anchor.
        assert_eq!(move_lead(&mut s, Move::Last, false).as_deref(), Some(o[n - 1].as_str()));
        move_lead(&mut s, Move::First, false);
        move_lead(&mut s, Move::Last, true);
        assert_eq!(ids(&s).len(), n);
    }

    #[test]
    fn arrows_continue_from_the_clicked_row() {
        let mut s = playlist_state();
        let list = lists::list_id(&s).unwrap();
        let o = order(&s);
        click(&mut s, &list, &o[3], Mods::default());
        move_lead(&mut s, Move::By(1), false);
        assert_eq!(ids(&s), [o[4].clone()]);
        click(&mut s, &list, &o[6], Mods { shift: true, ..Mods::default() });
        assert_eq!(ids(&s), sorted(o[4..=6].to_vec()));
        move_lead(&mut s, Move::By(1), true);
        assert_eq!(ids(&s), sorted(o[4..=7].to_vec()), "Shift+Down extends from the Shift-click's end");
    }

    /// The real thing, headless: keys through `on_key`, and the selected row ends up on screen.
    #[test]
    fn arrow_keys_scroll_the_selected_row_into_view() {
        use rune::{Env, Key, KeyEvent, Runtime};
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "local-files").unwrap().1)(&mut s);
        let (w, h) = (1180.0, 640.0);
        let mut rt = Runtime::new(s, crate::ui::view, (w, h), Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(crate::ui::on_key);
        rt.frame(0.0);
        let settle = |rt: &mut Runtime<State>| {
            for _ in 0..90 {
                rt.frame(1.0 / 60.0);
            }
        };
        let press = |rt: &mut Runtime<State>, key: Key| rt.key(KeyEvent { key, mods: Mods::default() });
        let mut o = Vec::new();
        rt.update(|s, _| o = order(s));
        assert!(o.len() * 50 > h as usize * 2, "the list is taller than the window");
        let on_screen = |rt: &Runtime<State>, id: &str| {
            rt.find(&mark_key("local-files", id)).is_some_and(|(_, y)| y > 0.0 && y < h - 80.0)
        };

        press(&mut rt, Key::End);
        settle(&mut rt);
        assert!(on_screen(&rt, o.last().unwrap()), "End reveals the last song");
        press(&mut rt, Key::Home);
        settle(&mut rt);
        assert!(on_screen(&rt, &o[0]), "Home reveals the first");
        for _ in 0..20 {
            press(&mut rt, Key::Down);
            rt.frame(1.0 / 60.0);
        }
        settle(&mut rt);
        let mut sel = Vec::new();
        rt.update(|s, _| sel = ids(s));
        assert_eq!(sel, [o[20].clone()]);
        assert!(on_screen(&rt, &o[20]), "stepping down keeps the row in view");
    }

    #[test]
    fn click_ctrl_shift_and_select_all() {
        let mut s = playlist_state();
        let list = lists::list_id(&s).unwrap();
        let plain = Mods::default();
        let ctrl = Mods { cmd: true, ctrl: true, ..Mods::default() };
        let shift = Mods { shift: true, ..Mods::default() };

        click(&mut s, &list, "pl-1", plain);
        assert_eq!(ids(&s), ["pl-1"]);
        click(&mut s, &list, "pl-4", shift);
        assert_eq!(ids(&s), ["pl-1", "pl-2", "pl-3", "pl-4"]);
        click(&mut s, &list, "pl-2", ctrl);
        assert_eq!(ids(&s), ["pl-1", "pl-3", "pl-4"]);
        click(&mut s, &list, "pl-9", plain);
        assert_eq!(ids(&s), ["pl-9"]);
        select_all(&mut s);
        assert_eq!(s.ui.sel.keys.len(), 14);
        assert_eq!(selected(&s).len(), 14);
    }

    #[test]
    fn selection_only_applies_to_its_list_and_follows_the_filter() {
        let mut s = playlist_state();
        let list = lists::list_id(&s).unwrap();
        click(&mut s, &list, "pl-1", Mods::default());
        assert!(s.ui.sel.has(&list, "pl-1"));
        assert!(!s.ui.sel.has("lib-songs", "pl-1"));

        // Filter by artist, accent-insensitively; the visible list shrinks, the selection stays.
        s.ui.filter.open = true;
        s.ui.filter.list = list.clone();
        s.ui.filter.edit.text = "OKAFOR".into();
        let visible = lists::visible(&s);
        assert!(!visible.is_empty() && visible.len() < 14);
        assert!(visible.iter().all(|x| x.artist.contains("Okafor")));
        select_all(&mut s);
        assert_eq!(s.ui.sel.keys.len(), visible.len());
    }
}
