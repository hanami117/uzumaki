//! A catalog artist's page, laid out like Apple Music's (`crate::artist`): a header with the
//! artist's wide editorial picture fading into the page (or a round portrait), Play / Shuffle of
//! the top songs and the artist menu; Latest Release beside Top Songs (five per column); then
//! shelves newest first (Albums, Singles & EPs, Live Albums, Compilations, Appears On),
//! Playlists, Similar Artists and About. Empty sections are left out. Each shelf's See All opens
//! a grid (`shelf_page`). A library artist without a catalog match keeps the old albums grid.
//!
//! Every shelf is a `carousel`, so only cards near the window ask for covers. Shelves past their
//! loaded end show a small trigger that fetches Apple's next page when it comes near the window.

use rune::{Cursor, El, Tx, col, el, row, text};

use super::browse::{not_ready, scroll_page};
use super::carousel::{carousel, carousel_with, near_window};
use super::grids::{GridSrc, grid};
use super::widgets::{Grid, Size, art, avatar, card, heading, muted, pill, status, subheading};
use super::{PAGE_PX, ctx_menu, icons, lists, tracks};
use crate::artist::{ArtistCard, ArtistPage, Shelf, fmt_release};
use crate::models::Artist;
use crate::state::{Load, More, Route, State};
use crate::theme::*;

const CARD_W: f32 = 176.0;
/// Cards built per shelf; See All has the rest.
const MAX_CARDS: usize = 30;
/// Top songs per column.
const TOP_ROWS: usize = 5;
const LATEST_W: f32 = 200.0;
const ARTIST_W: f32 = 150.0;
/// The portrait of an artist without an editorial picture.
const PORTRAIT: f32 = 190.0;
/// Below this content width Latest Release sits above Top Songs.
const SIDE_BY_SIDE: f32 = 720.0;

fn page_of<'a>(s: &'a State, id: &str) -> Option<&'a ArtistPage> {
    match s.artist_pages.get(id) {
        Some(Load::Ready(Some(p))) => Some(p),
        _ => None,
    }
}

/// The top songs of the page at route id `id` (the page's song list, `lists::songs`).
pub fn top_songs<'a>(s: &'a State, id: &str) -> Option<&'a [crate::models::Song]> {
    page_of(s, id).map(|p| p.top_songs.as_slice()).filter(|t| !t.is_empty())
}

/// The song list id of an artist page's top songs.
pub fn list_id(id: &str) -> String {
    format!("artist-top:{id}")
}

pub fn page(s: &State, id: &str, name: &str, g: Grid, avail: f32) -> El {
    let load = s.artist_pages.get(id);
    let p = match load {
        Some(Load::Ready(Some(p))) => p,
        Some(Load::Ready(None)) => return super::grids::artist_page(s, id, name, g),
        other => {
            let head = row().w_full().px(PAGE_PX).pt(28.0).pb(16.0).child(heading(name.to_string()).min_w(0.0).truncate());
            let body = match other {
                Some(l @ (Load::Loading | Load::Failed(_))) => not_ready(s, l, "this artist"),
                _ => not_ready(s, &Load::<()>::Idle, "this artist"),
            };
            return col().key(format!("artist-page-{id}")).grow(1.0).min_h(0.0).w_full().child(head)
                .child(col().w_full().px(PAGE_PX).child(body));
        }
    };
    let shown_name = if p.name.is_empty() { name } else { &p.name };
    let mut inner = col().w_full().px(PAGE_PX).pt(8.0);
    if p.latest.is_some() || !p.top_songs.is_empty() {
        inner = inner.child(top_row(s, id, p, avail));
    }
    for shelf in Shelf::ALL {
        if !p.section(shelf).items.is_empty() {
            inner = inner.child(shelf_view(s, id, shown_name, p, shelf, avail));
        }
    }
    if !p.similar.is_empty() {
        let sim = &p.similar;
        inner = inner.child(carousel(s, "artist-similar", "Similar Artists", sim.len().min(MAX_CARDS), |i| similar_card(s, &sim[i])));
    }
    if let Some(bio) = &p.bio {
        inner = inner.child(
            col().key("artist-about").w_full().mb(18.0).gap(8.0)
                .child(subheading("About"))
                .child(text(bio.clone()).font(BODY14).color(PAL.muted).wrap().max_lines(3).max_w(760.0)),
        );
    }
    if p.is_empty() {
        inner = inner.child(status("Nothing from this artist in your country's catalog.", false));
    }
    let body = col().w_full().pb(40.0).child(header(s, id, shown_name, p, avail)).child(inner);
    scroll_page(&format!("artist-{id}"), body)
}

