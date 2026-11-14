//! The song context menu. Rune's native pop-up menus only exist on macOS and Windows, so this is
//! an in-app overlay: a click-away scrim, the menu panel at the pointer, and the "Add to
//! Playlist" submenu as a second panel beside it.

use rune::{Cursor, El, ScrollSpec, TextStyle, col, el, icon, row, text};

use super::{icons, select};
use crate::models::{Collection, Song};
use crate::state::State;
use crate::theme::*;

pub(super) const W: f32 = 224.0;
pub(super) const ITEM_H: f32 = 32.0;

/// Where the menu was opened, which decides its first items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuContext {
    /// A page's song list (Play, Play Next, Add to Queue; Remove from Playlist).
    List,
    /// The playing song, in the player bar.
    NowPlaying,
    /// Row `i` of Up Next (Play, Remove from Up Next).
    UpNext(usize),
}

pub struct MenuState {
    pub at: [f32; 2],
    pub list: String,
    /// Key of the row the menu was opened on (`lists::row_key`; the song id outside lists).
    pub id: String,
    /// The songs it acts on (the selection when it opened), each song once.
    pub songs: Vec<Song>,
    /// The selected rows, one per row with its position in the list's own order: a song
    /// selected in two rows is here twice. Only Remove from Playlist uses it, since it acts
    /// on rows rather than songs. Empty outside the page's lists.
    pub rows: Vec<(usize, Song)>,
    /// "Add to Playlist" submenu showing.
    pub sub: bool,
    /// "Share" submenu (Copy Link) showing.
    pub share: bool,
    pub context: MenuContext,
    /// "Remove from Library" was clicked once: the next click removes.
    pub confirm: bool,
}

/// The menu of row `key` of the page's list `list`: it acts on the selection (which holds the
/// row), else on that row alone. Never on other rows of the same song.
pub fn open(s: &mut State, list: &str, key: &str, at: [f32; 2]) {
    let mut rows = select::selected_rows(s);
    if rows.is_empty() {
        rows = super::lists::visible_rows_at(s).into_iter().filter(|(_, k, _)| k == key).map(|(i, _, x)| (i, x)).collect();
    }
    let songs = select::unique(rows.iter().map(|(_, x)| x.clone()).collect());
    s.ui.ctx = None;
    crate::share::prepare();
    s.ui.menu = Some(MenuState { at, list: list.into(), id: key.into(), songs, rows, sub: false, share: false, context: MenuContext::List, confirm: false });
}

/// The menu for one song outside the page's lists: the player bar's, or an Up Next row's.
pub fn open_song(s: &mut State, song: Song, context: MenuContext, at: [f32; 2]) {
    s.ui.ctx = None;
    crate::share::prepare();
    let id = song.id.clone();
    s.ui.menu = Some(MenuState { at, list: String::new(), id, rows: Vec::new(), songs: vec![song], sub: false, share: false, context, confirm: false });
}

/// The user's playlist whose page the menu's list `list` is, when songs can be removed from it.
pub(super) fn editable_playlist_here(s: &State, list: &str) -> Option<Collection> {
    let crate::state::Route::Collection { kind: crate::models::CollectionKind::Playlist, id } = &s.route else { return None };
    if super::lists::list_id(s).as_deref() != Some(list) || !id.starts_with("p.") {
        return None;
    }
    s.collections.get(id).and_then(|l| l.ready()).filter(|c| c.editable).cloned()
}

/// The user's own playlists, which Add to Playlist can edit.
fn playlists(s: &State) -> Vec<&Collection> {
    s.library_playlists.items.iter().filter(|c| c.editable).collect()
}

pub(super) fn item(key: String, label: &str, chevron: bool) -> El {
    item_led(key, label, chevron, None)
}

/// `item` with `lead` (a 16 pt icon) before the label.
fn item_led(key: String, label: &str, chevron: bool, lead: Option<El>) -> El {
    let mut r = row().key(key).role("menuitem").label(label.to_string()).focusable(true).w_full().h(ITEM_H).px(12.0).gap(8.0)
        .center_y().radius(6.0).hover(crate::theme::ov(0.09)).press(crate::theme::ov(0.14))
        .cursor(Cursor::Pointer);
    if let Some(lead) = lead {
        r = r.child(lead);
    }
    r = r.child(text(label.to_string()).font(TextStyle::new(13.5, 400.0)).color(PAL.text).truncate().grow(1.0).min_w(0.0));
    if chevron {
        r = r.child(icon(&icons::CHEVRON_RIGHT, 14.0).color(PAL.muted));
    }
    r
}

