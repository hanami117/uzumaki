//! The sidebar: brand, Search / Home / New / Radio, then a scrolling Library section (Pins,
//! Recently Added, Artists, ...) and Playlists, or the icon rail (`RAIL_W`) when collapsed.
//! The scrolling part is one virtual list, so hundreds of playlists cost nothing.

use rune::{Cursor, El, GrowthSpec, Icon, ScrollSpec, Tx, col, el, icon, memo, row, text};

use super::widgets::{avatar, held_art, logo_tile};
use super::{ctx_menu, dnd, icons};
use crate::models::{Artwork, Collection};
use crate::settings::{Pin, PinKind};
use crate::state::{LibraryTab, Load, Route, State};
use crate::theme::*;

/// Icon sizes: a full sidebar row's, the rail's (larger, filling the rail under the macOS
/// traffic lights), and the settings and toggle buttons'. Shared by every platform.
const ICON: f32 = 19.0;
const RAIL_ICON: f32 = 22.0;
const BUTTON_ICON: f32 = 19.0;
/// A full sidebar row's height.
pub(super) const ROW_H: f32 = 36.0;

/// The sidebar shows as a rail when collapsed by the user, or when the window is narrower.
pub fn is_rail(s: &State, win_w: f32) -> bool {
    s.settings.sidebar_collapsed || is_narrow(win_w)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    Search,
    Home,
    New,
    Radio,
    RecentlyAdded,
    Artists,
    Albums,
    Songs,
    MadeForYou,
    LocalFiles,
    AllPlaylists,
    // Local only mode's library: Local Files becomes Songs, beside Artists, Albums and Folders.
    LocalArtists,
    LocalAlbums,
    LocalSongs,
    Folders,
}

const TOP: [Nav; 4] = [Nav::Search, Nav::Home, Nav::New, Nav::Radio];

/// Every sidebar entry the user can hide (Settings → Appearance), as (id, label), in sidebar
/// order. Ids are stored in `Settings::hidden_nav`.
pub const NAV_ITEMS: [(&str, &str); 17] = [
    ("nav-search", "Search"),
    ("nav-home", "Home"),
    ("nav-new", "New"),
    ("nav-radio", "Radio"),
    ("pins", "Pins"),
    ("nav-recent", "Recently Added"),
    ("nav-artists", "Artists"),
    ("nav-albums", "Albums"),
    ("nav-songs", "Songs"),
    ("nav-mfy", "Made for You"),
    ("nav-local", "Local Files"),
    ("nav-local-artists", "Artists"),
    ("nav-local-albums", "Albums"),
    ("nav-folders", "Folders"),
    ("nav-playlists", "All Playlists"),
    ("favourites", "Favourite Songs"),
    ("playlists", "Your playlists"),
];

/// The entries local only mode keeps; every other one is Apple Music's.
pub const LOCAL_NAV: [&str; 5] = ["nav-search", "nav-local-artists", "nav-local-albums", "nav-local", "nav-folders"];
/// The entries only local only mode has.
const LOCAL_ONLY_NAV: [&str; 3] = ["nav-local-artists", "nav-local-albums", "nav-folders"];

fn shown(s: &State, id: &str) -> bool {
    !s.nav_hidden(id) && if s.local_only() { LOCAL_NAV.contains(&id) } else { !LOCAL_ONLY_NAV.contains(&id) }
}

/// The entries Settings → Appearance offers in the current mode, as (id, label).
pub fn nav_choices(local: bool) -> Vec<(&'static str, &'static str)> {
    if local {
        // In the sidebar's order, Local Files as the Songs tab it is there.
        LOCAL_NAV.iter().filter_map(|id| NAV_ITEMS.iter().find(|(i, _)| i == id).map(|&(i, l)| (i, if i == "nav-local" { "Songs" } else { l }))).collect()
    } else {
        NAV_ITEMS.into_iter().filter(|(id, _)| !LOCAL_ONLY_NAV.contains(id)).collect()
    }
}
const LIBRARY: [Nav; 6] = [Nav::RecentlyAdded, Nav::Artists, Nav::Albums, Nav::Songs, Nav::MadeForYou, Nav::LocalFiles];
const LOCAL_LIBRARY: [Nav; 4] = [Nav::LocalArtists, Nav::LocalAlbums, Nav::LocalSongs, Nav::Folders];

/// The Library entries of the current mode.
fn library(s: &State) -> &'static [Nav] {
    if s.local_only() { &LOCAL_LIBRARY } else { &LIBRARY }
}

impl Nav {
    fn info(self) -> (&'static str, &'static Icon, &'static str, Route) {
        match self {
            Nav::Search => ("nav-search", &icons::SEARCH, "Search", Route::Search),
            Nav::Home => ("nav-home", &icons::HOME, "Home", Route::Home),
            Nav::New => ("nav-new", &icons::NEW, "New", Route::New),
            Nav::Radio => ("nav-radio", &icons::RADIO, "Radio", Route::Radio),
            Nav::RecentlyAdded => ("nav-recent", &icons::CLOCK, "Recently Added", Route::RecentlyAdded),
            Nav::Artists => ("nav-artists", &icons::ARTISTS, "Artists", Route::Artists),
            Nav::Albums => ("nav-albums", &icons::ALBUMS, "Albums", Route::Library(LibraryTab::Albums)),
            Nav::Songs => ("nav-songs", &icons::SONGS, "Songs", Route::Library(LibraryTab::Songs)),
            Nav::MadeForYou => ("nav-mfy", &icons::MADE_FOR_YOU, "Made for You", Route::MadeForYou),
            Nav::LocalFiles => ("nav-local", &icons::LOCAL_FILES, "Local Files", Route::LocalFiles),
            Nav::AllPlaylists => ("nav-playlists", &icons::ALL_PLAYLISTS, "All Playlists", Route::Library(LibraryTab::Playlists)),
            Nav::LocalArtists => ("nav-local-artists", &icons::ARTISTS, "Artists", Route::LocalArtists),
            Nav::LocalAlbums => ("nav-local-albums", &icons::ALBUMS, "Albums", Route::LocalAlbums),
            Nav::LocalSongs => ("nav-local", &icons::SONGS, "Songs", Route::LocalFiles),
            Nav::Folders => ("nav-folders", &icons::LOCAL_FILES, "Folders", Route::Folders { path: None }),
        }
    }

