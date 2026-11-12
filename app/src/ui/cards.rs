//! Radio station cards (regular and hero) and the artist row.

use rune::{Cursor, El, Tx, col, el, row, text};

use super::icons;
use super::widgets::{Size, art, avatar};
use crate::models::{Artist, Station};
use crate::state::{Route, State};
use crate::theme::*;

fn live_badge() -> El {
    row().px(7.0).h(18.0).radius(9.0).center().fill(PAL.accent).pin(Some(8.0), Some(8.0), None, None)
        .child(text("LIVE").font(rune::TextStyle::new(10.0, 600.0).tracking(0.6)).color(PAL.ink))
}

/// Station card of width `w` (plays on click, right-click pins).
pub fn station_card(s: &State, st: &Station, w: f32) -> El {
    let (s1, s2) = (st.clone(), st.clone());
    let mut cover = art(s, st.artwork.as_ref(), &st.name, Size::Fill, 8.0, 22.0);
    if st.live {
        cover = cover.child(live_badge());
    }
    col().key(format!("station-{}", st.id)).role("button").label(st.name.clone()).focusable(true).w(w).padding(8.0).gap(8.0)
        .radius(12.0).hover(PAL.card_hover).press(crate::theme::ov(0.04)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.play_station(&s1))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| super::ctx_menu::open_station(s, &s2, [p.x, p.y]))
        .child(cover)
        .child(
            col().gap(2.0).min_w(0.0)
                .child(text(st.name.clone()).font(CARD_TITLE).color(PAL.text).truncate())
                .child(text(if st.live { "Live radio" } else { "Station" }).font(SMALL13).color(PAL.muted).truncate()),
        )
}

/// A large live-radio card: cover with the name over a scrim.
pub fn hero_card(s: &State, st: &Station, w: f32) -> El {
    let (s1, s2) = (st.clone(), st.clone());
    let mut cover = art(s, st.artwork.as_ref(), &st.name, Size::Fill, 14.0, 0.0).aspect_ratio(1.7).clip_rounded(14.0);
    cover = cover
        .child(el().h(90.0).pin(Some(0.0), None, Some(0.0), Some(0.0)).gradient(rune::Color::TRANSPARENT, rune::Color::BLACK.alpha(0.7)))
        .child(
            row().pin(Some(14.0), None, Some(14.0), Some(12.0)).gap(10.0).center_y()
                .child(
                    col().grow(1.0).min_w(0.0).gap(1.0)
                        .child(text(st.name.clone()).font(rune::TextStyle::new(18.0, 600.0)).color(rune::Color::WHITE).truncate())
                        .child(text("Live radio").font(SMALL13).color(rune::Color::WHITE.alpha(0.8))),
                )
                .child(
                    el().size(36.0, 36.0).radius(18.0).center().fill(rune::Color::WHITE.alpha(0.92))
                        .child(rune::icon(&icons::PLAY, 15.0).color(PAL.ink)),
                ),
        );
    if st.live {
        cover = cover.child(live_badge());
    }
    el().key(format!("hero-{}", st.id)).role("button").label(st.name.clone()).focusable(true).w(w).radius(14.0)
        .hover(rune::Color::WHITE.alpha(0.05)).press_scale(0.985).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.play_station(&s1))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| super::ctx_menu::open_station(s, &s2, [p.x, p.y]))
        .child(cover)
}

/// One artist: avatar and name. Click opens the artist; right-click pins.
pub fn artist_row(key: String, a: &Artist) -> El {
    let (a1, a2) = (a.clone(), a.clone());
    row().key(key).role("button").label(a.name.clone()).focusable(true).w_full().min_h(52.0).px(12.0).gap(14.0).center_y()
        .radius(10.0).hover(PAL.row_hover).press(crate::theme::ov(0.04)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.navigate(Route::Artist { id: a1.id.clone(), name: a1.name.clone() }))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| super::ctx_menu::open_artist(s, &a2, [p.x, p.y]))
        .child(avatar(&a.name, 38.0))
        .child(text(a.name.clone()).font(TRACK_TITLE).color(PAL.text2).truncate().grow(1.0).min_w(0.0))
}
