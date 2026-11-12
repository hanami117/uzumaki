//! The Ctrl+F filter field above a list.

use rune::{Cursor, El, Key, KeyEvent, TextStyle, Tx, el, icon, row, text, text_input};

use super::{PAGE_PX, icons, lists};
use crate::state::State;
use crate::theme::*;

/// Shows the filter for this page and focuses it.
pub fn open(s: &mut State) {
    if let Some(id) = lists::filter_list_id(s) {
        let f = &mut s.ui.filter;
        if f.list != id {
            f.edit.clear();
        }
        f.open = true;
        f.list = id;
        f.edit.focused = true;
    }
}

/// Clears and hides it.
pub fn close(s: &mut State) {
    s.ui.filter.open = false;
    s.ui.filter.edit.clear();
}

/// The field with a "N of M" count, or nothing when the filter isn't showing.
pub fn bar(s: &State, shown: usize, total: usize) -> Option<El> {
    if !lists::filter_active(s) {
        return None;
    }
    let counts = if s.ui.filter.edit.text.trim().is_empty() { String::new() } else { format!("{shown} of {total}") };
    let field = row().key("filter-box").role("textbox").grow(1.0).max_w(420.0).h(34.0).px(10.0).gap(8.0).center_y().radius(8.0)
        .fill(PAL.field).border(1.0, PAL.field_border).cursor(Cursor::Text)
        .child(icon(&icons::SEARCH, 14.0).color(PAL.dim))
        .child(
            text_input(&s.ui.filter.edit, TextStyle::new(13.5, 400.0))
                .key("filter").grow(1.0).single_line().color(PAL.text).placeholder("Filter by title, artist or album", PAL.dim)
                .on_edit(|s: &mut State| &mut s.ui.filter.edit)
                .on_submit(|_s: &mut State, tx: &mut Tx| tx.blur())
                // The field would swallow Esc (it just drops focus), so close from here.
                .on_input_key(|s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
                    if super::keys::track_focus(s, ev, tx) {
                        return true; // the marker after a Tab (`keys::track_focus`)
                    }
                    if ev.key == Key::Escape {
                        close(s);
                        tx.blur();
                        return true;
                    }
                    false
                }),
        )
        .child(text(counts).font(SMALL12).color(PAL.dim));
    let close_btn = el().key("filter-close").role("button").label("Close filter").focusable(true).size(28.0, 28.0).radius(14.0).center()
        .hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
        .on_press(close)
        .child(icon(&icons::CLOSE, 14.0).color(PAL.muted));
    Some(row().key("filter-bar").w_full().px(PAGE_PX).pb(10.0).gap(8.0).center_y().child(field).child(close_btn))
}
