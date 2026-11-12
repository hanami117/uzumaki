//! Listen now: one card grid per shelf.

use rune::{El, ScrollSpec, col};

use super::PAGE_PX;
use super::widgets::{Grid, card_row, heading, muted, status, subheading};
use crate::state::{Load, SHELF_TITLES, State};

pub fn page(s: &State, grid: Grid) -> El {
    let mut body = col().w_full().px(PAGE_PX).pt(28.0).pb(40.0)
        .child(heading("Listen now").mb(4.0))
        .child(muted("Picked from your recent listening").mb(18.0));

    for (i, shelf) in s.shelves.iter().enumerate() {
        let mut section = col().w_full().mb(20.0);
        match shelf {
            Load::Ready(sh) => {
                section = section.child(subheading(sh.title.clone()).mb(6.0));
                if sh.items.is_empty() {
                    section = section.child(status("Nothing here yet.", false));
                }
                for r in 0..sh.items.len().div_ceil(grid.n) {
                    section = section.child(card_row(s, &sh.items, grid, r).mx(-8.0));
                }
            }
            other => {
                section = section.child(subheading(SHELF_TITLES[i]).mb(6.0)).child(match other {
                    Load::Loading => status("Loading\u{2026}", false),
                    Load::Failed(e) => status(format!("Could not load: {e}"), true),
                    _ if !s.session.authorized => status("Sign in to see your music here.", false),
                    _ => status("Nothing here yet.", false),
                });
            }
        }
        body = body.child(section);
    }

    col().key("home").grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(body)
}
