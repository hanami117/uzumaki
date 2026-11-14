//! Album / playlist page: header, then a track table, as one virtual list.

use rune::{Cursor, El, GrowthSpec, ScrollPos, ScrollSpec, Transition, Tx, col, el, icon, row, text};

use super::PAGE_PX;
use super::{artist_link, filter, icons, lists, select};
use super::songs_table::{self as table, Which};
use super::tracks::same_song;
use super::widgets::{Size, art, fmt_time, fmt_total, pill, status};
use crate::models::{Artist, Collection, CollectionKind, Song};
use crate::state::{Load, State};
use crate::theme::*;

fn play_at(s: &mut State, id: &str, index: usize) {
    let c = s.collections.get(id).and_then(|l| l.ready()).cloned();
    if let Some(c) = c {
        s.play_collection(&c, index);
    }
}

/// How tight the header's button row is, from the width beside the cover. The row never wraps
/// while it fits (a wrap would make the info column taller than the cover, so the whole page
/// would jump); it gets narrower instead, and `animate_layout` springs the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Fit {
    /// Play, Shuffle, More, and the Playlist/Songs switch on the right.
    Full,
    /// Shuffle shrinks to its icon.
    Compact,
    /// Too narrow even for that: the row may wrap.
    Wrap,
}

/// From the width of the info column beside the 220 pt cover (28 pt gap) for page content
/// `avail` wide. `switch`: the row also holds the Playlist/Songs switch (playlists, ~170 pt).
/// `star`: it also holds the 44 pt favourite button and its gap.
fn fit(avail: f32, switch: bool, star: bool) -> Fit {
    let info = avail - 220.0 - 28.0 + if switch { 0.0 } else { 170.0 } - if star { 54.0 } else { 0.0 };
    if info >= 490.0 {
        Fit::Full
    } else if info >= 410.0 {
        Fit::Compact
    } else {
        Fit::Wrap
    }
}

/// Whether the page offers the favourite star: signed in, and not the Favourite Songs playlist
/// itself.
fn has_star(s: &State, c: &Collection) -> bool {
    s.session.authorized && !c.favorites && !crate::local::view::is_album_id(&c.id)
}

/// The round favourite button, as big as More: coral and filled when favourite.
fn star_button(on: bool) -> El {
    el().key("coll-star").role("button").label(if on { "Undo Favourite" } else { "Favourite" }).focusable(true)
        .size(44.0, 44.0).radius(22.0).center().fill(PAL.nav_active).hover(crate::theme::ov(0.1))
        .press(rune::Color::BLACK.alpha(0.12)).cursor(Cursor::Pointer)
        .child(icon(if on { &icons::STAR_FILLED } else { &icons::STAR }, 18.0).color(if on { PAL.accent } else { PAL.text2 }))
}

/// The Shuffle button: a pill whose label folds away (width and fade) when `compact`, leaving a
/// round icon button the size of More.
fn shuffle_button(compact: bool) -> El {
    let label = el().key("shuffle-label").max_w(if compact { 0.0 } else { 120.0 }).clip_rounded(0.0)
        .opacity(if compact { 0.0 } else { 1.0 })
        .child(text("Shuffle").font(BUTTON).color(PAL.text).pl(8.0));
    row().key("shuffle").role("button").label("Shuffle").focusable(true).h(44.0).px(if compact { 14.0 } else { 22.0 })
        .center().radius(22.0).fill(PAL.nav_active).hover(crate::theme::ov(0.08)).press(rune::Color::BLACK.alpha(0.12))
        .cursor(Cursor::Pointer)
        .child(icon(&icons::SHUFFLE, 16.0).color(PAL.text))
        .child(label)
}

/// The album artist's coral, lightened while the pointer is on the link.
fn accent_hover() -> rune::Color {
    PAL.accent_soft
}

