//! Generic context menus (album / playlist cards and headers, pins, artists, stations): a list
//! of labelled actions in an in-app overlay. The song menu lives in `menu.rs`.

use std::rc::Rc;

use rune::{Drag, El, Phase, Tx, el, icon};

use super::{icons, menu};
use crate::models::{Artist, Collection, CollectionKind, Song, Station};
use crate::settings::{Pin, PinKind};
use crate::state::{LocalWhat, State};
use crate::theme::*;

type Act = Rc<dyn Fn(&mut State)>;

/// One entry; `act: None` is a separator.
pub struct CtxItem {
    label: String,
    act: Option<Act>,
    /// A destructive item: the first click shows this label instead, the second acts.
    confirm: Option<String>,
    /// Share (macOS): the item opens the Share submenu for this link instead of acting.
    sub: Option<crate::share::Link>,
}

pub struct CtxMenu {
    at: [f32; 2],
    items: Vec<CtxItem>,
    /// The destructive item clicked once.
    armed: Option<usize>,
    /// The item whose submenu is showing.
    open_sub: Option<usize>,
}

fn it(label: impl Into<String>, f: impl Fn(&mut State) + 'static) -> CtxItem {
    CtxItem { label: label.into(), act: Some(Rc::new(f)), confirm: None, sub: None }
}

/// An item that asks first: "Delete Playlist" → "Click again to delete".
fn danger(label: impl Into<String>, confirm: impl Into<String>, f: impl Fn(&mut State) + 'static) -> CtxItem {
    CtxItem { label: label.into(), act: Some(Rc::new(f)), confirm: Some(confirm.into()), sub: None }
}

fn sep() -> CtxItem {
    CtxItem { label: String::new(), act: None, confirm: None, sub: None }
}

/// Scenes: shows the submenu of the open menu's Share item.
pub fn open_share(s: &mut State) {
    if let Some(m) = &mut s.ui.ctx {
        m.open_sub = m.items.iter().position(|i| i.sub.is_some());
    }
}

/// Share, as in Apple Music: on macOS a submenu with Copy Link and the system's sharing services,
/// elsewhere a plain Copy Link. Nothing when the item has no page to link to.
pub fn share_item(link: crate::share::Link) -> Option<CtxItem> {
    if link == crate::share::Link::Hidden {
        return None;
    }
    if !crate::share::submenu() {
        return Some(it("Copy Link", move |s| s.copy_link(&link)));
    }
    Some(CtxItem { label: "Share".into(), act: Some(Rc::new(|_| {})), confirm: None, sub: Some(link) })
}

pub fn open(s: &mut State, at: [f32; 2], items: Vec<CtxItem>) {
    s.ui.menu = None;
    crate::share::prepare();
    s.ui.ctx = Some(CtxMenu { at, items, armed: None, open_sub: None });
}

fn pin_item(s: &State, id: &str, pin: impl Fn(&mut State) + 'static) -> CtxItem {
    if s.is_pinned(id) {
        let id = id.to_string();
        it("Unpin from Sidebar", move |s| s.unpin(&id))
    } else {
        it("Pin to Sidebar", pin)
    }
}

/// Play, Shuffle, Play Next and Add to Queue for a local album, artist or folder (local only
/// mode). The songs are looked up when an item is clicked. No pins, favourites or library
/// actions: those are Apple Music's.
pub fn local_items(what: LocalWhat, source: String) -> Vec<CtxItem> {
    let what = Rc::new(what);
    let source = Rc::new(source);
    let play = {
        let (w, src) = (what.clone(), source.clone());
        move |s: &mut State| {
            let songs = s.local_songs(&w);
            s.play_list(&songs, 0, &src);
        }
    };
    let shuffle = {
        let (w, src) = (what.clone(), source.clone());
        move |s: &mut State| {
            let songs = s.local_songs(&w);
            s.shuffle_play(&songs, &src);
        }
    };
    let (w3, w4) = (what.clone(), what.clone());
    let mut v = vec![
        it("Play", play),
        it("Shuffle", shuffle),
        it("Play Next", move |s| {
            let songs = s.local_songs(&w3);
            s.play_next(&songs);
        }),
        it("Add to Queue", move |s| {
            let songs = s.local_songs(&w4);
            s.add_to_queue(&songs);
        }),
    ];
    if let LocalWhat::Folder(path) | LocalWhat::FolderFiles(path) = &*what {
        let path = path.clone();
        v.push(sep());
        v.push(it("Show in Folder", move |_s| crate::desktop::show_in_folder(&path)));
    }
    v
}