    fn active(self, route: &Route) -> bool {
        let target = self.info().3;
        match (self, route) {
            (Nav::Artists, Route::Artist { id, .. }) => !crate::local::view::is_artist_id(id),
            (Nav::Artists, Route::ArtistShelf { .. }) => true,
            (Nav::LocalArtists, Route::Artist { id, .. }) => crate::local::view::is_artist_id(id),
            (Nav::Folders, Route::Folders { .. }) => true,
            _ => *route == target,
        }
    }
}

fn nav_item(n: Nav, route: &Route) -> El {
    let (key, ic, label, target) = n.info();
    hideable(item(key.to_string(), ic, label.to_string(), n.active(route)).on_press(move |s: &mut State| go(s, &target)), key, label)
}

/// Right-click on a sidebar entry: hide it, or open the full list in Settings.
fn hideable(e: El, id: &'static str, label: &'static str) -> El {
    e.on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| ctx_menu::open_nav(s, id, label, [p.x, p.y]))
}

/// Opens a top-level page; the Search page also gets the cursor.
fn go(s: &mut State, target: &Route) {
    s.navigate(target.clone());
    if *target == Route::Search {
        super::search::focus(s);
    }
}

fn pill(active: bool) -> (rune::Color, rune::Color) {
    if active { (PAL.nav_active, PAL.text) } else { (rune::Color::TRANSPARENT, PAL.nav_text) }
}

/// A sidebar row: line icon in the accent colour, label, rounded pill when active.
fn item(key: String, ic: &'static Icon, label: String, active: bool) -> El {
    item_with(key, icon(ic, ICON).color(PAL.accent), label, active)
}

/// A sidebar row with `lead` (an icon, or a cover thumbnail in an `ICON`-wide slot) before the label.
fn item_with(key: String, lead: El, label: String, active: bool) -> El {
    let (bg, fg) = pill(active);
    row().key(key).role("button").label(label.clone()).focusable(true).w_full().h(ROW_H).px(10.0).gap(12.0).center_y()
        .radius(8.0).fill(bg).hover(crate::theme::ov(0.05)).press(crate::theme::ov(0.08))
        .cursor(Cursor::Pointer)
        .child(lead)
        .child(text(label).font(NAV).color(fg).truncate().min_w(0.0).grow(1.0))
}

/// A pin's thumbnail: its size and corner radius are the playlists' too.
const THUMB: f32 = 24.0;

fn thumb(s: &State, pin: &Pin, size: f32) -> El {
    match pin.kind {
        PinKind::Artist => avatar(&pin.title, size),
        PinKind::Station if pin.artwork.is_none() => {
            el().size(size, size).radius(5.0).center().fill(PAL.chip_hover).child(icon(&icons::RADIO, size * 0.7).color(PAL.accent))
        }
        _ => cover(s.art.held(&pin.id, s.pin_cover(pin), crate::art::SMALL_PX).as_ref(), &pin.title, size),
    }
}

/// A small cover: the shared 120 px picture in the art cache (`Art::held`: once shown it stays,
/// whatever its link does), a tinted tile with initials until the first one lands.
fn cover(held: Option<&(rune::Image, bool)>, title: &str, size: f32) -> El {
    held_art(held, title, size, 5.0, size * 0.4)
}

fn pin_active(pin: &Pin, route: &Route) -> bool {
    matches!(route, Route::Collection { id, .. } | Route::Artist { id, .. } if *id == pin.id)
}

fn pin_handlers(e: El, pin: &Pin) -> El {
    let (p1, id) = (pin.clone(), pin.id.clone());
    e.on_press(move |s: &mut State| {
        // The tail of a drag is not a click.
        if !s.ui.dnd.swallow_click() {
            s.open_pin(&p1);
        }
    })
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| ctx_menu::open_pin(s, &id, [p.x, p.y]))
}