/// The artists a row's name links to: the song's own when Apple sent them, else the album's
/// when the song's artist is the album artist (the common case, with no extra request).
fn row_artists(s: &State, c: &Collection, t: &Song) -> Vec<Artist> {
    if t.is_local() {
        // Local only mode: the music folders' artist page. Apple mode: plain text, as before.
        return if s.local_only() { vec![crate::local::view::artist_of(t)] } else { Vec::new() };
    }
    if !t.artists.is_empty() {
        t.artists.clone()
    } else if c.kind == CollectionKind::Album && !t.is_local() && t.artist == c.subtitle {
        c.artists.clone()
    } else {
        Vec::new()
    }
}

/// The header's cover size.
const COVER: f32 = 220.0;

/// Scrolled further than this (points), the header's cover is off the page: its clip holds.
const COVER_BOTTOM: f32 = 28.0 + COVER;

thread_local! {
    /// The page whose scroll offset was last reported, and whether it was past the cover.
    static AWAY: std::cell::RefCell<(String, bool)> = const { std::cell::RefCell::new((String::new(), false)) };
}

/// The page scroller's offset: commits to the state only when it crosses the cover's bottom,
/// so scrolling never runs the view, and pausing happens even if no other frame follows.
fn track_scroll(handle: Option<crate::state::Handle>, id: String) -> impl Fn(ScrollPos) + 'static {
    move |p: ScrollPos| {
        let away = p.offset[1] > COVER_BOTTOM;
        let changed = AWAY.with_borrow_mut(|(page, was)| {
            let changed = *page != id || *was != away;
            (*page, *was) = (id.clone(), away);
            changed
        });
        if let (true, Some(h)) = (changed, &handle) {
            let id = id.clone();
            h.update_if(move |s: &mut State, _| {
                let page = away.then_some(id);
                if s.motion.scrolled_away == page {
                    return false;
                }
                s.motion.scrolled_away = page;
                s.poll_page_motion()
            });
        }
    }
}

/// Before each frame (`ui::poll`): the album page's animated cover. While it holds because the
/// window can't draw, a timer frame each second finds out when it can again.
pub fn poll(s: &mut State, tx: &mut Tx) -> bool {
    let changed = s.poll_page_motion();
    if s.motion.covered && s.motion.page.is_some() {
        tx.wake_after(std::time::Duration::from_secs(1));
    }
    if s.motion.page.as_ref().is_some_and(|p| p.warming()) {
        tx.wake_after(std::time::Duration::from_millis(250));
    }
    // A lookup that failed is asked again later: a frame then, even if nothing else draws.
    if let (true, Some(d)) = (s.page_facts_album_page(), s.motion.next_retry()) {
        tx.wake_after(d);
    }
    changed
}