/// A local album's menu: the local items and Go to Artist.
fn local_album_items(c: &Collection) -> Vec<CtxItem> {
    let mut v = local_items(LocalWhat::Album(c.id.clone()), c.title.clone());
    if let Some(a) = c.artists.first().cloned() {
        v.push(sep());
        v.push(it("Go to Artist", move |s| super::artist_link::open(s, &a)));
    }
    v
}

/// Play, Shuffle, queue actions (when the tracks are known) and Pin.
pub fn collection_items(s: &State, c: &Collection) -> Vec<CtxItem> {
    items_of_collection(s, c, true)
}

/// `pin`: with Pin/Unpin (a pin's own menu has its pin items instead).
fn items_of_collection(s: &State, c: &Collection, pin: bool) -> Vec<CtxItem> {
    if crate::local::view::is_album_id(&c.id) {
        return local_album_items(c);
    }
    let tracks: Vec<Song> = if c.tracks.is_empty() {
        s.collections.get(&c.id).and_then(|l| l.ready()).map(|x| x.tracks.clone()).unwrap_or_default()
    } else {
        c.tracks.clone()
    };
    let c1 = c.clone();
    let c2 = c.clone();
    let mut v = vec![
        it("Play", move |s| s.play_collection(&c1, 0)),
        it("Shuffle", move |s| s.shuffle_collection(&c2)),
    ];
    if !tracks.is_empty() {
        let t2 = tracks.clone();
        v.push(it("Play Next", move |s| s.play_next(&tracks)));
        v.push(it("Add to Queue", move |s| s.add_to_queue(&t2)));
    }
    v.push(sep());
    if State::can_set_picture(c) {
        let c7 = c.clone();
        let label = if State::can_rename(c) { "Edit Playlist\u{2026}" } else { "Change Picture\u{2026}" };
        v.push(it(label, move |s| super::new_playlist::open_edit(s, &c7)));
    }
    if pin {
        let c3 = c.clone();
        v.push(pin_item(s, &c.id, move |s| s.pin_collection(&c3)));
    }
    if s.session.authorized && !c.favorites {
        let c6 = c.clone();
        v.push(it(if s.is_favourite_collection(c) { "Undo Favourite" } else { "Favourite" }, move |s| s.toggle_favourite_collection(&c6)));
    }
    if !crate::state::in_library(&c.id) {
        let c4 = c.clone();
        v.push(it("Add to Library", move |s| s.add_collection_to_library(&c4)));
    }
    v.extend(share_item(crate::share::collection_link(c, s.session.storefront.as_deref())));
    if crate::state::in_library(&c.id) && (c.id.starts_with("l.") || (c.id.starts_with("p.") && c.editable)) {
        let c5 = c.clone();
        let (label, confirm) = if c.id.starts_with("p.") {
            ("Delete Playlist", "Click again to delete it")
        } else {
            ("Remove from Library", "Click again to remove it")
        };
        v.push(sep());
        v.push(danger(label, confirm, move |s| s.remove_collection_from_library(&c5)));
    }
    v
}

pub fn open_collection(s: &mut State, c: &Collection, at: [f32; 2]) {
    let items = collection_items(s, c);
    open(s, at, items);
}

pub fn artist_items(s: &State, a: &Artist) -> Vec<CtxItem> {
    if crate::local::view::is_artist_id(&a.id) {
        return local_items(LocalWhat::Artist(a.id.clone()), a.name.clone());
    }
    let a2 = a.clone();
    let mut v = vec![pin_item(s, &a.id, move |s| s.pin_artist(&a2))];
    v.extend(share_item(crate::share::artist_link(a, s.session.storefront.as_deref())));
    v
}

pub fn open_artist(s: &mut State, a: &Artist, at: [f32; 2]) {
    let items = artist_items(s, a);
    open(s, at, items);
}

/// A menu of `artists`, each opening its page (the link on an album or song with several).
pub fn open_artist_choice(s: &mut State, at: [f32; 2], artists: &[Artist]) {
    let items = artists
        .iter()
        .map(|a| {
            let a = a.clone();
            it(a.name.clone(), move |s| super::artist_link::open(s, &a))
        })
        .collect();
    open(s, at, items);
}

