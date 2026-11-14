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
}

pub struct CtxMenu {
    at: [f32; 2],
    items: Vec<CtxItem>,
    /// The destructive item clicked once.
    armed: Option<usize>,
}

fn it(label: impl Into<String>, f: impl Fn(&mut State) + 'static) -> CtxItem {
    CtxItem { label: label.into(), act: Some(Rc::new(f)), confirm: None }
}

/// An item that asks first: "Delete Playlist" → "Click again to delete".
fn danger(label: impl Into<String>, confirm: impl Into<String>, f: impl Fn(&mut State) + 'static) -> CtxItem {
    CtxItem { label: label.into(), act: Some(Rc::new(f)), confirm: Some(confirm.into()) }
}

fn sep() -> CtxItem {
    CtxItem { label: String::new(), act: None, confirm: None }
}

pub fn open(s: &mut State, at: [f32; 2], items: Vec<CtxItem>) {
    s.ui.menu = None;
    s.ui.ctx = Some(CtxMenu { at, items, armed: None });
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
    } else if c.id.starts_with("l.") || (c.id.starts_with("p.") && c.editable) {
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
    let mut v = Vec::new();
    // Apple's "Create Station" on an artist: its catalog id (a library artist's page knows it).
    let catalog = if crate::models::Artist::is_library_id(&a.id) {
        s.artist_pages.get(&a.id).and_then(|l| l.ready()).and_then(|p| p.as_ref()).map(|p| p.id.clone())
    } else {
        Some(a.id.clone())
    };
    if let Some(id) = catalog.filter(|_| !s.local_only()) {
        let name = a.name.clone();
        v.push(it("Create Station", move |s| s.create_artist_station(&id, &name)));
    }
    v.push(pin_item(s, &a.id, move |s| s.pin_artist(&a2)));
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
    let items = vec![it("Play", move |s| s.play_station(&s1)), sep(), pin_item(s, &st.id, move |s| s.pin_station(&s2))];
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
            vec![it("Play", move |s| s.play_station(&st))]
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
    for (i, item) in m.items.iter().enumerate() {
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
                if armed { e.fill(PAL.danger_bg) } else { e }
            }
        });
    }
    Some(
        el().key("ctx-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .child(
                el().key("ctx-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .on_press(|s: &mut State| s.ui.ctx = None)
                    .on_secondary_press(|s: &mut State, _p, _tx: &mut Tx| s.ui.ctx = None),
            )
            .child(panel),
    )
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
        assert_eq!(labels(&artist_items(&s, &apple)), ["Pin to Sidebar"]);
    }

    #[test]
    fn a_pin_has_the_menu_of_what_it_pins_plus_its_own_items() {
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
        assert_eq!(labels(&pin_items(&s, "pl.apple")), ["Play", "Shuffle", "", "Favourite", "Add to Library", "", "Unpin", "Move Up", "Move Down"]);
        assert_eq!(labels(&pin_items(&s, "ra.1")), ["Play", "", "Unpin", "Move Up", "Move Down"]);
        assert_eq!(labels(&pin_items(&s, "r.1")), ["Unpin", "Move Up"]);
        // Its Play plays the list (loaded first, as from the row).
        let play = pin_items(&s, "p.1").into_iter().next().unwrap();
        (play.act.unwrap())(&mut s);
        assert_eq!(s.sent.borrow().iter().filter(|p| p.contains("/v1/me/library/playlists/p.1")).count(), 1);
    }
}