fn pin_item(s: &State, pin: &Pin, k: usize) -> El {
    let (bg, fg) = pill(pin_active(pin, &s.route));
    let e = row().key(format!("pin-{}", pin.id)).role("button").label(pin.title.clone()).focusable(true).w_full().h(ROW_H)
        .pl(10.0).pr(10.0).gap(12.0).center_y().radius(8.0).fill(bg).hover(crate::theme::ov(0.05))
        .press(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
        .child(el().w(ICON).center().child(thumb(s, pin, THUMB)))
        .child(text(pin.title.clone()).font(NAV).color(fg).truncate().min_w(0.0).grow(1.0));
    let e = if dnd::is_pin_playlist(pin) { dnd::playlist_target(s, e, &pin.id) } else { e };
    // The rows are `ROW_H` apart (virtual rows take no gap).
    dnd::pin_row(s, row().w_full().px(10.0), pin_handlers(e, pin), k, pin, ROW_H)
}

fn pins_head(s: &State) -> El {
    let open = s.settings.pins_open;
    hideable(item("pins-head".into(), &icons::PIN, "Pins".into(), false), "pins", "Pins")
        .on_press_tx(|s: &mut State, tx: &mut Tx| {
            s.toggle_pins_open();
            tx.instant();
        })
        .child(
            el().pin(Some(2.0), Some(10.0), None, None)
                .child(icon(if open { &icons::DISCLOSE_OPEN } else { &icons::DISCLOSE_CLOSED }, 12.0).color(PAL.dim)),
        )
}

fn header(label: &'static str) -> El {
    let t = text(label).font(rune::TextStyle::new(12.0, 500.0)).color(PAL.dim);
    if label != "Playlists" {
        return t.px(20.0).pt(16.0).pb(5.0);
    }
    // A + beside the heading opens the New Playlist dialog.
    row().w_full().pl(20.0).pr(14.0).pt(10.0).pb(2.0).center_y()
        .child(t.grow(1.0))
        .child(
            el().key("new-playlist-btn").role("button").label("New Playlist").focusable(true).size(24.0, 24.0).radius(12.0).center()
                .hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
                .on_press(|s: &mut State| super::new_playlist::open(s, Vec::new()))
                .child(icon(&icons::PLUS, 14.0).color(PAL.muted)),
        )
}

/// The picture a sidebar playlist shows: the user's own, else the list's current one (the same
/// choice as `State::pin_cover`: an uploaded picture's signed link only lives a day, so the list's
/// fresh one is what loads). None keeps the icon.
fn playlist_cover<'a>(s: &'a State, c: &'a Collection) -> Option<&'a Artwork> {
    s.cover_of(c)
}

/// `held`: the row's picture from `Art::held` (looked up by `build_row`, which keys the memo on it).
fn playlist_item(s: &State, c: &Collection, active: bool, held: Option<&(rune::Image, bool)>) -> El {
    let (c1, c2) = (c.clone(), c.clone());
    let key = format!("pl-{}", c.id);
    let row = match playlist_cover(s, c) {
        // Same slot, size and radius as a pin's thumbnail.
        Some(_) => item_with(key, el().w(ICON).center().child(cover(held, &c.title, THUMB)), c.title.clone(), active),
        None => item(key, if c.favorites { &icons::FAVORITES } else { &icons::PLAYLIST }, c.title.clone(), active),
    };
    dnd::playlist_target(s, row, &c.id)
        .on_press(move |s: &mut State| s.open_collection(&c1))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| ctx_menu::open_collection(s, &c2, [p.x, p.y]))
}

fn playlist_active(c: &Collection, route: &Route) -> bool {
    matches!(route, Route::Collection { id, .. } if *id == c.id)
}

/// The scrolling section's rows, computed from counts alone.
#[derive(Clone, Copy)]
struct Rows {
    pins: usize,
    pins_open: bool,
    /// The Library entries not hidden, in order (`lib[..lib_n]`).
    lib: [Nav; 6],
    lib_n: usize,
    all_playlists: bool,
    /// Index of Favourite Songs in the library playlists.
    fav: Option<usize>,
    /// Playlists other than Favourite Songs.
    others: usize,
    note: bool,
}

enum Row {
    Header(&'static str),
    PinsHead,
    Pin(usize),
    Nav(Nav),
    /// Favourite Songs (always first under the heading).
    Favourites,
    /// The `k`-th playlist other than Favourite Songs.
    Playlist(usize),
    Note,
    Pad,
}

impl Rows {
    fn of(s: &State) -> Self {
        let items = &s.library_playlists.items;
        let fav = items.iter().position(|c| c.favorites).filter(|_| shown(s, "favourites"));
        let mut lib = LIBRARY;
        let mut lib_n = 0;
        for &n in library(s) {
            if shown(s, n.info().0) {
                lib[lib_n] = n;
                lib_n += 1;
            }
        }
        let playlists = shown(s, "playlists");
        let others = if playlists { items.iter().filter(|c| !c.favorites).count() } else { 0 };
        Rows {
            pins: if shown(s, "pins") { s.settings.pins.len() } else { 0 },
            pins_open: s.settings.pins_open,
            lib,
            lib_n,
            all_playlists: shown(s, "nav-playlists"),
            fav,
            others,
            note: playlists && items.is_empty() && matches!(s.library_playlists.load, Load::Loading),
        }
    }

    fn pin_rows(&self) -> usize {
        if self.pins > 0 { 1 + if self.pins_open { self.pins } else { 0 } } else { 0 }
    }

    /// The "Library" heading shows while anything under it does.
    fn lib_header(&self) -> bool {
        self.pin_rows() + self.lib_n > 0
    }

    /// The "Playlists" heading (with its +) shows while anything under it does.
    fn pl_header(&self) -> bool {
        self.all_playlists || self.fav.is_some() || self.others > 0 || self.note
    }

    fn count(&self) -> usize {
        usize::from(self.lib_header()) + self.pin_rows() + self.lib_n + usize::from(self.pl_header())
            + usize::from(self.all_playlists) + usize::from(self.fav.is_some()) + self.others + usize::from(self.note) + 1
    }

    fn at(&self, i: usize) -> Row {
        let mut k = i;
        if self.lib_header() {
            if k == 0 {
                return Row::Header("Library");
            }
            k -= 1;
        }
        if self.pins > 0 {
            if k == 0 {
                return Row::PinsHead;
            }
            k -= 1;
            if self.pins_open {
                if k < self.pins {
                    return Row::Pin(k);
                }
                k -= self.pins;
            }
        }
        if k < self.lib_n {
            return Row::Nav(self.lib[k]);
        }
        k -= self.lib_n;
        if self.pl_header() {
            if k == 0 {
                return Row::Header("Playlists");
            }
            k -= 1;
        }
        if self.all_playlists {
            if k == 0 {
                return Row::Nav(Nav::AllPlaylists);
            }
            k -= 1;
        }
        if self.fav.is_some() {
            if k == 0 {
                return Row::Favourites;
            }
            k -= 1;
        }
        if k < self.others {
            return Row::Playlist(k); // index among the non-favourite playlists; see `nth_other`
        }
        k -= self.others;
        if self.note && k == 0 { Row::Note } else { Row::Pad }
    }