/// What a Share submenu item does with its choice (the clipboard or a service).
pub(super) type ShareAct = std::rc::Rc<dyn Fn(&mut State, crate::share::Via)>;

/// The Share submenu (macOS), as Apple Music's: Copy Link, then the system's sharing services
/// with their own titles and icons (`share::services`, read when the menu opened). `x` is the
/// main panel's left, `item_y` the Share item's top, both in window points. `act` closes the
/// menu and sends the link on.
pub(super) fn share_panel(x: f32, item_y: f32, win: [f32; 2], act: ShareAct) -> El {
    use crate::share::Via;
    let services = crate::share::services();
    let seps = if services.is_empty() { 0.0 } else { 9.0 };
    let h = 12.0 + (1 + services.len()) as f32 * ITEM_H + seps;
    let sx = if x + 2.0 * W + 12.0 > win[0] { x - W - 4.0 } else { x + W + 4.0 };
    let sy = (item_y - 6.0).min(win[1] - h - 8.0).max(8.0);
    let a = act.clone();
    let link = icon(&icons::LINK, 15.0).color(PAL.text2);
    let mut p = panel(sx, sy).key("menu-share-panel").child(
        item_led("share-copy-link".into(), "Copy Link", false, Some(el().size(16.0, 16.0).center().child(link)))
            .on_press(move |s: &mut State| a(s, Via::Copy)),
    );
    if !services.is_empty() {
        p = p.child(sep());
    }
    for (i, svc) in services.iter().enumerate() {
        let a = act.clone();
        let lead = match &svc.icon {
            Some(px) => el().size(16.0, 16.0).pixels_fit(px, rune::SurfaceFit::Fill),
            // Sample lists (tests, snaps off the Mac) have no icons: a plain tile keeps the column.
            None => el().size(16.0, 16.0).radius(4.0).fill(PAL.chip_hover),
        };
        p = p.child(item_led(format!("share-service-{i}"), &svc.title, false, Some(lead)).on_press(move |s: &mut State| a(s, Via::Service(i))));
    }
    p
}

pub(super) fn panel(x: f32, y: f32) -> El {
    panel_box().absolute(x, y)
}

/// A menu panel, not yet placed.
fn panel_box() -> El {
    col().w(W).padding(6.0).radius(10.0).fill(crate::theme::pop_fill()).border(1.0, crate::theme::pop_border())
        .shadow(8.0, 28.0, crate::theme::shadow(0.5))
}

/// Top of a menu `h` tall opened at pointer height `at` in a window `win_h` tall: below the
/// pointer, else above it (its bottom at the pointer, as on the Mac), else as low as fits.
pub(super) fn place_y(at: f32, h: f32, win_h: f32) -> f32 {
    if at + h + 8.0 <= win_h {
        at
    } else if at - h >= 8.0 {
        at - h
    } else {
        (win_h - h - 8.0).max(8.0)
    }
}

pub(super) fn sep() -> El {
    el().w_full().h(1.0).fill(PAL.chip_hover).my(4.0)
}

/// Runs `f` on the songs of the open menu and closes it.
fn act(s: &mut State, f: impl FnOnce(&mut State, &MenuState)) {
    if let Some(m) = s.ui.menu.take() {
        f(s, &m);
    }
}

/// The menu panel, built item by item; it keeps count so the submenu can line up with
/// "Add to Playlist" wherever that lands.
struct Panel {
    el: El,
    items: usize,
    seps: usize,
    pending_sep: bool,
}

impl Panel {
    fn add(&mut self, e: El) {
        let mut el = std::mem::replace(&mut self.el, col());
        if self.pending_sep && self.items > 0 {
            el = el.child(sep());
            self.seps += 1;
        }
        self.pending_sep = false;
        self.el = el.child(e);
        self.items += 1;
    }

    /// A separator before the next item (none at the top, none doubled).
    fn sep(&mut self) {
        self.pending_sep = true;
    }

    /// Distance from the panel's top to the next item.
    fn next_y(&self) -> f32 {
        let seps = self.seps + usize::from(self.pending_sep && self.items > 0);
        6.0 + self.items as f32 * ITEM_H + seps as f32 * 9.0
    }
}

