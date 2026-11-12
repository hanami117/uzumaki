//! A titled shelf that scrolls sideways (wheel / trackpad), with arrow buttons that show while
//! the pointer is over it.
//!
//! The view builds every item of every shelf, on screen or not, so only items near the window ask
//! for their covers (`UiState::near`, kept by each item's `on_layout`): a page of a dozen shelves
//! would otherwise hold hundreds of covers in memory (546 KB each on a 2× screen).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use rune::{Cursor, El, ScrollPos, Tx, col, el, icon, row};

use super::icons;
use super::widgets::subheading;
use crate::theme::*;

thread_local! {
    /// Per shelf: (scroll offset, max offset, viewport width), written by the scroll callback
    /// (which doesn't touch state) and read by the arrow buttons.
    static POS: RefCell<HashMap<String, [f32; 3]>> = RefCell::new(HashMap::new());
    /// The window's size in points, from the last view.
    static WINDOW: Cell<[f32; 2]> = const { Cell::new([1360.0, 860.0]) };
}

/// The view records the window's size here (`ui::view`).
pub fn set_window(size: [f32; 2]) {
    WINDOW.set(size);
}

/// Whether an item laid out at `r` (`[x, y, w, h]`, window points) is near the window: on it,
/// or within half the window's size of it, so covers are ready a little before they scroll in.
fn near(r: [f32; 4], win: [f32; 2]) -> bool {
    let (mx, my) = (win[0] * 0.5, win[1] * 0.5);
    r[0] + r[2] > -mx && r[0] < win[0] + mx && r[1] + r[3] > -my && r[1] < win[1] + my
}

fn arrow(key: String, shelf: String, dir: f32, left: bool) -> El {
    let ic = if left { &icons::CHEVRON_LEFT } else { &icons::CHEVRON_RIGHT };
    el().key(key).role("button").label(if left { "Scroll left" } else { "Scroll right" }).focusable(true).size(28.0, 28.0)
        .radius(14.0).center().fill(PAL.nav_active).hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
        .on_press_tx(move |_s: &mut crate::state::State, tx: &mut Tx| {
            let [off, max, vp] = POS.with_borrow(|m| m.get(&shelf).copied()).unwrap_or([0.0, 0.0, 600.0]);
            let to = (off + dir * (vp * 0.85)).clamp(0.0, max.max(0.0));
            tx.scroll_to(format!("{shelf}-row"), to, 0.0);
        })
        .child(icon(ic, 14.0).color(PAL.text2))
}

/// Whether an element laid out at `r` (window points) is on or near the window (`near`).
pub fn near_window(r: [f32; 4]) -> bool {
    near(r, WINDOW.get())
}

/// `count` fixed-width cards built by `item`. `key` is unique per page and shelf. Items far off
/// screen are built with `widgets::off_screen`.
pub fn carousel(s: &crate::state::State, key: &str, title: &str, count: usize, item: impl Fn(usize) -> El) -> El {
    carousel_with(s, key, title, None, count, item)
}

/// `carousel` with `extra` (e.g. a "See All" link) between the title and the arrows.
pub fn carousel_with(s: &crate::state::State, key: &str, title: &str, extra: Option<El>, count: usize, item: impl Fn(usize) -> El) -> El {
    let items = (0..count).map(|i| {
        let ik = format!("{key}-i{i}");
        let built = if s.ui.near.contains(&ik) { item(i) } else { super::widgets::off_screen(|| item(i)) };
        // Reports every frame of a scroll; only a change of side commits (and runs the view).
        el().key(ik.clone())
            .on_layout_tx(move |s: &mut crate::state::State, r: [f32; 4], tx: &mut Tx| {
                let n = near(r, WINDOW.get());
                if n == s.ui.near.contains(&ik) {
                    tx.live_only();
                } else if n {
                    s.ui.near.insert(ik.clone());
                } else {
                    s.ui.near.remove(&ik);
                }
            })
            .child(built)
    });
    let k = key.to_string();
    let mut head = row().w_full().gap(6.0).center_y().mb(6.0)
        .child(subheading(title.to_string()).grow(1.0).min_w(0.0).truncate());
    if let Some(x) = extra {
        head = head.child(x);
    }
    let head = head
        .child(el().group_opacity(0.0, 1.0).child(arrow(format!("{key}-l"), k.clone(), -1.0, true)))
        .child(el().group_opacity(0.0, 1.0).child(arrow(format!("{key}-r"), k.clone(), 1.0, false)));
    let body = row().key(format!("{key}-row")).w_full().ml(-8.0).scroll_x().scroll_edge_fade(20.0).items_start()
        .on_scroll_offset(move |p: ScrollPos| {
            POS.with_borrow_mut(|m| {
                m.insert(k.clone(), [p.offset[0], p.max[0], p.viewport[0]]);
            })
        })
        .child(row().gap(12.0).items_start().children(items));
    col().key(key.to_string()).w_full().mb(18.0).hover_group().child(head).child(body)
}

#[cfg(test)]
mod tests {
    use super::near;

    #[test]
    fn covers_load_on_and_near_the_window_only() {
        let win = [1360.0, 860.0];
        let card = |x: f32, y: f32| near([x, y, 176.0, 230.0], win);
        assert!(card(300.0, 200.0), "on screen");
        assert!(card(1500.0, 200.0) && card(300.0, 1200.0), "within half a window: ready before it scrolls in");
        assert!(card(-500.0, 200.0), "just scrolled past on the left");
        assert!(!card(2100.0, 200.0), "a shelf's far end");
        assert!(!card(300.0, 1400.0) && !card(300.0, -700.0), "shelves far below and above");
    }
}