    fn key(&self, s: &State, i: usize) -> String {
        match self.at(i) {
            Row::Header(h) => format!("h-{h}"),
            Row::PinsHead => "pins-head".into(),
            Row::Pin(k) => format!("pin-{}", s.settings.pins.get(k).map_or("", |p| p.id.as_str())),
            Row::Nav(n) => n.info().0.into(),
            Row::Favourites => format!("pl-{}", self.fav.and_then(|f| s.library_playlists.items.get(f)).map_or("", |c| c.id.as_str())),
            Row::Playlist(k) => format!("pl-{}", playlist_at(s, k).map_or("", |c| c.id.as_str())),
            Row::Note => "note".into(),
            Row::Pad => "pad".into(),
        }
    }
}

/// The `k`-th library playlist other than Favourite Songs.
fn playlist_at(s: &State, k: usize) -> Option<&Collection> {
    s.library_playlists.items.iter().filter(|c| !c.favorites).nth(k)
}

fn build_row(s: &State, r: &Rows, i: usize) -> El {
    let wrap = |e: El| row().w_full().px(10.0).child(e);
    let playlist = |c: &Collection| {
        let active = playlist_active(c, &s.route);
        // Rows rarely change: reuse the built row until its title or active state does.
        let drag = dnd::row_state(s, &c.id, c.editable);
        // The thumbnail is part of the key: the picture shown (by content, so a re-signed link or
        // a reload of the same picture rebuilds nothing), and whether it is still fading in.
        // Asked for every view, not only when the row builds: that holds the cover in memory
        // while the row shows, and once one has shown, `held` keeps it (never back to the tile).
        let cover = playlist_cover(s, c);
        let held = s.art.held(&c.id, cover, crate::art::SMALL_PX);
        let pic = held.as_ref().map(|(img, fresh)| (img.id(), *fresh));
        let version = (&c.title, active, drag, crate::theme::is_light(), crate::theme::over_picture(), cover.is_some(), pic);
        wrap(memo(format!("pl-{}", c.id), version, || playlist_item(s, c, active, held.as_ref())))
    };
    match r.at(i) {
        Row::Header(h) => header(h),
        Row::PinsHead => wrap(pins_head(s)),
        Row::Pin(k) => match s.settings.pins.get(k) {
            Some(p) => pin_item(s, p, k),
            None => col(),
        },
        Row::Nav(n) => wrap(nav_item(n, &s.route)),
        Row::Favourites => match r.fav.and_then(|f| s.library_playlists.items.get(f)) {
            Some(c) => playlist(c),
            None => col(),
        },
        Row::Playlist(k) => match playlist_at(s, k) {
            Some(c) => playlist(c),
            None => col(),
        },
        Row::Note => text("Loading\u{2026}").font(SMALL13).color(PAL.dim).px(20.0).py(8.0),
        Row::Pad => col().h(12.0),
    }
}

fn toggle_button(s: &State, key: &'static str) -> El {
    el().key(key).role("button").label("Toggle sidebar").focusable(true).size(32.0, 32.0).radius(8.0).center()
        .hover(crate::theme::ov(0.07)).cursor(Cursor::Pointer).help(super::keys::hint(s, "Toggle sidebar", super::keys::Action::ToggleSidebar))
        .on_press(super::narrow::toggle_sidebar)
        .child(icon(&icons::SIDEBAR, BUTTON_ICON).color(PAL.dim))
}

fn settings_button(s: &State, key: &'static str) -> El {
    el().key(key).role("button").label("Settings").focusable(true).size(32.0, 32.0).radius(8.0).center()
        .hover(crate::theme::ov(0.07)).cursor(Cursor::Pointer).help(super::keys::hint(s, "Settings", super::keys::Action::Settings))
        .on_press(|s: &mut State| super::settings::toggle(s))
        .child(icon(&icons::GEAR, BUTTON_ICON).color(PAL.dim))
}

/// `chrome_top`: the traffic-light band's height, or 0 where the system draws the window frame
/// (Linux) or none shows (macOS full screen).
pub fn sidebar(s: &State, rail: bool, chrome_top: f32, reduced: bool) -> El {
    if rail {
        return rail_view(s, chrome_top, reduced);
    }
    let mut top = col().w_full().px(10.0).gap(2.0);
    for n in TOP.into_iter().filter(|n| shown(s, n.info().0)) {
        top = top.child(nav_item(n, &s.route));
    }
    let r = Rows::of(s);
    let rows = col().key("sb-rows").growth(GrowthSpec::default())
        .virtual_rows(
            r.count(),
            ROW_H + 2.0,
            move |s: &State, i| r.key(s, i).into(),
            move |s: &State, i| build_row(s, &r, i),
        )
        .virtual_rows_version((
            r.count(),
            s.settings.pins.len(),
            r.fav,
            s.library_playlists.items.first().map(|c| c.id.clone()),
            s.ui.dnd.version(),
            s.settings.hidden_nav.len(),
            r.lib_n,
            s.local_only(),
        ));
    titlebar_band(col().key("sidebar").w(SIDEBAR_W).h_full().fill(PAL.sidebar), chrome_top)
        .child(
            row().px(20.0).pb(14.0).gap(10.0).center_y()
                .child(logo_tile(30.0))
                .child(text("Uzumaki").font(BRAND).color(PAL.text).grow(1.0))
                .child(settings_button(s, "sb-settings"))
                .child(toggle_button(s, "sb-toggle").mr(-8.0)),
        )
        .child(top)
        .child(dnd::sidebar_scroll(s, col().key("sb-scroll").grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(rows)))
}

/// A rail entry's height; each rail row adds 1 pt of padding above and below.
const RAIL_ITEM_H: f32 = 42.0;

fn rail_item(key: String, label: &str, active: bool, content: El) -> El {
    let (bg, _) = pill(active);
    el().key(key).role("button").label(label.to_string()).focusable(true).size(56.0, RAIL_ITEM_H).radius(10.0).center().fill(bg)
        .hover(crate::theme::ov(0.05)).press(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
        .help(label.to_string()).child(content)
}

fn rail_nav(n: Nav, route: &Route) -> El {
    let (key, ic, label, target) = n.info();
    hideable(rail_item(key.to_string(), label, n.active(route), icon(ic, RAIL_ICON).color(PAL.accent)).on_press(move |s: &mut State| go(s, &target)), key, label)
}

/// A rail row, in order: the top entries, the Library entries, All Playlists and Favourite
/// Songs, the pins, then every other playlist.
#[derive(Clone, Copy)]
enum RailRow {
    Nav(Nav),
    Sep,
    Fav,
    Pin(usize),
    /// The `k`-th playlist other than Favourite Songs (`playlist_at`).
    Playlist(usize),
    Pad,
}

/// The rail's rows: a short fixed head (at most about 25), then the playlists by count, so a
/// long list costs only the rows on screen.
struct RailRows {
    head: Vec<RailRow>,
    playlists: usize,
}

impl RailRows {
    fn of(s: &State) -> Self {
        let mut head: Vec<RailRow> = TOP.into_iter().filter(|n| shown(s, n.info().0)).map(RailRow::Nav).collect();
        head.push(RailRow::Sep);
        head.extend(library(s).iter().filter(|n| shown(s, n.info().0)).map(|&n| RailRow::Nav(n)));
        if !s.local_only() {
            head.push(RailRow::Sep);
        }
        if shown(s, "nav-playlists") {
            head.push(RailRow::Nav(Nav::AllPlaylists));
        }
        let items = &s.library_playlists.items;
        if items.iter().any(|c| c.favorites) && shown(s, "favourites") {
            head.push(RailRow::Fav);
        }
        if s.settings.pins_open && !s.settings.pins.is_empty() && shown(s, "pins") {
            head.push(RailRow::Sep);
            head.extend((0..s.settings.pins.len().min(8)).map(RailRow::Pin));
        }
        let playlists = if shown(s, "playlists") { items.iter().filter(|c| !c.favorites).count() } else { 0 };
        if playlists > 0 {
            head.push(RailRow::Sep);
        }
        RailRows { head, playlists }
    }

    fn count(&self) -> usize {
        self.head.len() + self.playlists + 1
    }

    fn at(&self, i: usize) -> RailRow {
        match self.head.get(i) {
            Some(&r) => r,
            None if i - self.head.len() < self.playlists => RailRow::Playlist(i - self.head.len()),
            None => RailRow::Pad,
        }
    }

    fn key(&self, s: &State, i: usize) -> String {
        match self.at(i) {
            RailRow::Nav(n) => n.info().0.into(),
            RailRow::Sep => format!("rail-sep-{i}"),
            RailRow::Fav => "pl-fav".into(),
            RailRow::Pin(k) => format!("pin-{}", s.settings.pins.get(k).map_or("", |p| p.id.as_str())),
            RailRow::Playlist(k) => format!("rail-pl-{}", playlist_at(s, k).map_or("", |c| c.id.as_str())),
            RailRow::Pad => "rail-pad".into(),
        }
    }
}

/// A playlist in the rail: its picture (as a pin's) or the playlist icon, the name as a tooltip,
/// and the full sidebar's click, menu and drop target.
fn rail_playlist(s: &State, c: &Collection, active: bool, held: Option<&(rune::Image, bool)>) -> El {
    let (c1, c2) = (c.clone(), c.clone());
    let content = match playlist_cover(s, c) {
        Some(_) => cover(held, &c.title, 28.0),
        None => icon(&icons::PLAYLIST, RAIL_ICON).color(PAL.accent),
    };
    dnd::playlist_target(s, rail_item(format!("rail-pl-{}", c.id), &c.title, active, content), &c.id)
        .on_press(move |s: &mut State| s.open_collection(&c1))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| ctx_menu::open_collection(s, &c2, [p.x, p.y]))
}

fn build_rail_row(s: &State, r: &RailRows, i: usize) -> El {
    // Row spacing is padding inside each row (virtual rows take no gap).
    let wrap = |e: El| row().w_full().py(1.0).justify_center().child(e);
    match r.at(i) {
        RailRow::Nav(n) => wrap(rail_nav(n, &s.route)),
        RailRow::Sep => row().w_full().py(8.0).justify_center().child(el().w(36.0).h(1.0).fill(PAL.divider)),
        RailRow::Fav => match s.library_playlists.items.iter().find(|c| c.favorites) {
            Some(f) => {
                let f1 = f.clone();
                wrap(
                    rail_item("pl-fav".into(), &f.title, playlist_active(f, &s.route), icon(&icons::FAVORITES, RAIL_ICON).color(PAL.accent))
                        .on_press(move |s: &mut State| s.open_collection(&f1)),
                )
            }
            None => col(),
        },
        RailRow::Pin(k) => match s.settings.pins.get(k) {
            Some(p) => {
                let item = rail_item(format!("pin-{}", p.id), &p.title, pin_active(p, &s.route), thumb(s, p, 28.0));
                let item = if dnd::is_pin_playlist(p) { dnd::playlist_target(s, item, &p.id) } else { item };
                // Drag to reorder, as in the full sidebar. The rows are the item and its padding apart.
                dnd::pin_row(s, row().w_full().py(1.0).justify_center(), pin_handlers(item, p), k, p, RAIL_ITEM_H + 2.0)
            }
            None => col(),
        },
        RailRow::Playlist(k) => match playlist_at(s, k) {
            Some(c) => {
                // As in the full sidebar: reuse the built row until what it draws changes.
                let active = playlist_active(c, &s.route);
                let drag = dnd::row_state(s, &c.id, c.editable);
                let cover = playlist_cover(s, c);
                let held = s.art.held(&c.id, cover, crate::art::SMALL_PX);
                let pic = held.as_ref().map(|(img, fresh)| (img.id(), *fresh));
                let version = (&c.title, active, drag, crate::theme::is_light(), crate::theme::over_picture(), cover.is_some(), pic);
                wrap(memo(format!("rail-pl-{}", c.id), version, || rail_playlist(s, c, active, held.as_ref())))
            }
            None => col(),
        },
        RailRow::Pad => col().h(12.0),
    }
}

fn rail_view(s: &State, chrome_top: f32, reduced: bool) -> El {
    let r = std::rc::Rc::new(RailRows::of(s));
    let (kr, version) = (r.clone(), (
        r.count(),
        r.head.len(),
        s.settings.pins.len(),
        s.library_playlists.items.first().map(|c| c.id.clone()),
        s.ui.dnd.version(),
        s.settings.hidden_nav.len(),
        s.local_only(),
    ));
    let rows = col().key("sb-rail-rows").growth(GrowthSpec::default())
        .virtual_rows(r.count(), 44.0, move |s: &State, i| kr.key(s, i).into(), move |s: &State, i| build_rail_row(s, &r, i))
        .virtual_rows_version(version);
    // The contents shake (in a narrow window, where the rail can't expand) and the strip stays.
    let inner = col().key("sb-rail").w_full().grow(1.0).min_h(0.0).items_center()
        .child(logo_tile(36.0))
        .child(toggle_button(s, "sb-toggle").mt(10.0))
        .child(settings_button(s, "sb-settings").mt(4.0).mb(12.0))
        .child(dnd::sidebar_scroll(s, col().key("sb-scroll").grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(rows)));
    titlebar_band(col().key("sidebar").w(RAIL_W).h_full().fill(PAL.sidebar).items_center(), chrome_top)
        .child(super::narrow::bind(s, super::narrow::Target::Rail, inner, reduced))
}

/// The sidebar's top: 16 pt of padding, or the traffic-light band and a little less padding.
fn titlebar_band(sidebar: El, chrome_top: f32) -> El {
    if chrome_top > 0.0 {
        sidebar.child(el().key("sb-titlebar").w_full().h(chrome_top).window_drag()).child(el().h(6.0))
    } else {
        sidebar.pt(16.0)
    }
}

#[cfg(test)]
mod hide_tests {
    use super::*;
    use crate::models::CollectionKind;