/// Play, Shuffle (both play the top songs) and the artist menu.
fn buttons(id: &str, name: &str, has_songs: bool) -> El {
    let mut r = row().key("artist-buttons").gap(10.0).mt(16.0).items_center();
    if has_songs {
        let (i1, n1, i2, n2) = (id.to_string(), name.to_string(), id.to_string(), name.to_string());
        r = r
            .child(pill("artist-play", "Play", &icons::PLAY, PAL.accent, PAL.ink).on_press(move |s: &mut State| {
                if let Some(t) = top_songs(s, &i1).map(<[_]>::to_vec) {
                    s.play_list(&t, 0, &n1);
                }
            }))
            .child(pill("artist-shuffle", "Shuffle", &icons::SHUFFLE, PAL.nav_active, PAL.text).on_press(move |s: &mut State| {
                if let Some(t) = top_songs(s, &i2).map(<[_]>::to_vec) {
                    s.shuffle_play(&t, &n2);
                }
            }));
    }
    let a = Artist { id: id.to_string(), name: name.to_string() };
    r.child(ctx_menu::more_button("artist-more", 44.0, move |s| ctx_menu::artist_items(s, &a)))
}

fn name_text(name: &str) -> El {
    text(name.to_string()).font(rune::TextStyle::new(44.0, 700.0).tracking(-0.9)).color(PAL.text).wrap().max_lines(2)
}

fn header(s: &State, id: &str, name: &str, p: &ArtistPage, avail: f32) -> El {
    let has_songs = !p.top_songs.is_empty();
    let Some(hero) = &p.hero else {
        // No editorial picture: a round portrait beside the name, like a collection header.
        let portrait = match &p.artwork {
            Some(a) => art(s, Some(a), name, Size::Fixed(PORTRAIT), PORTRAIT / 2.0, 0.0).clip_rounded(PORTRAIT / 2.0),
            None => avatar(name, PORTRAIT),
        };
        return row().key("artist-head").w_full().px(PAGE_PX).pt(20.0).pb(24.0).gap(28.0).items_end()
            .child(portrait.key("artist-portrait"))
            .child(
                col().grow(1.0).min_w(0.0)
                    .child(text("ARTIST").font(LABEL12).color(PAL.muted))
                    .child(name_text(name).mt(4.0))
                    .child(buttons(id, name, has_songs)),
            );
    };
    // The picture spans the content area edge to edge, as tall as its shape asks within limits.
    // It fades itself out towards the bottom (`clip_fade`), so whatever is behind the page shows
    // through with no seam (the theme's ground, or the background picture), and the name and
    // buttons sit on that ground in both themes. A light scrim inside the fade helps them read
    // where the picture is still strong.
    let full_w = avail + 2.0 * PAGE_PX;
    let h = (full_w / hero.aspect.max(1.0)).clamp(260.0, 440.0);
    let mut pic = el().key("artist-hero-art").w_full().h_full().fill(PAL.card_hover);
    if let Some(img) = s.art.get(&hero.art, hero.px) {
        pic = pic.pixels_fit(img.pixels(), rune::SurfaceFit::Fill);
    }
    let pic = pic.child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).gradient(PAL.bg.alpha(0.0), PAL.bg.alpha(0.55)));
    el().key("artist-hero").w_full().h(h)
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).clip_fade(0.0, 0.0, 0.0, h * 0.7).child(pic))
        .child(
            col().pin(Some(PAGE_PX), None, Some(PAGE_PX), Some(18.0)).min_w(0.0)
                .child(name_text(name))
                .child(buttons(id, name, has_songs)),
        )
}