fn header(s: &State, c: &Collection, fit: Fit) -> El {
    let kind = match c.kind {
        CollectionKind::Album => "ALBUM",
        CollectionKind::Playlist => "PLAYLIST",
    };
    let count = if c.tracks.is_empty() { c.track_count.unwrap_or(0) as usize } else { c.tracks.len() };
    let total: f64 = c.tracks.iter().map(|t| t.duration).sum();
    let mut meta: Vec<String> = Vec::new();
    meta.extend(c.year.clone());
    meta.extend(c.genre.clone());
    if count > 0 {
        meta.push(format!("{count} song{}", if count == 1 { "" } else { "s" }));
    }
    if total > 0.0 {
        meta.push(fmt_total(total));
    }
    let sub_color = if c.kind == CollectionKind::Album { PAL.accent } else { PAL.muted };

    let id_play = c.id.clone();
    let id_shuffle = c.id.clone();
    let mut buttons = row().key("coll-buttons").w_full().gap(10.0).mt(18.0).items_center().animate_layout();
    if fit == Fit::Wrap {
        buttons = buttons.flex_wrap();
    }
    buttons = buttons
        .child(pill("play", "Play", &icons::PLAY, PAL.accent, PAL.ink).on_press(move |s: &mut State| play_at(s, &id_play, 0)))
        .child(shuffle_button(fit != Fit::Full).on_press(move |s: &mut State| {
            if let Some(c) = s.collections.get(&id_shuffle).and_then(|l| l.ready()).cloned() {
                s.shuffle_collection(&c);
            }
        }));
    if has_star(s, c) {
        let id_star = c.id.clone();
        buttons = buttons.child(star_button(s.is_favourite_collection(c)).on_press(move |s: &mut State| {
            if let Some(c) = s.collections.get(&id_star).and_then(|l| l.ready()).cloned() {
                s.toggle_favourite_collection(&c);
            }
        }));
    }
    let id_more = c.id.clone();
    buttons = buttons.child(super::ctx_menu::more_button("coll-more", 44.0, move |s: &State| {
        s.collections.get(&id_more).and_then(|l| l.ready()).map(|c| super::ctx_menu::collection_items(s, c)).unwrap_or_default()
    }));
    if c.kind == CollectionKind::Playlist {
        // Pushed to the right edge, so it never sits on a line of its own.
        buttons = buttons.child(el().grow(1.0).min_w(8.0)).child(table::segmented(s.ui.table.songs_view));
    }

    let mut info = col().grow(1.0).min_w(0.0)
        .child(text(kind).font(LABEL12).color(PAL.muted))
        .child(text(c.title.clone()).font(HERO).color(PAL.text).wrap().max_lines(2).my(6.0))
        .child(artist_link::link(
            "coll-artist".into(),
            text(c.subtitle.clone()).font(rune::TextStyle::new(16.0, 500.0)).color(sub_color).truncate().min_w(0.0),
            &c.artists,
            accent_hover(),
        ))
        .child(text(meta.join(" \u{b7} ")).font(SMALL13).color(PAL.muted).mt(4.0));
    if let Some(d) = c.description.as_ref().filter(|d| !d.trim().is_empty()) {
        info = info.child(text(d.clone()).font(SMALL13).color(PAL.dim).wrap().max_lines(2).max_w(680.0).mt(10.0));
    }
    info = info.child(buttons);

    let mut cover = art(s, s.cover_of(c), &c.title, Size::Fixed(COVER), 12.0, 40.0);
    if State::can_set_picture(c) {
        // Clicking the cover edits the playlist; "Edit" and a dim show on hover.
        let c2 = c.clone();
        cover = cover.key("coll-cover").role("button").label("Edit Playlist").focusable(true).hover_group()
            .cursor(Cursor::Pointer).clip_rounded(12.0)
            .on_press(move |s: &mut State| super::new_playlist::open_edit(s, &c2))
            .child(
                el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().fill(rune::Color::BLACK.alpha(0.45))
                    .group_opacity(0.0, 1.0)
                    .child(text("Edit").font(rune::TextStyle::new(15.0, 600.0)).color(rune::Color::WHITE)),
            );
    } else {
        cover = cover.key("coll-art");
        // Animated artwork (`state::motion`): Apple's square loop over the still, in the same box,
        // fading in once its first frame is drawn (as the Mac app's album page does); until then
        // the Mac's clip draws nearly transparent so that frame gets drawn at all.
        let album = s.page_album_known(c);
        if let Some(layer) = s.motion.page_layer_for(album.as_deref()) {
            cover = cover.clip_rounded(12.0).child(layer.key("coll-motion").transition(super::now_playing::motion_transition()));
        } else if let Some(warm) = s.motion.page_warm_layer_for(album.as_deref()) {
            cover = cover.clip_rounded(12.0).child(warm.key("coll-motion-warm").transition(Transition::none()));
        }
    }
    // The cover keeps its place at the top on every page. The info column is at least as tall
    // and sits at its bottom, so a short one lines up with the cover's bottom (the wide look)
    // and a taller one (a narrow window: wrapped title, description, wrapped buttons) starts
    // level with it and grows down, instead of pushing the cover down.
    row().w_full().px(PAGE_PX).pt(28.0).pb(28.0).gap(28.0).items_start()
        .child(cover)
        .child(info.key("coll-info").min_h(COVER).justify_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_row_folds_before_it_wraps() {
        // Content width = info column + 248 (cover and gap).
        assert_eq!(fit(248.0 + 600.0, true, false), Fit::Full);
        assert_eq!(fit(248.0 + 490.0, true, false), Fit::Full);
        assert_eq!(fit(248.0 + 450.0, true, false), Fit::Compact);
        assert_eq!(fit(248.0 + 300.0, true, false), Fit::Wrap);
        // Albums have no Playlist/Songs switch: they fold later.
        assert_eq!(fit(248.0 + 450.0, false, false), Fit::Full);
        assert_eq!(fit(248.0 + 300.0, false, false), Fit::Compact);
        // The star button takes 54 pt of the row, so everything folds 54 pt sooner.
        assert_eq!(fit(248.0 + 544.0, true, true), Fit::Full);
        assert_eq!(fit(248.0 + 543.0, true, true), Fit::Compact);
        assert_eq!(fit(248.0 + 464.0, true, true), Fit::Compact);
        assert_eq!(fit(248.0 + 463.0, true, true), Fit::Wrap);
        assert_eq!(fit(248.0 + 300.0, false, true), Fit::Compact);
        assert_eq!(fit(248.0 + 290.0, false, true), Fit::Wrap);
    }

    /// Animated artwork on an album page: the clip fades in over the still in the cover's box
    /// (nothing moves, the info column stays put), and leaving the page frees the player. The
    /// Mac's player shows its poster at once; the ffmpeg path needs a handle for its report.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_album_cover_crossfades_to_its_clip_and_stops_off_the_page() {
        if std::env::var("UZUMAKI_MOTION").as_deref() == Ok("ffmpeg") {
            return;
        }
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "album-motion").unwrap().1)(&mut s);
        s.settings.animated_artwork = crate::settings::AnimatedArtwork::NowPlaying;
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        let step = |rt: &mut rune::Runtime<State>| {
            rt.update_if(|s, tx| crate::ui::poll(s, 0.0, tx));
            rt.frame(1.0 / 60.0);
        };
        for _ in 0..30 {
            step(&mut rt);
        }
        assert!(rt.presented("coll-motion").is_none(), "Now Playing only: the still on album pages");
        let cover = rt.presented("coll-art").unwrap().0;
        let info = rt.presented("coll-info").unwrap().0;

        rt.update(|s, _| s.set_animated_artwork(crate::settings::AnimatedArtwork::Always));
        let mut seen = Vec::new();
        for _ in 0..60 {
            step(&mut rt);
            if let Some((rect, opacity)) = rt.presented("coll-motion") {
                assert_eq!(rect, cover, "the clip fills the cover's box exactly");
                seen.push(opacity);
            }
            assert_eq!(rt.presented("coll-art").unwrap().0, cover, "the cover never moves");
            assert_eq!(rt.presented("coll-info").unwrap().0, info, "nor the title");
        }
        assert!(seen.iter().any(|o| *o > 0.05 && *o < 0.95), "a crossfade, not a cut: {seen:?}");
        assert!((seen.last().unwrap() - 1.0).abs() < 1e-3, "settles fully shown: {seen:?}");

        rt.update(|s, _| s.navigate(crate::state::Route::Home));
        for _ in 0..30 {
            step(&mut rt);
        }
        let mut gone = false;
        rt.update(|s, _| gone = s.motion.page.is_none());
        assert!(gone, "no player left once the page is gone");
    }

    /// The album page's Mac clip waits for its first drawn frame too (the 2026-11-14 flash of an
    /// empty box before the clip): nearly transparent until then, the still showing, then a fade.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_album_clip_fades_in_only_after_its_first_frame_is_drawn() {
        if std::env::var("UZUMAKI_MOTION").as_deref() == Ok("ffmpeg") {
            return;
        }
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "album-motion").unwrap().1)(&mut s);
        let crate::state::Route::Collection { id, .. } = s.route.clone() else { panic!() };
        let clip = s.motion.clips.values().find_map(crate::state::Load::ready).unwrap().clone();
        s.motion.page = Some(crate::motion::Playback::start(id, 9, &clip, Box::new(|_, _| {}), true));
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        let step = |rt: &mut rune::Runtime<State>| {
            rt.update_if(|s, tx| crate::ui::poll(s, 0.0, tx));
            rt.frame(1.0 / 60.0);
        };
        for _ in 0..20 {
            step(&mut rt);
            assert!(rt.presented("coll-motion").is_none(), "the still until a frame is drawn");
            let (_, o) = rt.presented("coll-motion-warm").expect("drawn, to pull its first frame");
            assert!(o < 0.01, "invisible: {o}");
        }
        rt.update_if(|s, _| s.motion_event(9, crate::motion::Event::FirstFrame));
        let mut seen = Vec::new();
        for _ in 0..40 {
            step(&mut rt);
            if let Some((_, o)) = rt.presented("coll-motion") {
                seen.push(o);
            }
        }
        assert!(seen.iter().any(|o| *o > 0.05 && *o < 0.95), "a crossfade: {seen:?}");
        assert!((seen.last().unwrap() - 1.0).abs() < 1e-3);
    }

    /// The header's cover and info column, settled, in the scene at `size`.
    fn header_rects(scene: &str, size: (f32, f32)) -> ([f32; 4], [f32; 4]) {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
        let mut rt = rune::Runtime::new(s, crate::ui::view, size, rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
        }
        let cover = rt.presented("coll-art").or_else(|| rt.presented("coll-cover")).expect("cover").0;
        (cover, rt.presented("coll-info").expect("info").0)
    }

    #[test]
    fn the_cover_sits_at_the_top_of_the_header_on_every_page() {
        for size in [(640.0, 1280.0), (1360.0, 860.0)] {
            let (album, album_info) = header_rects("album", size);
            let (playlist, playlist_info) = header_rects("playlist", size);
            assert_eq!(album[1], playlist[1], "the cover moved between album and playlist at {size:?}");
            for (cover, info) in [(album, album_info), (playlist, playlist_info)] {
                // Level with the top of the info column (the kicker), or above it when the
                // info is shorter and lines up with the cover's bottom instead.
                assert!(cover[1] <= info[1] + 0.5, "cover {cover:?} below info {info:?} at {size:?}");
                assert!(info[1] + info[3] >= cover[1] + cover[3] - 0.5, "info ends above the cover at {size:?}");
            }
        }
        // Narrow: the playlist's info is taller than the cover and starts level with it.
        let (cover, info) = header_rects("playlist", (640.0, 1280.0));
        assert!(info[3] > cover[3]);
        assert!((cover[1] - info[1]).abs() < 0.5);
    }
}