    fn pl(id: &str, favorites: bool) -> Collection {
        Collection {
            kind: CollectionKind::Playlist,
            id: id.into(),
            title: id.into(),
            subtitle: String::new(),
            artwork: None,
            year: None,
            genre: None,
            description: None,
            track_count: None,
            modified: None,
            editable: true,
            favorites,
            public: None,
            tracks: Vec::new(),
            artists: Vec::new(),
        }
    }

    fn keys(s: &State) -> Vec<String> {
        let r = Rows::of(s);
        (0..r.count()).map(|i| r.key(s, i)).filter(|k| k != "pad").collect()
    }

    #[test]
    fn hidden_entries_and_empty_headings_leave_the_sidebar() {
        let mut s = State::offline();
        s.library_playlists.items = vec![pl("p.a", false), pl("p.fav", true), pl("p.b", false)];
        let all = keys(&s);
        assert_eq!(all.first().map(String::as_str), Some("h-Library"));
        assert!(all.contains(&"pl-p.fav".to_string()) && all.contains(&"pl-p.b".to_string()));
        for (id, _) in NAV_ITEMS.iter().filter(|(id, _)| id.starts_with("nav-") && *id != "nav-playlists") {
            s.set_nav_hidden(id, true);
        }
        let k = keys(&s);
        assert!(!k.contains(&"h-Library".to_string()), "nothing left under Library: no heading");
        assert_eq!(k, ["h-Playlists", "nav-playlists", "pl-p.fav", "pl-p.a", "pl-p.b"]);
        s.set_nav_hidden("favourites", true);
        s.set_nav_hidden("playlists", true);
        assert_eq!(keys(&s), ["h-Playlists", "nav-playlists"]);
        s.set_nav_hidden("nav-playlists", true);
        assert!(keys(&s).is_empty());
        s.set_nav_hidden("playlists", false);
        assert_eq!(keys(&s), ["h-Playlists", "pl-p.a", "pl-p.b"], "Favourite Songs stays hidden on its own");
    }

