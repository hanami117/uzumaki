//! Generic context menus (album / playlist cards and headers, pins, artists, stations): a list
//! of labelled actions in an in-app overlay. The song menu lives in `menu.rs`.

use std::rc::Rc;

use rune::{Drag, El, Phase, Tx, el, icon};

use super::{icons, menu};
use crate::models::{Artist, Collection, Song, Station};
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
    let c3 = c.clone();
    v.push(pin_item(s, &c.id, move |s| s.pin_collection(&c3)));
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
    vec![pin_item(s, &a.id, move |s| s.pin_artist(&a2))]
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

/// Right-click on a pin: Unpin, Move Up, Move Down.
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

pub fn open_pin(s: &mut State, id: &str, at: [f32; 2]) {
    let pins = &s.settings.pins;
    let Some(i) = pins.iter().position(|p| p.id == id) else { return };
    let last = pins.len() - 1;
    let mut items = Vec::new();
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
    open(s, at, items);
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
}