fn head_cell(label: &'static str) -> El {
    text(label.to_uppercase()).font(TABLE_HEAD).color(PAL.dim)
}

fn table_head(first: &'static str) -> El {
    row().w_full().px(PAGE_PX - 12.0).child(
        row().w_full().min_h(32.0).px(12.0).gap(12.0).center_y().mb(6.0)
            .child(el().w(44.0).child(head_cell(first)))
            .child(el().grow(1.0).min_w(0.0).child(head_cell("Title")))
            .child(el().grow(1.0).min_w(0.0).child(head_cell("Artist")))
            .child(el().w(super::tracks::STAR_W))
            .child(row().w(56.0).justify_end().child(head_cell("Time"))),
    )
}

/// Row of the artwork list view. `i` is the track's position in the collection.
fn track_row(s: &State, c: &Collection, i: usize, t: &Song, list: &str) -> El {
    let cur = s.playback.song.as_ref().is_some_and(|p| same_song(p, t));
    let play_icon = |size: f32| icon(&icons::PLAY, size).color(PAL.text);
    let key = format!("t{i}-{}", t.id);
    let lead = match c.kind {
        CollectionKind::Album => {
            let n = t.track_number.map_or(i + 1, |n| n as usize);
            el().w(44.0).h(36.0).items_center()
                .child(text(n.to_string()).font(TIME14).color(PAL.dim).group_opacity(1.0, 0.0))
                .child(el().absolute(0.0, 10.0).group_opacity(0.0, 1.0).child(play_icon(16.0)))
        }
        CollectionKind::Playlist => el().w(44.0).child(
            art(s, t.artwork.as_ref(), &t.album, Size::Fixed(36.0), 6.0, 0.0).child(
                el().size(36.0, 36.0).radius(6.0).center().fill(rune::Color::BLACK.alpha(0.5)).group_opacity(0.0, 1.0)
                    .child(play_icon(14.0)),
            ),
        ),
    };
    let rk = lists::row_key(s, i);
    let lead = select::play_button(lead.key(format!("{key}-play")), list, &rk);
    let star = super::tracks::star_slot(s, t, &key);
    let r_key = key.clone();
    let r = row().key(key).role("button").label(t.title.clone()).focusable(true).w_full().min_h(48.0)
        .px(12.0).gap(12.0).center_y().radius(8.0).hover(PAL.row_hover).hover_group().cursor(Cursor::Pointer)
        .child(lead)
        .child(text(t.title.clone()).font(TRACK_TITLE).color(if cur { PAL.accent } else { PAL.text2 }).truncate().grow(1.0).min_w(0.0))
        .child(
            row().grow(1.0).min_w(0.0).child(artist_link::link(
                format!("{}-artist", r_key),
                text(t.artist.clone()).font(BODY14).color(PAL.muted).truncate().min_w(0.0),
                &row_artists(s, c, t),
                PAL.text,
            )),
        )
        .child(star)
        .child(row().w(56.0).justify_end().child(text(fmt_time(t.duration)).font(TIME14).color(PAL.muted)));
    select::wire(s, r, &rk, list)
}