    #[test]
    fn local_only_keeps_search_and_local_files_and_the_choices() {
        let mut s = State::offline();
        s.library_playlists.items = vec![pl("p.a", false), pl("p.fav", true)];
        s.settings.pins.push(crate::settings::Pin { kind: PinKind::Album, id: "a.1".into(), title: "A".into(), artwork: None });
        s.set_nav_hidden("nav-radio", true);
        s.set_music_source(crate::settings::MusicSource::LocalOnly);
        assert_eq!(keys(&s), ["h-Library", "nav-local-artists", "nav-local-albums", "nav-local", "nav-folders"]);
        // The rail lists the same four, and Local Files is called Songs there.
        assert_eq!(library(&s).iter().map(|n| n.info().2).collect::<Vec<_>>(), ["Artists", "Albums", "Songs", "Folders"]);
        assert_eq!(nav_choices(true).iter().map(|c| c.1).collect::<Vec<_>>(), ["Search", "Artists", "Albums", "Songs", "Folders"]);
        let top: Vec<_> = TOP.iter().filter(|n| shown(&s, n.info().0)).map(|n| n.info().0).collect();
        assert_eq!(top, ["nav-search"]);
        assert_eq!(s.settings.pins.len(), 1, "pins stay in the settings");
        s.set_music_source(crate::settings::MusicSource::Apple);
        assert!(keys(&s).contains(&"pins-head".to_string()) && keys(&s).contains(&"pl-p.a".to_string()));
        assert!(!shown(&s, "nav-radio"), "the user's own hidden entries stay hidden");
    }