/// Right-click on a folder row.
pub fn open_folder(s: &mut State, path: &std::path::Path, at: [f32; 2]) {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    open(s, at, local_items(LocalWhat::Folder(path.to_path_buf()), name));
}

pub fn open_station(s: &mut State, st: &Station, at: [f32; 2]) {
    let (s1, s2) = (st.clone(), st.clone());
    let mut items = vec![it("Play", move |s| s.play_station(&s1)), sep(), pin_item(s, &st.id, move |s| s.pin_station(&s2))];
    items.extend(share_item(crate::share::station_link(st, s.session.storefront.as_deref())));
    open(s, at, items);
}

/// Right-click on a sidebar entry.
pub fn open_nav(s: &mut State, id: &'static str, label: &'static str, at: [f32; 2]) {
    let items = vec![
        it(format!("Hide “{label}” from Sidebar"), move |s| {
            s.set_nav_hidden(id, true);
            s.toast = Some(format!("“{label}” is hidden. Settings → Appearance shows it again."));
        }),
        sep(),
        it("Edit Sidebar\u{2026}", |s| super::settings::open_tab(s, super::settings::Tab::Appearance)),
    ];
    open(s, at, items);
}

/// The album or playlist a pin stands for: the library's or loaded copy when there is one (it
/// knows whether the list is editable, Favourite Songs, ...), else what the pin remembers.
fn pin_collection(s: &State, pin: &Pin) -> Collection {
    let listed = match pin.kind {
        PinKind::Playlist => s.library_playlists.items.iter().find(|c| c.id == pin.id),
        _ => s.library_albums.items.iter().find(|c| c.id == pin.id),
    };
    let loaded = || s.collections.get(&pin.id).and_then(|l| l.ready());
    listed.or_else(loaded).cloned().unwrap_or_else(|| Collection {
        kind: if pin.kind == PinKind::Playlist { CollectionKind::Playlist } else { CollectionKind::Album },
        id: pin.id.clone(),
        title: pin.title.clone(),
        subtitle: String::new(),
        artwork: pin.artwork.clone(),
        year: None,
        genre: None,
        description: None,
        track_count: None,
        modified: None,
        editable: false,
        favorites: false,
        public: None,
        tracks: Vec::new(),
        artists: Vec::new(),
    })
}

/// Right-click on a pin (full sidebar or rail): the menu of what it pins (an album or playlist
/// row's, a station's Play), then Unpin, Move Up, Move Down.
pub fn pin_items(s: &State, id: &str) -> Vec<CtxItem> {
    let pins = &s.settings.pins;
    let Some(i) = pins.iter().position(|p| p.id == id) else { return Vec::new() };
    let pin = &pins[i];
    let last = pins.len() - 1;
    let mut items = match pin.kind {
        PinKind::Album | PinKind::Playlist => items_of_collection(s, &pin_collection(s, pin), false),
        PinKind::Station => {
            let st = Station { id: pin.id.clone(), name: pin.title.clone(), artwork: pin.artwork.clone(), live: false };
            let mut v = vec![it("Play", move |s| s.play_station(&st))];
            v.extend(share_item(crate::share::link_of_station(&pin.id, &pin.title, s.session.storefront.as_deref())));
            v
        }
        PinKind::Artist => Vec::new(),
    };
    if !items.is_empty() {
        items.push(sep());
    }
    let id1 = id.to_string();
    items.push(it("Unpin", move |s| s.unpin(&id1)));
    if i > 0 {
        let id2 = id.to_string();
        items.push(it("Move Up", move |s| s.move_pin(&id2, -1)));
    }
    if i < last {
        let id3 = id.to_string();
        items.push(it("Move Down", move |s| s.move_pin(&id3, 1)));
    }
    items
}

pub fn open_pin(s: &mut State, id: &str, at: [f32; 2]) {
    let items = pin_items(s, id);
    if !items.is_empty() {
        open(s, at, items);
    }
}