/// A trigger at a shelf's loaded end: asks for Apple's next page when it comes near the window.
fn more_trigger(key: String, id: &str, what: More, w: f32, h: f32) -> El {
    let id = id.to_string();
    el().key(key).size(w, h).center()
        .on_layout_tx(move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
            if !(near_window(r) && s.artist_more(&id, what)) {
                tx.live_only();
            }
        })
        .child(text("\u{2026}").font(BODY14).color(PAL.dim))
}

fn top_row(s: &State, id: &str, p: &ArtistPage, avail: f32) -> El {
    let side = avail >= SIDE_BY_SIDE && p.latest.is_some() && !p.top_songs.is_empty();
    let latest = p.latest.as_ref().map(|c| {
        let date = p.latest_date.as_deref().map(fmt_release).unwrap_or_default();
        col().key("artist-latest").w(LATEST_W + 8.0).mb(18.0)
            .child(subheading("Latest Release").mb(6.0))
            .child(
                col().ml(-8.0)
                    .child(text(date).font(SECTION11).color(PAL.muted).px(8.0))
                    .child(card(s, c, LATEST_W)),
            )
    });
    let top = (!p.top_songs.is_empty()).then(|| {
        let n = p.top_songs.len();
        let top_w = if side { avail - LATEST_W - 8.0 - 28.0 } else { avail };
        let col_w = if top_w >= 640.0 { (top_w - 24.0) / 2.0 } else { top_w - 24.0 }.clamp(280.0, 480.0);
        let n_cols = n.div_ceil(TOP_ROWS);
        let more = p.top_next.is_some() && n < crate::artist::TOP_SONGS_MAX;
        let list = list_id(id);
        let id2 = id.to_string();
        el().grow(1.0).min_w(0.0).child(carousel(s, "artist-top", "Top Songs", n_cols + usize::from(more), move |c| {
            if c == n_cols {
                return more_trigger("artist-top-more".into(), &id2, More::Top, 48.0, 48.0);
            }
            col().w(col_w).children((c * TOP_ROWS..((c + 1) * TOP_ROWS).min(n)).map(|i| {
                tracks::song_row_compact(s, &p.top_songs[i], format!("top-{i}"), &list, &lists::row_key(s, i))
            }))
        }))
    });
    if side {
        row().w_full().gap(28.0).items_start().children(latest).children(top)
    } else {
        col().w_full().children(latest).children(top)
    }
}

fn see_all(id: &str, name: &str, shelf: Shelf) -> El {
    let (id, name) = (id.to_string(), name.to_string());
    el().key(format!("artist-see-all-{}", shelf.view())).role("button").label("See All").focusable(true).px(8.0).h(28.0)
        .center().radius(14.0).hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.navigate(Route::ArtistShelf { id: id.clone(), name: name.clone(), shelf }))
        .child(text("See All").font(rune::TextStyle::new(13.0, 600.0)).color(PAL.accent))
}

fn shelf_view(s: &State, id: &str, name: &str, p: &ArtistPage, shelf: Shelf, avail: f32) -> El {
    let sec = p.section(shelf);
    let n = sec.items.len().min(MAX_CARDS);
    let fits = ((avail + 12.0) / (CARD_W + 12.0)).floor() as usize;
    let extra = (sec.items.len() > fits || sec.next.is_some()).then(|| see_all(id, name, shelf));
    let more = sec.next.is_some() && sec.items.len() < MAX_CARDS;
    let key = format!("artist-{}", shelf.view());
    let id2 = id.to_string();
    carousel_with(s, &key, shelf.title(), extra, n + usize::from(more), |i| {
        if i == n {
            return more_trigger(format!("{key}-more"), &id2, More::Shelf(shelf), 60.0, CARD_W);
        }
        card(s, &sec.items[i], CARD_W)
    })
}