    #[test]
    fn apple_mode_keeps_its_library_entries_and_never_shows_the_local_tabs() {
        let mut s = State::offline();
        s.library_playlists.items = vec![pl("p.a", false)];
        let k = keys(&s);
        let lib: Vec<&str> = k.iter().map(String::as_str).filter(|k| k.starts_with("nav-")).collect();
        assert_eq!(lib, ["nav-recent", "nav-artists", "nav-albums", "nav-songs", "nav-mfy", "nav-local", "nav-playlists"]);
        assert_eq!(Nav::LocalFiles.info().2, "Local Files");
        let chips: Vec<&str> = nav_choices(false).iter().map(|c| c.0).collect();
        assert_eq!(chips.len(), 14, "the same choices as before");
        assert!(!chips.contains(&"nav-folders") && !chips.contains(&"nav-local-albums"));
    }

    fn rail_keys(s: &State) -> Vec<String> {
        let r = RailRows::of(s);
        (0..r.count()).map(|i| r.key(s, i)).filter(|k| k != "rail-pad" && !k.starts_with("rail-sep")).collect()
    }

    #[test]
    fn the_rail_lists_every_playlist_after_the_pins() {
        let mut s = State::offline();
        s.library_playlists.items = vec![pl("p.a", false), pl("p.fav", true), pl("p.b", false)];
        s.settings.pins.push(crate::settings::Pin { kind: PinKind::Album, id: "a.1".into(), title: "A".into(), artwork: None });
        s.settings.pins_open = true;
        let k = rail_keys(&s);
        let tail: Vec<&str> = k.iter().map(String::as_str).skip_while(|k| *k != "nav-playlists").collect();
        assert_eq!(tail, ["nav-playlists", "pl-fav", "pin-a.1", "rail-pl-p.a", "rail-pl-p.b"]);
        s.set_nav_hidden("playlists", true);
        assert!(!rail_keys(&s).iter().any(|k| k == "rail-pl-p.a"), "hidden with the full sidebar's \"Your playlists\"");
        s.set_nav_hidden("playlists", false);
        s.settings.pins_open = false;
        assert!(rail_keys(&s).ends_with(&["pl-fav".to_string(), "rail-pl-p.a".to_string(), "rail-pl-p.b".to_string()]));
    }