pub fn page(s: &State, id: &str, avail: f32) -> El {
    let key = format!("coll-{id}");
    let scroller = |inner: El| col().key(key.clone()).grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(track_scroll(s.app_handle().cloned(), id.to_string())).child(inner);
    let c = match s.collections.get(id) {
        Some(Load::Ready(c)) => c,
        Some(Load::Failed(e)) => {
            return scroller(col().w_full().px(PAGE_PX).pt(28.0).child(status(format!("Could not load this page: {e}"), true)));
        }
        _ => return scroller(col().w_full().px(PAGE_PX).pt(28.0).child(status("Loading\u{2026}", false))),
    };

    let order = lists::order(s);
    let m = order.len();
    let first = if c.kind == CollectionKind::Album { "#" } else { "" };
    let songs_view = c.kind == CollectionKind::Playlist && s.ui.table.songs_view;
    let sort = s.ui.table.sort(Which::Playlist);
    let cols = s.ui.table.cols(avail);
    let query = lists::query(s);
    let list = lists::list_id(s).unwrap_or_default();
    let cur_id = s.playback.song.as_ref().map(|x| x.id.clone());
    let cid = c.id.clone();
    let fit = fit(avail, c.kind == CollectionKind::Playlist, has_star(s, c));
    let body = scroller(
        col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(
                m + 3,
                50.0,
                move |_: &State, i| format!("{cid}/{i}").into(),
                {
                    let (cid, list) = (c.id.clone(), list.clone());
                    move |s: &State, i: usize| {
                        let Some(c) = s.collections.get(&cid).and_then(|l| l.ready()) else { return col() };
                        if i == 0 {
                            return header(s, c, fit);
                        }
                        if i == 1 {
                            return if songs_view {
                                table::head(Which::Playlist, s.ui.table.sort(Which::Playlist), cols)
                            } else {
                                table_head(first)
                            };
                        }
                        let order = lists::order(s);
                        match order.get(i - 2).and_then(|&k| c.tracks.get(k).map(|t| (k, t))) {
                            Some((k, t)) => {
                                let r = if songs_view {
                                    table::song_row_with(s, t, i - 1, cols, format!("t{k}-{}", t.id), &list, &lists::row_key(s, k), &row_artists(s, c, t))
                                } else {
                                    track_row(s, c, k, t, &list)
                                };
                                row().w_full().px(PAGE_PX - 12.0).py(1.0).child(r)
                            }
                            None if i == 2 && !c.tracks.is_empty() => {
                                col().w_full().px(PAGE_PX).child(status("No songs match your filter.", false))
                            }
                            None => col().h(40.0),
                        }
                    }
                },
            )
            .virtual_rows_version((c.id.clone(), m, songs_view, sort, cols, query, s.ui.sel.rev, cur_id, fit, s.favourites_rev(), s.motion.page_rev())),
    );
    let mut page = col().grow(1.0).min_h(0.0).w_full();
    if let Some(bar) = filter::bar(s, m, c.tracks.len()) {
        page = page.child(el().h(12.0)).child(bar);
    }
    page.child(body)
}