pub fn overlay(s: &State, win: [f32; 2]) -> Option<El> {
    let m = s.ui.menu.as_ref()?;
    let lists = playlists(s);
    let x = m.at[0].min(win[0] - W - 8.0).max(8.0);

    let closes = |e: El| {
        e.on_hover_change(|s: &mut State, inside, tx: &mut rune::Tx| {
            if let (true, Some(m)) = (inside, &mut s.ui.menu) {
                m.sub = false;
                m.share = false;
                tx.instant();
            }
        })
    };
    let local = m.songs.iter().any(|x| x.is_local());
    let one = (m.songs.len() == 1).then(|| m.songs[0].clone());
    let mut p = Panel { el: panel_box(), items: 0, seps: 0, pending_sep: false };

    // Playing.
    match m.context {
        MenuContext::List => {
            p.add(closes(item("menu-play".into(), "Play", false).on_press(|s: &mut State| {
                act(s, |s, m| {
                    if m.songs.len() > 1 {
                        s.play_songs(&m.songs, 0);
                    } else {
                        select::play_row(s, &m.list, &m.id);
                    }
                })
            })));
            p.add(closes(item("menu-next".into(), "Play Next", false).on_press(|s: &mut State| act(s, |s, m| s.play_next(&m.songs)))));
            p.add(closes(item("menu-queue".into(), "Add to Queue", false).on_press(|s: &mut State| act(s, |s, m| s.add_to_queue(&m.songs)))));
        }
        MenuContext::UpNext(i) => {
            p.add(closes(item("menu-play".into(), "Play", false).on_press(move |s: &mut State| act(s, |s, _| s.play_up_next(i)))));
            p.add(closes(item("menu-unqueue".into(), "Remove from Up Next", false).on_press(move |s: &mut State| act(s, |s, _| s.remove_up_next(i)))));
        }
        MenuContext::NowPlaying => {}
    }
    // Apple's "Create Station": a station seeded by this song (one Apple catalog song).
    if let Some(song) = one.as_ref().filter(|x| !s.local_only() && x.catalog_ref().is_some()) {
        let song = song.clone();
        p.add(closes(item("menu-station".into(), "Create Station", false).on_press(move |s: &mut State| {
            s.ui.menu = None;
            s.create_song_station(&song);
        })));
    }
    p.sep();

    // The library.
    // Apple mode: a file's album is Local Files itself. Local only mode has album pages.
    let at_local_page = local && s.route == crate::state::Route::LocalFiles && !s.local_only();
    if one.is_some() && !at_local_page {
        p.add(closes(item("menu-album".into(), "Go to Album", false).on_press(|s: &mut State| {
            // From an Up Next row in full-screen Now Playing: the page opens behind it.
            super::now_playing::close(s);
            act(s, |s, m| {
                if let Some(song) = m.songs.first() {
                    s.go_to_album(song);
                }
            })
        })));
    }
    if local && s.local_only() && one.as_ref().is_some_and(|x| x.is_local()) {
        p.add(closes(item("menu-artist".into(), "Go to Artist", false).on_press(|s: &mut State| {
            super::now_playing::close(s);
            act(s, |s, m| {
                if let Some(song) = m.songs.first() {
                    s.go_to_local_artist(song);
                }
            })
        })));
    }
    let mut sub_y = None;
    if !local {
        sub_y = Some(p.next_y());
        let add = item("menu-playlist".into(), "Add to Playlist", true).on_press(|s: &mut State| {
            if let Some(m) = &mut s.ui.menu {
                m.sub = !m.sub;
            }
        });
        let add = if m.sub { add.fill(crate::theme::ov(0.09)) } else { add };
        // Like the Mac: the submenu opens when the pointer rests on its item, and closes when it
        // moves to another one.
        p.add(add.on_hover_change(|s: &mut State, inside, tx: &mut rune::Tx| {
            if let Some(m) = &mut s.ui.menu {
                m.sub = m.sub || inside;
                m.share = m.share && !inside;
                tx.instant();
            }
        }));
    }
    let catalog = m.songs.iter().any(|x| !x.is_local() && !crate::state::in_library(&x.id));
    if catalog {
        p.add(closes(item("menu-library".into(), "Add to Library", false).on_press(|s: &mut State| act(s, |s, m| s.add_songs_to_library(&m.songs)))));
    }
    let starrable: Vec<&Song> = m.songs.iter().filter(|x| s.can_favourite(x)).collect();
    if !starrable.is_empty() {
        let label = if starrable.iter().all(|x| s.is_favourite(x)) { "Undo Favourite" } else { "Favourite" };
        p.add(closes(item("menu-favourite".into(), label, false).on_press(|s: &mut State| act(s, |s, m| s.toggle_favourite_songs(&m.songs)))));
    }
    if m.context == MenuContext::List && editable_playlist_here(s, &m.list).is_some() {
        p.add(closes(item("menu-remove".into(), "Remove from Playlist", false).on_press(|s: &mut State| {
            act(s, |s, m| {
                if let Some(c) = editable_playlist_here(s, &m.list) {
                    s.remove_from_playlist(&c, &m.rows);
                }
            })
        })));
    }
    if m.songs.iter().any(|x| x.id.starts_with("i.")) {
        // Two clicks: the first turns the item into its own confirmation.
        let label = if m.confirm { "Click again to remove from library" } else { "Remove from Library" };
        let mut it = item("menu-unlibrary".into(), label, false).on_press(|s: &mut State| {
            let confirmed = s.ui.menu.as_ref().is_some_and(|m| m.confirm);
            if confirmed {
                act(s, |s, m| s.remove_songs_from_library(&m.songs));
            } else if let Some(m) = &mut s.ui.menu {
                m.confirm = true;
                m.sub = false;
            }
        });
        if m.confirm {
            it = it.fill(PAL.danger_bg);
        }
        p.add(closes(it));
    }
    p.sep();

    // Elsewhere.
    let mut share: Option<(f32, String)> = None;
    if let Some(song) = &one {
        if let Some(path) = song.local_path() {
            let path = path.to_path_buf();
            p.add(closes(item("menu-folder".into(), "Show in Folder", false).on_press(move |s: &mut State| {
                s.ui.menu = None;
                crate::desktop::show_in_folder(&path);
            })));
        } else if let crate::share::Link::Url(url) = crate::share::song_link(song, s.session.storefront.as_deref()) {
            let u2 = url.clone();
            p.add(closes(item("menu-web".into(), "Show in Apple Music", false).on_press_tx(move |s: &mut State, tx: &mut rune::Tx| {
                s.ui.menu = None;
                if tx.open_url(&url).is_err() {
                    s.toast = Some("Couldn't open the link.".into());
                }
            })));
            if crate::share::submenu() {
                // Share ▸ Copy Link and the system's services, as in Apple Music; the submenu
                // opens like Add to Playlist's.
                share = Some((p.next_y(), u2));
                let shown = m.share;
                let e = item("menu-share".into(), "Share", true).on_press(|s: &mut State| {
                    if let Some(m) = &mut s.ui.menu {
                        m.share = true;
                        m.sub = false;
                    }
                });
                let e = if shown { e.fill(crate::theme::ov(0.09)) } else { e };
                p.add(e.on_hover_change(|s: &mut State, inside, tx: &mut rune::Tx| {
                    if let (true, Some(m)) = (inside, &mut s.ui.menu) {
                        m.share = true;
                        m.sub = false;
                        tx.instant();
                    }
                }));
            } else {
                // No system share menu here (Linux): Copy Link itself, where Share would be.
                p.add(closes(item("menu-copy-link".into(), "Copy Link", false).on_press(move |s: &mut State| {
                    s.ui.menu = None;
                    s.copy_link(&crate::share::Link::Url(u2.clone()));
                })));
            }
        }
    }
    // Placed once its height is known (the Now Playing menu is much shorter than a list's).
    let h = 12.0 + p.items as f32 * ITEM_H + p.seps as f32 * 9.0;
    let y = place_y(m.at[1], h, win[1]);
    let main = p.el.absolute(x, y);

    let mut layer = el().key("menu-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(
            el().key("menu-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .on_press(|s: &mut State| s.ui.menu = None)
                .on_secondary_press(|s: &mut State, _p, _tx: &mut rune::Tx| s.ui.menu = None),
        )
        .child(main);

    if let (true, Some(sub_y)) = (m.sub, sub_y) {
        // Beside the main panel, or on its left near the right edge; top aligned with its item.
        let sx = if x + 2.0 * W + 12.0 > win[0] { x - W - 4.0 } else { x + W + 4.0 };
        let sy = y + sub_y;
        let sy = sy.min(win[1] - 120.0).max(8.0);
        let mut list = col().w_full().gap(0.0).child(
            item("menu-new-playlist".into(), "New Playlist\u{2026}", false)
                .on_press(|s: &mut State| act(s, |s, m| super::new_playlist::open(s, m.songs.clone()))),
        ).child(sep());
        if lists.is_empty() {
            list = list.child(text("No playlists in your library").font(SMALL13).color(PAL.dim).px(12.0).py(8.0));
        }
        for c in lists {
            let id = c.id.clone();
            list = list.child(item(format!("menu-pl-{}", c.id), &c.title, false).on_press(move |s: &mut State| {
                act(s, |s, m| {
                    let target = s.library_playlists.items.iter().find(|c| c.id == id).cloned();
                    if let Some(c) = target {
                        s.add_to_playlist(&c, &m.songs);
                    }
                })
            }));
        }
        layer = layer.child(
            panel(sx, sy).max_h(300.0).child(
                col().key("menu-playlists").w_full().max_h(288.0).scroll(ScrollSpec { stick_bottom: false }).child(list),
            ),
        );
    }
    if let (true, Some((share_y, url))) = (m.share, share) {
        let link = crate::share::Link::Url(url);
        let at = m.at;
        layer = layer.child(share_panel(x, y + share_y, win, std::rc::Rc::new(move |s: &mut State, via| {
            s.ui.menu = None;
            s.share_link(&link, via, at);
        })));
    }
    Some(layer)
}

#[cfg(test)]
mod tests {
    use super::place_y;
    use crate::models::{CollectionKind, Song};
    use crate::state::{Load, Route, State};
    use crate::ui::{lists, select};
    use rune::{Env, Runtime};

    /// The playlist scene with `pl-1` also at row 5: one song, two rows.
    fn duplicated() -> State {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "playlist").unwrap().1)(&mut s);
        let Route::Collection { id, .. } = s.route.clone() else { panic!("not a playlist page") };
        let Some(Load::Ready(c)) = s.collections.get_mut(&id) else { panic!() };
        assert_eq!(c.kind, CollectionKind::Playlist);
        c.tracks[5] = c.tracks[1].clone();
        c.editable = true;
        s
    }

    fn titles(v: &[Song]) -> Vec<String> {
        v.iter().map(|x| x.id.clone()).collect()
    }

    /// Settled frames of the app on `s`.
    fn run(s: State) -> Runtime<State> {
        let mut rt = Runtime::new(s, crate::ui::view, (1180.0, 900.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    fn settle(rt: &mut Runtime<State>) {
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn right_click(rt: &mut Runtime<State>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
        rt.pointer_move(x + 40.0, y + 10.0);
        rt.secondary_down(x + 40.0, y + 10.0);
        rt.secondary_up(x + 40.0, y + 10.0);
        settle(rt);
    }

    fn tap(rt: &mut Runtime<State>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
        rt.pointer_move(x + 8.0, y + 8.0);
        rt.pointer_down(x + 8.0, y + 8.0);
        rt.pointer_up(x + 8.0, y + 8.0);
        settle(rt);
    }

    fn read<T>(rt: &mut Runtime<State>, f: impl FnOnce(&State) -> T) -> T {
        let mut out = None;
        rt.update(|s, _| out = Some(f(s)));
        out.unwrap()
    }

    #[test]
    fn a_song_listed_twice_gets_one_row_key_per_row() {
        let s = duplicated();
        let keys = lists::row_keys(&s);
        assert_eq!(keys[1], "pl-1");
        assert_ne!(keys[5], keys[1], "the second copy has its own key");
        let unique: std::collections::HashSet<&String> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
        // Element keys of the rows stay unique too (the selection wrapper and the "…" button).
        let rt = run(s);
        assert!(rt.find(&select::mark_key("coll:p.0", &keys[1])).is_some());
        assert!(rt.find(&select::mark_key("coll:p.0", &keys[5])).is_some());
        assert!(rt.find(&format!("more-{}", keys[5])).is_some());
    }

    /// The live bug: Add to Queue from one row of a song a playlist holds twice queued it twice.
    #[test]
    fn add_to_queue_on_one_row_of_a_duplicated_song_queues_it_once() {
        let mut rt = run(duplicated());
        right_click(&mut rt, "t5-pl-1");
        let (sel, menu) = read(&mut rt, |s| (s.ui.sel.keys.len(), s.ui.menu.as_ref().map(|m| titles(&m.songs))));
        assert_eq!(sel, 1, "only the clicked row is selected");
        assert_eq!(menu.as_deref(), Some(&["pl-1".to_string()][..]));
        tap(&mut rt, "menu-queue");
        assert_eq!(read(&mut rt, |s| titles(&s.queue.user.iter().cloned().collect::<Vec<_>>())), ["pl-1"]);

        // Play Next, from the first copy this time.
        right_click(&mut rt, "t1-pl-1");
        tap(&mut rt, "menu-next");
        assert_eq!(read(&mut rt, |s| titles(&s.queue.user.iter().cloned().collect::<Vec<_>>())), ["pl-1", "pl-1"], "one more, on top");
    }

    #[test]
    fn play_starts_at_the_clicked_copy() {
        let mut rt = run(duplicated());
        right_click(&mut rt, "t5-pl-1");
        tap(&mut rt, "menu-play");
        let (pos, ctx) = read(&mut rt, |s| (s.queue.pos, s.queue.context.len()));
        assert_eq!((pos, ctx), (5, 14), "row 5, not the first copy at row 1");
    }

    #[test]
    fn remove_from_playlist_drops_the_clicked_copy_in_a_sorted_view() {
        use crate::ui::songs_table::{Col, Sort};
        for sort in [Sort { col: None, asc: false }, Sort { col: Some(Col::Title), asc: true }] {
            let mut s = duplicated();
            s.ui.table.songs_view = true;
            s.ui.table.pl_sort = sort;
            let list = lists::list_id(&s).unwrap();
            let Route::Collection { id, .. } = s.route.clone() else { panic!() };
            let c = s.collections[&id].ready().unwrap().clone();
            let mut c = crate::models::Collection { id: "p.dup".into(), ..c };
            for t in &mut c.tracks {
                t.id = format!("i.{}", t.id); // library songs
            }
            s.collections.insert("p.dup".into(), Load::Ready(c.clone()));
            s.collections.remove(&id);
            s.route = Route::Collection { id: "p.dup".into(), kind: CollectionKind::Playlist };
            let list = lists::list_id(&s).unwrap_or(list);
            let rows = lists::visible_rows_at(&s);
            assert_ne!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), (0..rows.len()).collect::<Vec<_>>(), "the view is not Apple's order");
            // The second copy of pl-1 (Apple's position 5), wherever it shows.
            let key = lists::row_keys(&s)[5].clone();
            super::open(&mut s, &list, &key, [100.0, 100.0]);
            let rows = s.ui.menu.as_ref().unwrap().rows.clone();
            assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [5], "{sort:?}");
            s.remove_from_playlist(&c, &rows);
            assert!(s.sent.borrow().last().unwrap().starts_with("/v1/me/library/playlists/p.dup/tracks "), "Apple's list is read first");
            let mut want: Vec<String> = c.tracks.iter().map(|t| t.id.clone()).collect();
            want.remove(5);
            let now: Vec<String> = s.collections["p.dup"].ready().unwrap().tracks.iter().map(|t| t.id.clone()).collect();
            assert_eq!(now, want, "{sort:?}: Apple's position 5 left the page, the first copy stayed");
        }
    }

    #[test]
    fn selecting_both_copies_acts_on_the_song_once() {
        let mut s = duplicated();
        let list = lists::list_id(&s).unwrap();
        let keys = lists::row_keys(&s);
        select::click(&mut s, &list, &keys[1], rune::Mods::default());
        select::click(&mut s, &list, &keys[5], rune::Mods { cmd: true, ctrl: true, ..rune::Mods::default() });
        assert_eq!(select::selected_rows(&s).len(), 2);
        assert_eq!(titles(&select::selected(&s)), ["pl-1"]);
        super::open(&mut s, &list, &keys[5], [100.0, 100.0]);
        let m = s.ui.menu.as_ref().unwrap();
        assert_eq!(titles(&m.songs), ["pl-1"], "queue, playlist and favourite actions get it once");
        assert_eq!(m.rows.len(), 2, "Remove from Playlist sees both rows");
        let songs = m.songs.clone();
        s.add_to_queue(&songs);
        assert_eq!(s.queue.user.len(), 1);

        // A Shift range over both copies too.
        select::click(&mut s, &list, &keys[0], rune::Mods::default());
        select::click(&mut s, &list, &keys[6], rune::Mods { shift: true, ..rune::Mods::default() });
        assert_eq!(select::selected_rows(&s).len(), 7);
        assert_eq!(select::selected(&s).len(), 6);
    }


    #[test]
    fn menus_open_below_the_pointer_or_flip_above_it() {
        assert_eq!(place_y(100.0, 200.0, 800.0), 100.0, "room below");
        assert_eq!(place_y(760.0, 180.0, 800.0), 580.0, "near the bottom: its bottom at the pointer");
        assert_eq!(place_y(150.0, 700.0, 800.0), 92.0, "taller than either side: as low as fits");
        assert_eq!(place_y(10.0, 900.0, 800.0), 8.0);
    }
}
