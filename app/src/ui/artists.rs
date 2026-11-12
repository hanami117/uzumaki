//! Library > Artists: a virtualised list with an A-Z jump strip.

use std::cell::RefCell;
use std::rc::Rc;

use rune::{Cursor, El, GrowthSpec, ScrollSpec, Tx, col, el, row, text};

use super::browse::not_ready;
use super::cards::artist_row;
use super::songs_table::fold;
use super::widgets::{heading, muted, status};
use super::PAGE_PX;
use crate::state::{Load, Route, State};
use crate::theme::*;

/// Row slot height (the row is 52 plus 2 of padding either side), exact so letters can jump.
const ROW_H: f32 = 56.0;
const LETTERS: &str = "#ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Artists sorted by folded name, and where each letter starts.
pub struct Index {
    order: Vec<usize>,
    /// Row of the first artist of each of `LETTERS` (or of the next letter present).
    starts: Vec<usize>,
}

type Key = (usize, String, String);

/// Cached index, rebuilt only when the list changes.
#[derive(Default)]
pub struct Cache(RefCell<Option<(Key, Rc<Index>)>>);

fn bucket(name: &str) -> char {
    match fold(name).chars().find(|c| c.is_alphanumeric()) {
        Some(c) if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
        _ => '#',
    }
}

/// The artists listed: the library's, or the music folders' on local only mode's Artists.
fn items(s: &State) -> &[crate::models::Artist] {
    if s.route == Route::LocalArtists { &s.local_lib.view.artists } else { &s.library_artists.items }
}

fn index(s: &State) -> Rc<Index> {
    let items = items(s);
    let first = items.first().map_or("", |a| a.id.as_str());
    let last = items.last().map_or("", |a| a.id.as_str());
    if let Some((k, v)) = s.ui.artists.0.borrow().as_ref() {
        if k.0 == items.len() && k.1 == first && k.2 == last {
            return v.clone();
        }
    }
    let key: Key = (items.len(), first.to_string(), last.to_string());
    let mut keyed: Vec<(String, usize)> = items.iter().enumerate().map(|(i, a)| (fold(&a.name), i)).collect();
    keyed.sort();
    let order: Vec<usize> = keyed.into_iter().map(|(_, i)| i).collect();
    let mut starts = vec![order.len(); LETTERS.len()];
    for (row, &i) in order.iter().enumerate().rev() {
        let b = bucket(&items[i].name);
        if let Some(l) = LETTERS.find(b) {
            starts[l] = row;
        }
    }
    // A letter with no artists jumps to the next one that has some.
    for l in (0..starts.len().saturating_sub(1)).rev() {
        starts[l] = starts[l].min(starts[l + 1]);
    }
    let v = Rc::new(Index { order, starts });
    *s.ui.artists.0.borrow_mut() = Some((key, v.clone()));
    v
}

fn strip() -> El {
    let mut c = col().key("az").w(22.0).items_center().py(8.0).mr(8.0);
    for (l, ch) in LETTERS.chars().enumerate() {
        c = c.child(
            el().key(format!("az-{ch}")).role("button").label(ch.to_string()).w(22.0).h(15.0).center().radius(4.0)
                .hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
                .on_press_tx(move |s: &mut State, tx: &mut Tx| {
                    let row = index(s).starts[l];
                    tx.scroll_to_y("artists-scroll", row as f32 * ROW_H);
                })
                .child(text(ch.to_string()).font(rune::TextStyle::new(10.0, 600.0)).color(PAL.accent)),
        );
    }
    col().h_full().justify_center().child(c)
}

pub fn page(s: &State, win_h: f32) -> El {
    let local = s.route == Route::LocalArtists;
    let n = items(s).len();
    let sub = match n {
        0 => String::new(),
        _ if local => format!("{} artists in your music folders", super::local_files::thousands(n)),
        _ => format!("{n} artists in your library"),
    };
    let head = col().w_full().px(PAGE_PX).pt(28.0).pb(16.0).gap(4.0).child(heading("Artists")).child(muted(sub));
    let mut page = col().key("artists").grow(1.0).min_h(0.0).w_full().child(head);
    if local && n == 0 {
        return page.child(super::local_files::placeholder(s).unwrap_or_else(col));
    }
    if n == 0 {
        let msg = match &s.library_artists.load {
            Load::Ready(_) => status("No artists in your library.", false),
            other => not_ready(s, other, "your artists"),
        };
        return page.child(col().w_full().px(PAGE_PX).child(msg));
    }
    let idx = index(s);
    let rows = col().key("rows").growth(GrowthSpec::default())
        .virtual_rows(
            idx.order.len(),
            ROW_H,
            |s: &State, i| {
                let idx = index(s);
                let id = idx.order.get(i).and_then(|&k| items(s).get(k)).map_or("", |a| a.id.as_str());
                format!("a{id}").into()
            },
            move |s: &State, i| {
                let idx = index(s);
                match idx.order.get(i).and_then(|&k| items(s).get(k)) {
                    Some(a) => row().w_full().px(PAGE_PX - 12.0).py(2.0).child(artist_row(format!("artist-{}", a.id), a)),
                    None => col().h(ROW_H),
                }
            },
        )
        .virtual_rows_version((n, items(s).first().map(|a| a.id.clone()), local, s.local_lib.generation));
    let list = col().key("artists-scroll").grow(1.0).min_h(0.0).min_w(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
    let mut body = row().grow(1.0).min_h(0.0).w_full().child(list);
    // The strip needs ~410pt; skip it in short windows.
    if win_h >= 640.0 {
        body = body.child(strip());
    }
    page = page.child(body);
    page
}