fn similar_card(s: &State, a: &ArtistCard) -> El {
    let (a1, a2) = (a.artist(), a.artist());
    let d = ARTIST_W - 16.0;
    let pic = match &a.artwork {
        Some(w) => art(s, Some(w), &a.name, Size::Fill, d / 2.0, 0.0).clip_rounded(d / 2.0),
        None => avatar(&a.name, d),
    };
    col().key(format!("similar-{}", a.id)).role("button").label(a.name.clone()).focusable(true).w(ARTIST_W).padding(8.0)
        .gap(8.0).items_center().radius(12.0).hover(PAL.card_hover).press(crate::theme::ov(0.04)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.navigate(Route::Artist { id: a1.id.clone(), name: a1.name.clone() }))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| ctx_menu::open_artist(s, &a2, [p.x, p.y]))
        .child(pic)
        .child(text(a.name.clone()).font(CARD_TITLE).color(PAL.text).truncate().max_w(d))
}

/// See All: one shelf of an artist page in full, newest first. Its pages load while it is open.
pub fn shelf_page(s: &State, id: &str, name: &str, shelf: Shelf, g: Grid) -> El {
    let head = col().w_full().px(PAGE_PX).pt(28.0).pb(16.0).gap(4.0)
        .child(heading(shelf.title()))
        .child(muted(name.to_string()));
    let body = match s.artist_pages.get(id) {
        Some(Load::Ready(Some(p))) => {
            let sec = p.section(shelf);
            if sec.items.is_empty() && !sec.loading {
                col().w_full().px(PAGE_PX).child(status("Nothing here.", false))
            } else {
                let mut b = col().w_full().grow(1.0).min_h(0.0)
                    .child(grid(s, format!("artist-{id}-{}-all", shelf.view()), GridSrc::ArtistShelf(id.to_string(), shelf), g, sec.loading, None, String::new()));
                if let Some(e) = &sec.error {
                    b = b.child(col().w_full().px(PAGE_PX).child(status(format!("Could not load more: {e}"), true)));
                }
                b
            }
        }
        Some(l @ (Load::Loading | Load::Failed(_))) => col().w_full().px(PAGE_PX).child(not_ready(s, l, "this artist")),
        _ => col().w_full().px(PAGE_PX).child(not_ready(s, &Load::<()>::Idle, "this artist")),
    };
    col().key(format!("artist-shelf-{id}-{}", shelf.view())).grow(1.0).min_h(0.0).w_full().child(head).child(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(name: &str) -> State {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == name).unwrap().1)(&mut s);
        s
    }

    #[test]
    fn the_top_songs_are_the_pages_song_list_and_it_has_no_filter() {
        let s = scene("artist-full");
        assert_eq!(lists::list_id(&s).as_deref(), Some("artist-top:28673423"));
        assert_eq!(lists::songs(&s).len(), 14);
        assert_eq!(lists::filter_list_id(&s), None);
        let fallback = scene("artist-library-fallback");
        assert_eq!(lists::list_id(&fallback), None);
    }

    #[test]
    fn the_page_shows_its_sections_and_hides_the_empty_one() {
        let s = scene("artist-full");
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 2600.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        for k in ["artist-hero", "artist-latest", "artist-top", "artist-full-albums", "artist-singles", "artist-compilation-albums",
            "artist-appears-on-albums", "artist-featured-playlists", "artist-similar", "artist-about", "artist-see-all-full-albums"] {
            assert!(rt.presented(k).is_some(), "{k} shown");
        }
        assert!(rt.presented("artist-live-albums").is_none(), "Live Albums is empty: hidden");
        // Compilations (2 cards) fit: no See All.
        assert!(rt.presented("artist-see-all-compilation-albums").is_none());
    }

    #[test]
    fn see_all_opens_the_shelf_grid() {
        let s = scene("artist-full");
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let r = rt.presented("artist-see-all-full-albums").expect("See All").0;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        let mut route = None;
        rt.update(|s: &mut State, _| route = Some(s.route.clone()));
        let route = route.unwrap();
        assert_eq!(route, Route::ArtistShelf { id: "28673423".into(), name: "Mara Quell".into(), shelf: Shelf::Albums });
    }
}
