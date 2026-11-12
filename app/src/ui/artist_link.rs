//! An artist's name as a link to the artist's page (album header, song tables).

use rune::{Cursor, Drag, El, Phase, Tx, row};

use super::ctx_menu;
use crate::models::Artist;
use crate::state::{Route, State};

/// Opens `a`'s page.
pub fn open(s: &mut State, a: &Artist) {
    s.navigate(Route::Artist { id: a.id.clone(), name: a.name.clone() });
}

/// Makes `name_text` (the name, already styled) a link when `artists` is not empty; it comes back
/// untouched otherwise, with no hover and no pointer. One artist: a click opens the page. Several
/// ("A & B"): a click opens a small menu of them at the pointer. On hover the text turns to
/// `hover_color` (painted by Rune, no state). Pressing it does not select or play the row it
/// sits in (a child's own press wins).
pub fn link(key: String, name_text: El, artists: &[Artist], hover_color: rune::Color) -> El {
    if artists.is_empty() {
        return name_text;
    }
    let wrap = row().key(key).role("link").focusable(true).min_w(0.0).hover_group().cursor(Cursor::Pointer)
        .child(name_text.group_color(hover_color));
    // The outer row takes whatever width the layout gives; only the name itself is the link.
    let outer = |wrap: El| row().min_w(0.0).child(wrap);
    outer(match artists {
        [one] => {
            let one = one.clone();
            wrap.label(one.name.clone()).on_press(move |s: &mut State| open(s, &one))
        }
        many => {
            let many = many.to_vec();
            let label = many.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
            wrap.label(label).on_drag(move |s: &mut State, d: &mut Drag, _tx: &mut Tx| {
                if d.phase == Phase::Up && !d.cancelled() {
                    ctx_menu::open_artist_choice(s, d.window_point(), &many);
                }
            })
        }
    })
}