/// A round "…" button that opens the menu `items` builds, at the button.
pub fn more_button(key: &'static str, size: f32, items: impl Fn(&State) -> Vec<CtxItem> + 'static) -> El {
    el().key(key).role("button").label("More").focusable(true).size(size, size).radius(size / 2.0).center()
        .fill(PAL.nav_active).hover(crate::theme::ov(0.1)).cursor(rune::Cursor::Pointer)
        .on_drag(move |s: &mut State, d: &mut Drag, _tx: &mut Tx| {
            if d.phase == Phase::Up && !d.cancelled() {
                let at = d.window_point();
                let list = items(s);
                open(s, at, list);
            }
        })
        .child(icon(&icons::MORE, 16.0).color(PAL.text2))
}

pub fn overlay(s: &State, win: [f32; 2]) -> Option<El> {
    let m = s.ui.ctx.as_ref()?;
    let h = m.items.iter().map(|i| if i.act.is_some() { menu::ITEM_H } else { 9.0 }).sum::<f32>() + 12.0;
    let x = m.at[0].min(win[0] - menu::W - 8.0).max(8.0);
    let y = menu::place_y(m.at[1], h, win[1]);
    let mut panel = menu::panel(x, y);
    let mut sub_at = None;
    let mut top = 6.0;
    for (i, item) in m.items.iter().enumerate() {
        top += if item.act.is_some() { menu::ITEM_H } else { 9.0 };
        panel = panel.child(match &item.act {
            None => menu::sep(),
            Some(act) => {
                let act = act.clone();
                let armed = m.armed == Some(i);
                let label = match (&item.confirm, armed) {
                    (Some(c), true) => c.as_str(),
                    _ => item.label.as_str(),
                };
                let asks = item.confirm.is_some();
                if item.sub.is_some() {
                    // Like the song menu's Add to Playlist: the submenu opens when the pointer
                    // rests on the item (or it is clicked) and closes when another item is reached.
                    let shown = m.open_sub == Some(i);
                    if shown {
                        sub_at = Some((i, top - menu::ITEM_H));
                    }
                    let e = menu::item(format!("ctx-{i}"), label, true).on_press(move |s: &mut State| {
                        if let Some(m) = &mut s.ui.ctx {
                            m.open_sub = Some(i); // the pointer resting on it opened it already
                        }
                    });
                    let e = if shown { e.fill(crate::theme::ov(0.09)) } else { e };
                    e.on_hover_change(move |s: &mut State, inside, tx: &mut Tx| {
                        if let (true, Some(m)) = (inside, &mut s.ui.ctx) {
                            m.open_sub = Some(i);
                            tx.instant();
                        }
                    })
                } else {
                    let e = menu::item(format!("ctx-{i}"), label, false).on_press(move |s: &mut State| {
                        if asks && !armed {
                            if let Some(m) = &mut s.ui.ctx {
                                m.armed = Some(i);
                            }
                            return;
                        }
                        s.ui.ctx = None;
                        act(s);
                    });
                    let e = if armed { e.fill(PAL.danger_bg) } else { e };
                    e.on_hover_change(|s: &mut State, inside, tx: &mut Tx| {
                        let closed = inside && s.ui.ctx.as_mut().is_some_and(|m| m.open_sub.take().is_some());
                        if closed {
                            tx.instant();
                        }
                    })
                }
            }
        });
    }
    let mut layer = el().key("ctx-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(
            el().key("ctx-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .on_press(|s: &mut State| s.ui.ctx = None)
                .on_secondary_press(|s: &mut State, _p, _tx: &mut Tx| s.ui.ctx = None),
        )
        .child(panel);
    if let Some((i, off)) = sub_at
        && let Some(link) = m.items[i].sub.clone()
    {
        let at = m.at;
        layer = layer.child(menu::share_panel(x, y + off, win, Rc::new(move |s: &mut State, via| {
            s.ui.ctx = None;
            s.share_link(&link, via, at);
        })));
    }
    Some(layer)
}

#[cfg(test)]
mod local_tests {
    use super::*;
    use crate::local::view::{self, tests::track};

    fn labels(items: &[CtxItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn local_albums_artists_and_folders_offer_only_what_works_for_files() {
        crate::share::set_submenu(Some(true));
        let mut s = State::offline();
        s.settings.music_folders = vec!["/m".into()];
        s.session.authorized = true; // even signed in: these are files
        let (songs, v) = view::from_index(&[track("/m/A/1.flac", "One", "Ann", "Debut", "", None, Some(1))], &s.settings.music_folders);
        s.set_local_index(songs, v);
        let album = s.local_lib.view.albums[0].clone();
        assert_eq!(labels(&collection_items(&s, &album)), ["Play", "Shuffle", "Play Next", "Add to Queue", "", "Go to Artist"]);
        let artist = s.local_lib.view.artists[0].clone();
        assert_eq!(labels(&artist_items(&s, &artist)), ["Play", "Shuffle", "Play Next", "Add to Queue"]);
        let folder = local_items(LocalWhat::Folder("/m/A".into()), "A".into());
        assert_eq!(labels(&folder), ["Play", "Shuffle", "Play Next", "Add to Queue", "", "Show in Folder"]);
        // Apple items keep their menus.
        let apple = Artist { id: "r.1".into(), name: "Ann".into() };
        assert_eq!(labels(&artist_items(&s, &apple)), ["Pin to Sidebar", "Share"], "a library artist is looked up in the catalog");
    }

    #[test]
    fn a_pin_has_the_menu_of_what_it_pins_plus_its_own_items() {
        crate::share::set_submenu(Some(true));
        use crate::settings::{Pin, PinKind};
        let mut s = State::offline();
        s.session.authorized = true;
        let mut mine = Collection::parse(&serde_json::json!({"id": "p.1", "type": "library-playlists",
            "attributes": {"name": "Mine", "canEdit": true, "canDelete": true}})).unwrap();
        mine.tracks.clear();
        s.library_playlists = crate::state::Paged::ready(vec![mine.clone()]);
        let pin = |kind, id: &str| Pin { kind, id: id.into(), title: format!("Pin {id}"), artwork: None };
        s.settings.pins = vec![pin(PinKind::Playlist, "p.1"), pin(PinKind::Playlist, "pl.apple"), pin(PinKind::Station, "ra.1"), pin(PinKind::Artist, "r.1")];
        // The playlist row's menu, without its Pin item; then the pin's own.
        let row: Vec<String> = collection_items(&s, &mine).iter().map(|i| i.label.clone()).filter(|l| l != "Unpin from Sidebar").collect();
        let mut want = row.clone();
        want.extend(["".to_string(), "Unpin".into(), "Move Down".into()]);
        assert_eq!(labels(&pin_items(&s, "p.1")), want);
        assert_eq!(labels(&pin_items(&s, "pl.apple")), ["Play", "Shuffle", "", "Favourite", "Add to Library", "Share", "", "Unpin", "Move Up", "Move Down"]);
        assert_eq!(labels(&pin_items(&s, "ra.1")), ["Play", "Share", "", "Unpin", "Move Up", "Move Down"]);
        assert_eq!(labels(&pin_items(&s, "r.1")), ["Unpin", "Move Up"]);
        // Its Play plays the list (loaded first, as from the row).
        let play = pin_items(&s, "p.1").into_iter().next().unwrap();
        (play.act.unwrap())(&mut s);
        assert_eq!(s.sent.borrow().iter().filter(|p| p.contains("/v1/me/library/playlists/p.1")).count(), 1);
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::share::{Kind, Link, Via};
    use rune::{Env, Runtime};

    /// The Mac's style (a Share submenu) with a sample list of services, on any system.
    fn mac_style() {
        crate::share::set_submenu(Some(true));
        crate::share::set_services(Some(crate::share::sample_services()));
        crate::share::take_performed();
    }

    fn scene(name: &str) -> State {
        mac_style();
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == name).unwrap().1)(&mut s);
        s
    }

    fn run(s: State) -> Runtime<State> {
        let mut rt = Runtime::new(s, crate::ui::view, (1180.0, 760.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    fn tap(rt: &mut Runtime<State>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
        rt.pointer_move(x + 8.0, y + 8.0);
        rt.pointer_down(x + 8.0, y + 8.0);
        rt.pointer_up(x + 8.0, y + 8.0);
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn labels(s: &State) -> Vec<String> {
        s.ui.ctx.as_ref().map(|m| m.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default()
    }

    #[test]
    fn share_then_copy_link_puts_the_page_on_the_clipboard_and_toasts() {
        let mut s = scene("recently-added");
        // A catalog album's card.
        let c = crate::models::Collection { id: "617154241".into(), title: "Random Access Memories".into(), ..sample(CollectionKind::Album) };
        s.session.storefront = Some("it".into());
        open_collection(&mut s, &c, [520.0, 200.0]);
        assert!(labels(&s).contains(&"Share".to_string()));
        let board = rune::clipboard::memory();
        let mut rt = run(s);
        rt.use_clipboard(&board);
        rt.poll = Some(crate::ui::poll);
        let share = rt.state.ui.ctx.as_ref().unwrap().items.iter().position(|i| i.sub.is_some()).unwrap();
        assert!(rt.find("share-copy-link").is_none(), "the submenu is closed until Share is reached");
        tap(&mut rt, &format!("ctx-{share}"));
        tap(&mut rt, "share-copy-link");
        let s = rt.state;
        assert!(s.ui.ctx.is_none(), "the menu closes");
        assert_eq!(s.toast.as_deref(), Some("Link copied"));
        // `ui::poll` hands the link to Rune's clipboard (the system one in the app).
        assert_eq!(s.pending_share, None);
        assert_eq!(board.text().as_deref().map(String::as_str), Some("https://music.apple.com/it/album/random-access-memories/617154241"));
    }

    #[test]
    fn copy_link_asks_for_the_link_before_toasting_on_a_library_album() {
        mac_style();
        let mut s = State::offline();
        s.copy_link(&Link::Lookup { kind: Kind::Album, id: "l.zzz".into(), title: "Nevermind".into() });
        assert_eq!(s.sent.borrow().len(), 1);
        assert!(s.sent.borrow()[0].starts_with("/v1/me/library/albums/l.zzz/catalog "));
        assert_eq!(s.pending_share, None, "nothing is copied before Apple answers");
        s.apply_msg(crate::state::Msg::ShareLink {
            kind: Kind::Album,
            id: "l.zzz".into(),
            title: "Nevermind".into(),
            via: Via::Copy,
            at: [0.0, 0.0],
            result: Ok(Some(("1440783617".into(), None))),
        });
        assert_eq!(s.pending_share.as_ref().map(|p| p.url.as_str()), Some("https://music.apple.com/us/album/nevermind/1440783617"));
        assert_eq!(s.toast.as_deref(), Some("Link copied"));
        // No catalog match: a toast, nothing copied.
        let mut s = State::offline();
        s.apply_msg(crate::state::Msg::ShareLink {
            kind: Kind::Artist,
            id: "r.1".into(),
            title: "X".into(),
            via: Via::Copy,
            at: [0.0, 0.0],
            result: Ok(None),
        });
        assert_eq!(s.pending_share, None);
        assert_eq!(s.toast.as_deref(), Some("Apple Music has no page to link to."));
    }

    #[test]
    fn each_kind_of_menu_has_share_where_a_page_exists() {
        mac_style();
        let mut s = State::offline();
        s.session.storefront = Some("it".into());
        let album = crate::models::Collection { id: "617154241".into(), ..sample(CollectionKind::Album) };
        assert!(labels_of(collection_items(&s, &album)).contains(&"Share".to_string()));
        let local = crate::models::Collection { id: format!("{}abc", crate::local::view::ALBUM_PREFIX), ..sample(CollectionKind::Album) };
        assert!(!labels_of(collection_items(&s, &local)).contains(&"Share".to_string()), "local albums: no page");
        let own = crate::models::Collection { id: "p.own1".into(), editable: true, ..sample(CollectionKind::Playlist) };
        assert!(!labels_of(collection_items(&s, &own)).contains(&"Share".to_string()), "a library playlist with no global id");
        let apple = crate::models::Collection { id: "pl.abc".into(), ..sample(CollectionKind::Playlist) };
        assert!(labels_of(collection_items(&s, &apple)).contains(&"Share".to_string()));
        let artist = Artist { id: "5468295".into(), name: "Daft Punk".into() };
        assert!(labels_of(artist_items(&s, &artist)).contains(&"Share".to_string()));
        let local_artist = Artist { id: crate::local::view::artist_id("x"), name: "x".into() };
        assert!(!labels_of(artist_items(&s, &local_artist)).contains(&"Share".to_string()));
    }

    fn labels_of(v: Vec<CtxItem>) -> Vec<String> {
        v.into_iter().map(|i| i.label).collect()
    }

    fn sample(kind: CollectionKind) -> crate::models::Collection {
        crate::models::Collection {
            kind,
            id: String::new(),
            title: "T".into(),
            subtitle: String::new(),
            artwork: None,
            year: None,
            genre: None,
            description: None,
            track_count: None,
            modified: None,
            editable: false,
            favorites: false,
            public: None,
            tracks: Vec::new(),
            artists: Vec::new(),
        }
    }

    #[test]
    fn stations_apple_made_have_share_the_personal_one_does_not() {
        mac_style();
        let mut s = State::offline();
        let apple = Station { id: "ra.978194965".into(), name: "Apple Music 1".into(), artwork: None, live: true };
        open_station(&mut s, &apple, [10.0, 10.0]);
        assert!(labels(&s).contains(&"Share".to_string()));
        let mine = Station { id: "ra.u-1234".into(), name: "My Station".into(), artwork: None, live: false };
        open_station(&mut s, &mine, [10.0, 10.0]);
        assert!(!labels(&s).contains(&"Share".to_string()));
    }

    #[test]
    fn the_song_menu_copies_the_album_page_with_the_song_id() {
        let mut s = scene("share-song-player-bar");
        s.ui.menu.as_mut().unwrap().share = false;
        let board = rune::clipboard::memory();
        let mut rt = run(s);
        rt.use_clipboard(&board);
        rt.poll = Some(crate::ui::poll);
        tap(&mut rt, "menu-share");
        tap(&mut rt, "share-copy-link");
        assert!(rt.state.ui.menu.is_none());
        assert_eq!(rt.state.toast.as_deref(), Some("Link copied"));
        assert_eq!(
            board.text().as_deref().map(String::as_str),
            Some("https://music.apple.com/it/album/night-bus-hymns/1600000002?i=1600000010")
        );
    }

    #[test]
    fn a_library_song_without_a_catalog_twin_and_a_local_file_have_no_share() {
        let mut s = scene("share-song-player-bar");
        let mut song = s.ui.menu.as_ref().unwrap().songs[0].clone();
        song.id = "i.abc".into();
        song.catalog_id = None;
        crate::ui::menu::open_song(&mut s, song.clone(), crate::ui::menu::MenuContext::NowPlaying, [80.0, 380.0]);
        s.ui.menu.as_mut().unwrap().share = true;
        let rt = run(s);
        assert!(rt.find("menu-share").is_none());
        let mut s = State::offline();
        song.id = "file:///m/a.flac".into();
        crate::ui::menu::open_song(&mut s, song, crate::ui::menu::MenuContext::NowPlaying, [80.0, 380.0]);
        assert!(run(s).find("menu-share").is_none());
    }

    #[test]
    fn a_service_in_the_share_submenu_gets_the_resolved_link_and_nothing_is_copied() {
        let mut s = scene("recently-added");
        let c = crate::models::Collection { id: "617154241".into(), title: "Random Access Memories".into(), ..sample(CollectionKind::Album) };
        s.session.storefront = Some("it".into());
        open_collection(&mut s, &c, [520.0, 200.0]);
        let board = rune::clipboard::memory();
        let mut rt = run(s);
        rt.use_clipboard(&board);
        rt.poll = Some(crate::ui::poll);
        let share = rt.state.ui.ctx.as_ref().unwrap().items.iter().position(|i| i.sub.is_some()).unwrap();
        tap(&mut rt, &format!("ctx-{share}"));
        // Copy Link, then the services in the system's order, with their titles.
        for (i, title) in ["AirDrop", "Mail", "Messages"].iter().enumerate() {
            assert!(rt.find(&format!("share-service-{i}")).is_some(), "{title} is listed");
        }
        tap(&mut rt, "share-service-2");
        assert!(rt.state.ui.ctx.is_none(), "the menu closes");
        assert_eq!(
            crate::share::take_performed(),
            [(2, "https://music.apple.com/it/album/random-access-memories/617154241".to_string())],
            "Messages gets the album's page"
        );
        assert_eq!(rt.state.toast, None, "no Link copied toast: the service's own window shows");
        assert_eq!(board.text(), None, "the clipboard is left alone");
    }

    #[test]
    fn a_library_album_is_looked_up_before_the_service_gets_its_link() {
        mac_style();
        let mut s = State::offline();
        s.share_link(&Link::Lookup { kind: Kind::Album, id: "l.zzz".into(), title: "Nevermind".into() }, Via::Service(0), [300.0, 200.0]);
        assert!(s.sent.borrow()[0].starts_with("/v1/me/library/albums/l.zzz/catalog "));
        assert_eq!(s.pending_share, None, "nothing goes to the service before Apple answers");
        s.apply_msg(crate::state::Msg::ShareLink {
            kind: Kind::Album,
            id: "l.zzz".into(),
            title: "Nevermind".into(),
            via: Via::Service(0),
            at: [300.0, 200.0],
            result: Ok(Some(("1440783617".into(), Some("https://music.apple.com/it/album/nevermind/1440783617".into())))),
        });
        let p = s.pending_share.clone().unwrap();
        assert_eq!((p.via, p.url.as_str(), p.at), (Via::Service(0), "https://music.apple.com/it/album/nevermind/1440783617", [300.0, 200.0]));
        assert_eq!(s.toast, None);
        // `ui::poll` hands it over (recorded in tests, never performed).
        let mut rt = run(s);
        rt.poll = Some(crate::ui::poll);
        rt.frame(1.0 / 60.0);
        assert_eq!(crate::share::take_performed(), [(0, "https://music.apple.com/it/album/nevermind/1440783617".to_string())]);
    }

    #[test]
    fn the_song_menus_share_submenu_hands_the_song_link_to_a_service() {
        let mut s = scene("share-song-player-bar");
        s.ui.menu.as_mut().unwrap().share = false;
        let mut rt = run(s);
        rt.poll = Some(crate::ui::poll);
        tap(&mut rt, "menu-share");
        tap(&mut rt, "share-service-0");
        assert!(rt.state.ui.menu.is_none());
        assert_eq!(crate::share::take_performed(), [(0, "https://music.apple.com/it/album/night-bus-hymns/1600000002?i=1600000010".to_string())]);
    }

    #[test]
    fn without_a_system_share_menu_copy_link_sits_where_share_was() {
        crate::share::set_submenu(Some(false));
        let mut s = State::offline();
        s.session.storefront = Some("it".into());
        let album = crate::models::Collection { id: "617154241".into(), ..sample(CollectionKind::Album) };
        let items = labels_of(collection_items(&s, &album));
        assert!(items.contains(&"Copy Link".to_string()) && !items.contains(&"Share".to_string()), "{items:?}");
        let station = Station { id: "ra.978194965".into(), name: "Apple Music 1".into(), artwork: None, live: true };
        open_station(&mut s, &station, [10.0, 10.0]);
        assert_eq!(labels(&s).last().map(String::as_str), Some("Copy Link"));
        // Still hidden where there is no page.
        let local = crate::models::Collection { id: format!("{}abc", crate::local::view::ALBUM_PREFIX), ..sample(CollectionKind::Album) };
        assert!(!labels_of(collection_items(&s, &local)).contains(&"Copy Link".to_string()));
        let favs = crate::models::Collection { id: "p.favs".into(), editable: false, favorites: true, ..sample(CollectionKind::Playlist) };
        assert!(!labels_of(collection_items(&s, &favs)).iter().any(|l| l == "Copy Link" || l == "Share"), "Favourite Songs");
        // The song menu: a flat Copy Link that copies.
        let mut s = scene("share-song-player-bar");
        crate::share::set_submenu(Some(false));
        s.ui.menu.as_mut().unwrap().share = false;
        let board = rune::clipboard::memory();
        let mut rt = run(s);
        rt.use_clipboard(&board);
        rt.poll = Some(crate::ui::poll);
        assert!(rt.find("menu-share").is_none());
        tap(&mut rt, "menu-copy-link");
        assert_eq!(board.text().as_deref().map(String::as_str), Some("https://music.apple.com/it/album/night-bus-hymns/1600000002?i=1600000010"));
        assert!(crate::share::take_performed().is_empty());
    }

    #[test]
    fn the_platform_decides_the_style() {
        crate::share::set_submenu(None);
        #[cfg(target_os = "macos")]
        assert!(crate::share::submenu(), "macOS: a Share submenu with the system's services");
        #[cfg(not(target_os = "macos"))]
        assert!(!crate::share::submenu(), "elsewhere: a flat Copy Link");
    }
}