    #[test]
    fn a_rail_playlist_opens_on_click_and_shows_active() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "sidebar-collapsed-playlists").unwrap().1)(&mut s);
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1180.0, 900.0), rune::Env { system_dark: true, scale: 1.0 });
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let title = rt.state.library_playlists.items.iter().find(|c| c.id == "p.1").map(|c| c.title.clone());
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("rail-pl-p.1") && e.label == title).expect("pl-p.1 in the rail").rect;
        rt.pointer_down(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_up(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert!(matches!(&rt.state.route, Route::Collection { id, .. } if id == "p.1"), "{:?}", rt.state.route);
        let c = rt.state.library_playlists.items.iter().find(|c| c.id == "p.1").unwrap();
        assert!(playlist_active(c, &rt.state.route));
    }

    /// Fold the sidebar into the rail and open it again: each playlist row must show its name.
    /// The rail once memoized its rows under the full sidebar's keys, so the full rows came back as icons.
    #[test]
    fn playlist_rows_keep_their_names_after_the_rail() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "sidebar-collapsed-playlists").unwrap().1)(&mut s);
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1180.0, 900.0), rune::Env { system_dark: true, scale: 1.0 });
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        rt.update(|s, _| s.settings.sidebar_collapsed = false);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        let c = rt.state.library_playlists.items.iter().find(|c| c.id == "p.1").unwrap().clone();
        let all = rt.elements();
        let row = all.iter().find(|e| e.key.as_deref() == Some("pl-p.1") && !e.exiting).expect("pl-p.1 in the full sidebar");
        let pre = format!("{}/", row.path);
        let named = all.iter().any(|e| e.path.starts_with(&pre) && !e.exiting && e.text.as_deref() == Some(c.title.as_str()));
        assert!(named, "the row shows \"{}\"", c.title);
    }

    fn art(template: &str) -> Artwork {
        Artwork { template: template.into(), bg: None }
    }

    #[test]
    fn a_sidebar_playlist_draws_the_users_picture_else_the_lists_else_nothing() {
        let mut s = State::offline();
        let mut a = pl("p.a", false);
        a.artwork = Some(art("https://x.invalid/fresh-{w}x{h}.jpg"));
        s.library_playlists.items = vec![a.clone(), pl("p.b", false)];
        assert_eq!(playlist_cover(&s, &a), Some(&art("https://x.invalid/fresh-{w}x{h}.jpg")));
        assert_eq!(playlist_cover(&s, &s.library_playlists.items[1]), None, "no artwork keeps the icon");
        s.settings.playlist_art.insert("p.b".into(), art("file:///tmp/mine.jpg"));
        s.settings.playlist_art.insert("p.a".into(), art("file:///tmp/mine-a.jpg"));
        assert_eq!(playlist_cover(&s, &s.library_playlists.items[1]), Some(&art("file:///tmp/mine.jpg")), "a picture of the user's own");
        assert_eq!(playlist_cover(&s, &a), Some(&art("file:///tmp/mine-a.jpg")), "and it wins over Apple's");
    }

    /// The leading element of row `key`: the size of the first descendant with a square rect.
    fn lead_size(rt: &rune::Runtime<State>, key: &str) -> f32 {
        let all = rt.elements();
        let row = all.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no row {key}"));
        // The icon slot (19 pt) holds the 19 pt icon or the 24 pt thumbnail.
        all.iter()
            .filter(|e| e.path.starts_with(&format!("{}/", row.path)) && e.rect[2] == e.rect[3] && (19.0..=24.0).contains(&e.rect[2]))
            .map(|e| e.rect[2])
            .fold(0.0, f32::max)
    }

    #[test]
    fn playlists_with_artwork_get_a_pin_sized_thumbnail_and_the_others_keep_the_icon() {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "sidebar-playlist-art").unwrap().1)(&mut s);
        let items = s.library_playlists.items.clone();
        let with = items.iter().find(|c| c.artwork.is_some() && !c.favorites).expect("one with artwork");
        let without = items.iter().find(|c| c.artwork.is_none() && !c.favorites).expect("one without");
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1180.0, 900.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(lead_size(&rt, &format!("pl-{}", with.id)), THUMB, "a thumbnail, as big as a pin's");
        assert_eq!(lead_size(&rt, &format!("pl-{}", without.id)), ICON, "the playlist icon");
        assert_eq!(lead_size(&rt, "pin-a2"), THUMB, "a pin is unchanged");
    }

    /// Signed blobstore link of an uploaded picture, as Apple sends it (`sig` stands for the signature).
    fn signed(upload: &str, sig: &str) -> Artwork {
        art(&format!("https://store-033.blobstore.apple.com/sq-mq-us-033-000002/{upload}/image?X-Amz-Date={sig}&X-Amz-Expires=86400"))
    }

    /// Per playlist id: does its row draw a picture (not the tile), and the picture's opacity.
    fn pictures(rt: &rune::Runtime<State>, ids: &[&str]) -> Vec<Option<f32>> {
        let all = rt.elements();
        ids.iter()
            .map(|id| {
                let row = all.iter().find(|e| e.key.as_deref() == Some(&format!("pl-{id}")) && !e.exiting)?;
                let pre = format!("{}/", row.path);
                all.iter().find(|e| e.path.starts_with(&pre) && e.key.as_deref() == Some("thumb-pic") && !e.exiting).map(|e| e.opacity)
            })
            .collect()
    }

    /// Start-up, replayed: the cached list's links, pictures arriving late, the fresh list with
    /// re-signed links, a changed picture still loading, and every cover let go from memory.
    /// Each frame, a row that has drawn its picture must still draw one (never the tile again),
    /// and only the first appearance fades.
    #[test]
    fn a_thumbnail_once_shown_never_goes_back_to_the_tile() {
        let mut s = State::offline();
        s.art.test_live(rune::ImageCache::new(std::sync::Arc::new(|| {})));
        let mut a = pl("p.a", false);
        a.artwork = Some(signed("aa/bb/uuid-a", "20261111T080000Z"));
        let mut b = pl("p.b", false);
        b.artwork = Some(art("https://is1-ssl.mzstatic.com/image/thumb/b/{w}x{h}bb.jpg"));
        let c = pl("p.c", false);
        s.library_playlists.items = vec![a.clone(), b.clone(), c];
        let ids = ["p.a", "p.b", "p.c"];
        let px = crate::art::SMALL_PX;
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1180.0, 900.0), rune::Env { system_dark: true, scale: 2.0 });

        let mut shown = [false; 3];
        let mut frames = 0;
        let mut log: Vec<String> = Vec::new();
        let mut step = |rt: &mut rune::Runtime<State>, what: &str, n: usize| {
            for _ in 0..n {
                rt.frame(if frames == 0 { 0.0 } else { 1.0 / 60.0 });
                frames += 1;
                let now = pictures(rt, &ids);
                for (i, p) in now.iter().enumerate() {
                    assert!(!(shown[i] && p.is_none()), "{} went back to the tile at frame {frames} ({what}); log: {log:?}", ids[i]);
                    shown[i] |= p.is_some();
                }
                log.push(format!("{frames} {what}: {now:?}"));
            }
            pictures(rt, &ids)
        };

        let p = step(&mut rt, "cached list, nothing in memory", 3);
        assert_eq!(p, vec![None, None, None], "tiles (and the icon for p.c) until a picture lands");

        // A's picture lands (from the disk cache): it fades in once.
        rt.update(|s, _| s.art.test_arrive(&signed("aa/bb/uuid-a", "20261111T080000Z"), px, 8, [200, 40, 40]));
        let p = step(&mut rt, "A arrives", 1);
        let first = p[0].expect("A draws its picture");
        assert!(first < 1.0, "the first appearance fades in (opacity {first})");
        step(&mut rt, "A fading", 30);

        // The fresh list: A's link re-signed. Same picture, so nothing to load and no blink.
        rt.update(|s, _| s.library_playlists.items[0].artwork = Some(signed("aa/bb/uuid-a", "20261112T090000Z")));
        let p = step(&mut rt, "fresh list, A re-signed", 5);
        assert!(p[0].is_some_and(|o| o > 0.99), "A keeps its picture at full strength: {p:?}");

        // B lands, then the user changes B's picture: the new one is still loading.
        rt.update(|s, _| s.art.test_arrive(&art("https://is1-ssl.mzstatic.com/image/thumb/b/{w}x{h}bb.jpg"), px, 8, [40, 200, 40]));
        step(&mut rt, "B arrives", 30);
        rt.update(|s, _| s.library_playlists.items[1].artwork = Some(signed("cc/dd/uuid-b2", "20261112T090000Z")));
        let p = step(&mut rt, "B's new picture loading", 5);
        assert!(p[1].is_some_and(|o| o > 0.99), "B keeps the old picture while the new one loads: {p:?}");
        rt.update(|s, _| s.art.test_arrive(&signed("cc/dd/uuid-b2", "20261112T090000Z"), px, 8, [40, 40, 200]));
        let p = step(&mut rt, "B's new picture arrives", 5);
        assert!(p[1].is_some_and(|o| o > 0.99), "and swaps in place, no second fade: {p:?}");

        // Memory pressure (a page full of covers) lets every cover go; views keep running.
        rt.update(|s, _| s.art.test_evict_all());
        step(&mut rt, "all covers let go", 3);
        rt.update(|_, _| {});
        let p = step(&mut rt, "another view", 3);
        assert!(p[0].is_some_and(|o| o > 0.99) && p[1].is_some_and(|o| o > 0.99) && p[2].is_none(), "both pictures stay, p.c keeps its icon: {p:?}");
    }
}
