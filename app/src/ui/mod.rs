//! The view: sidebar | content | player bar, plus session banners and global keys.

mod artist;
mod artist_link;
mod artists;
mod backdrop;
mod browse;
mod cards;
mod caret;
mod carousel;
mod collection;
mod ctx_menu;
mod dnd;
mod filter;
mod grids;
mod home;
mod icons;
pub mod intro;
mod keys;
mod lib_search;
mod library;
mod lists;
mod local_files;
mod local_lib;
#[cfg(test)]
mod lyrics_snap_tests;
mod marquee;
mod menu;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // Linux builds it for the tests only
pub(crate) mod menu_bar;
mod narrow;
mod new_playlist;
mod now_playing;
pub mod onboarding;
mod player_bar;
mod right_panel;
mod search;
mod select;
pub mod settings;
pub(crate) mod settings_search;
mod sidebar;
mod songs_table;
pub use songs_table::TableView;
#[cfg(test)]
pub use songs_table::Which;
mod tour;
mod tracks;
mod up_next;
mod up_next_anim;
mod widgets;

use rune::{Cursor, Cx, El, KeyEvent, TextEdit, Tx, col, el, icon, row, text};


use crate::state::{Route, State};
use crate::theme::*;
pub use search::Tab as SearchTab;
pub use songs_table::{Col, Sort, fold};
use widgets::{Grid, small_button};

/// View-only state.
#[derive(Default)]
pub struct UiState {
    /// The Search page's field (Apple Music and Library tabs share it).
    pub search: TextEdit,
    /// The Search page's tab (kept while the app runs).
    pub search_tab: search::Tab,
    /// Library search lifts its caps ("Show all").
    pub lib_show_all: bool,
    /// Library-search index and last result.
    lib: lib_search::Cache,
    /// Sorted artist index.
    artists: artists::Cache,
    /// Filtered Library > Albums.
    albums: lists::AlbumCache,
    /// The open card / pin / artist context menu.
    ctx: Option<ctx_menu::CtxMenu>,
    /// The compact player bar's volume popover is open.
    pub volume_open: bool,
    /// Songs-table view mode, sorts and the sorted-order cache.
    pub table: songs_table::TableState,
    /// Full-screen Now Playing is showing.
    pub now_playing: bool,
    /// The right sidebar's follow state.
    right: right_panel::PanelState,
    up_next_open: u8,
    up_next_placed: bool,
    /// The Up Next "History" pill, driven by the list's scroll offset (no view rebuild).
    up_next_hint: up_next::HistoryHint,
    /// Up Next rows sliding to their new places after an edit (`up_next_anim.rs`).
    up_next_anim: up_next_anim::UpNextAnim,
    /// The New Playlist dialog (Ctrl+N).
    pub new_pl: new_playlist::Dialog,
    /// Selected rows of the page's song list.
    pub sel: select::Selection,
    /// The Ctrl+F filter field.
    pub filter: lists::Filter,
    /// The open song context menu.
    pub menu: Option<menu::MenuState>,
    /// Display-order cache of the page's song list.
    order: lists::OrderCache,
    row_keys: lists::RowKeysCache,
    /// Drag and drop: pin reorder, songs onto playlists / the queue.
    pub dnd: dnd::Dnd,
    /// The Settings overlay (Ctrl+,).
    pub settings: settings::SettingsUi,
    /// Settings → Keyboard is waiting for the new keys of `keys::BINDINGS[i]`.
    pub recording: Option<usize>,
    /// The startup intro or first-sync screen, while it shows (`intro.rs`).
    pub intro: Option<intro::Intro>,
    /// The first-run onboarding, or its replay, while it shows (`onboarding.rs`).
    pub onboarding: Option<onboarding::Onboarding>,
    /// When a library sync that went on in the background finished (poll time): the progress
    /// pill says "Library synced" for a moment.
    pub sync_done: Option<f64>,
    /// The seek bars' fills, which follow the playback clock (`player_bar::poll`).
    pub seek: player_bar::SeekFills,
    /// Lines of text that scroll when they don't fit (the player bar's title and artist).
    pub marquee: marquee::Marquees,
    /// Carousel items near the window, whose covers load (`carousel.rs`).
    pub near: std::collections::HashSet<String>,
    /// The window's width at the last layout (0 before the first), for handlers that act
    /// differently in a narrow window (`narrow.rs`).
    pub win_w: f32,
    /// Short shakes for what a narrow window can't open (`narrow.rs`).
    pub shakes: narrow::Shakes,
    /// Whether the focus came from Tab (Enter presses it) or a click (`keys::KbdFocus`).
    pub kbd: keys::KbdFocus,
    /// The focused text field's caret blink (`caret.rs`).
    caret: caret::Blink,
}

impl UiState {
    /// Forget per-page state (selection, filter, menu). `State::navigate`/`back` call this.
    pub fn route_changed(&mut self) {
        self.sel.clear();
        self.filter.open = false;
        self.filter.edit.clear();
        self.menu = None;
        self.ctx = None;
        self.volume_open = false;
        self.lib_show_all = false;
        self.near.clear();
    }
}

pub use new_playlist::{open as new_playlist_open, open_edit as edit_playlist_open};
pub use now_playing::{close as close_now_playing, open as open_now_playing};
pub use now_playing::{scene_np_cover_only, scene_np_motion, scene_np_interlude, scene_np_intro, scene_np_lines_only, scene_np_unsynced, scene_np_words};

/// Page horizontal padding.
pub const PAGE_PX: f32 = 32.0;

/// `AppSpec::poll`: returns whether anything visible changed.
pub fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    crate::framelog::frame(t);
    let queue_open = if s.ui.now_playing && s.settings.now_playing_queue { 2 }
        else if right_panel::visible(s) && s.settings.right_tab == crate::settings::RightTab::Queue { 1 } else { 0 };
    if queue_open != s.ui.up_next_open {
        s.ui.up_next_open = queue_open;
        s.ui.up_next_placed = false;
        s.ui.up_next_anim.reset();
    }
    // Rows snapped to their old places last frame start moving (render-side, no view run).
    s.ui.up_next_anim.poll(t, tx);
    // Data that landed since the last frame makes this frame's commit instant: the lyrics
    // views and a pin drag then hold a change back a frame so it glides (see `State::landing`).
    let landed = crate::state::take_landed();
    let a = dnd::poll(s, t, landed, tx);
    let b = now_playing::poll(s, landed, tx);
    let c = right_panel::poll(s, landed, tx);
    let d = new_playlist::poll(s);
    let e = intro::poll(s, t, tx);
    let f = intro::poll_sync(s, t, tx);
    let g = player_bar::poll(s, t, tx);
    let h = onboarding::poll(s, t, tx);
    let i = settings::poll(s, t, tx);
    let j = caret::poll(s, t, tx);
    a || b || c || d || e || f || g || h || i || j
}

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    // The palette for this build: the Theme setting, or the OS appearance (`--theme light|dark` headless).
    let light = s.settings.theme.is_light(cx.system_dark());
    crate::theme::apply(light);
    // Light: text one step darker while a background picture shows (it is printed pale, never
    // past `uzumaki_fx::LIGHT_FLOOR`, and this ink keeps 4.5:1 on it).
    crate::theme::set_over_picture(light && backdrop::app_plan(&s.settings.appearance, s.playback.song.as_ref()).is_some());
    // The keyboard focus ring (Rune draws it only for Tab focus, as `:focus-visible`): coral,
    // just outside the control, following its corners.
    cx.focus_ring(2.0, 2.0, PAL.accent);
    // App icon: Follow theme needs the OS appearance, which only the view sees.
    s.report_system_dark(cx.system_dark());
    // Animated artwork holds still under Reduce Motion, which only the view sees.
    s.report_reduced_motion(cx.is_reduced_motion());
    // macOS: the menu bar follows the state and the user's shortcuts (nothing headless).
    #[cfg(target_os = "macos")]
    menu_bar::sync(s);
    crate::dock_icon::sync(s.settings.app_icon.resolve(s.settings.theme, Some(cx.system_dark())).unwrap_or(crate::settings::IconVariant::Dark));
    // Covers are decoded for this scale (`art::memory_px`).
    crate::art::set_scale(cx.env().scale);
    // Covers drawn from here on are held in memory until a view leaves them out (`Art::get`).
    s.art.begin_frame();
    // Width available to page content, from the last layout (a first-frame guess until then).
    let avail = cx.layout_size("main").map_or(1076.0, |[w, _]| w - 2.0 * PAGE_PX).max(300.0);
    let grid = Grid::new(avail);

    let win = cx.layout_size("app-root").unwrap_or([1360.0, 860.0]);
    carousel::set_window(win);
    let rail = sidebar::is_rail(s, win[0]);
    let reduced = cx.is_reduced_motion();
    caret::set_reduced(reduced);
    let compact = win[0] < COMPACT_W;
    // macOS draws the traffic lights over the window's top-left (Rune's full-size content), so the
    // sidebar keeps a band clear for them. None in full screen, and Linux draws its own frame.
    let chrome_top = if cfg!(target_os = "macos") && cx.titlebar_inset() > 0.0 { TITLEBAR_BAND } else { 0.0 };

    let page = match &s.route {
        Route::Home => home::page(s, grid),
        Route::Library(tab) => library::page(s, *tab, grid, avail),
        Route::Search => search::page(s, grid),
        Route::Collection { id, .. } => collection::page(s, id, avail),
        Route::New => browse::new_page(s),
        Route::MadeForYou => browse::made_for_you_page(s),
        Route::LocalFiles => local_files::page(s, avail),
        Route::Radio => browse::radio_page(s, avail),
        Route::RecentlyAdded => grids::recently_added_page(s, grid),
        Route::Artists | Route::LocalArtists => artists::page(s, win[1]),
        Route::Artist { id, name } if crate::local::view::is_artist_id(id) => local_lib::artist_page(s, id, name, grid, avail),
        Route::Artist { id, name } => artist::page(s, id, name, grid, avail),
        Route::ArtistShelf { id, name, shelf } => artist::shelf_page(s, id, name, *shelf, grid),
        Route::LocalAlbums => local_lib::albums_page(s, grid),
        Route::Folders { path } => local_lib::folder_page(s, path.as_deref(), avail),
    };

    // No fill: the root's background (and the background picture, behind everything) shows.
    let mut main = col().key("main").grow(1.0).min_w(0.0).h_full();
    if !backdrop::np_active(s) {
        s.backdrops.hide(crate::backdrop::Slot::NowPlaying);
    }
    if !settings::preview_active(s) {
        s.backdrops.hide(crate::backdrop::Slot::Preview);
    }
    main = main.child(banners(s));
    if !s.back_stack.is_empty() {
        main = main.child(
            row().w_full().px(PAGE_PX - 8.0).pt(10.0).child(
                el().key("back").role("button").label("Back").focusable(true).size(32.0, 32.0).radius(16.0).center()
                    .hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
                    .on_press(|s: &mut State| s.back())
                    .child(text("\u{2039}").font(rune::TextStyle::new(26.0, 500.0)).color(PAL.text2)),
            ),
        );
    }
    main = main.child(page);
    // A library download going on behind the app (the sync screen was skipped).
    if let Some(p) = intro::sync_pill(s) {
        main = main.child(p);
    }
    // Shakes when the right panel can't open and no player bar shows its buttons.
    let main = narrow::bind(s, narrow::Target::Page, main, reduced);

    let panel = right_panel::visible(s);
    let overlay = win[0] < right_panel::OVERLAY_BELOW;
    let mut top = row().grow(1.0).min_h(0.0).w_full()
        .child(sidebar::sidebar(s, rail, chrome_top, reduced))
        .child(el().w(1.0).h_full().fill(PAL.divider))
        .child(main);
    if panel && !overlay {
        top = top.child(right_panel::panel(s, cx, false));
    }
    let mut root = col().key("app-root").w_full().h_full().fill(PAL.bg)
        // Handlers need the width too (`narrow.rs`); the view only runs again when it crosses the line.
        .on_layout_tx(|s: &mut State, r: [f32; 4], tx: &mut Tx| {
            let was = narrow::narrow(s);
            s.ui.win_w = r[2];
            if narrow::narrow(s) == was {
                tx.live_only();
            }
        });
    // The background picture is window-sized and behind every panel (the sidebar, right panel and
    // player bar are opaque), so opening a panel or collapsing the sidebar reveals more or less of
    // the same picture instead of re-rendering it at a new size.
    if let Some(bg) = backdrop::app_layer(s, cx) {
        root = root.child(bg);
    }
    root = root.child(top);
    if s.playback.song.is_some() {
        if s.settings.player_style == crate::settings::PlayerStyle::Floating {
            root = root.child(player_bar::floating_bar(s, win[0], cx.is_reduced_motion()));
            if s.ui.volume_open {
                let (right, bottom) = player_bar::floating_popover_pos(win[0]);
                root = root.child(player_bar::volume_popover(s, right, bottom));
            }
        } else {
            root = root.child(player_bar::player_bar(s, compact, cx.is_reduced_motion()));
            if compact && s.ui.volume_open {
                root = root.child(player_bar::volume_popover(s, 14.0, PLAYER_H + 8.0));
            }
        }
    }
    if panel && overlay {
        root = root.child(right_panel::panel(s, cx, true));
    }
    if s.ui.now_playing {
        root = root.children(crate::theme::dark_scope(|| now_playing::overlay(s, cx)));
    }
    // Why a narrow window shook something; it floats over the page and Now Playing.
    root = root.children(narrow::hints(s, win[0], chrome_top, reduced));
    if s.ui.dnd.active {
        root = root.child(dnd::ghost(s));
    }
    if let Some(m) = menu::overlay(s, win) {
        root = root.child(m);
    }
    if let Some(m) = ctx_menu::overlay(s, win) {
        root = root.child(m);
    }
    if let Some(m) = settings::overlay(s, cx, win) {
        root = root.child(m);
    }
    if let Some(m) = new_playlist::overlay(s, win) {
        root = root.child(m);
    }
    // The intro sits beside the app, not in it, since it folds and twists the whole app root;
    // and under it, so the app twists out in front of the swirl (it is clear until then).
    // Space (play/pause) beats the focused control's own press, and Enter presses it only after
    // Tab: see `keys::control_key`. Clicks end keyboard focus (`keys::pointer`).
    let mut window = el().key("window").w_full().h_full()
        .on_key(|s: &mut State, ev: &KeyEvent, tx: &mut Tx| keys::control_key(s, ev, tx))
        .on_pointer_move_local(|s: &mut State, p, tx: &mut Tx| {
            keys::pointer(s, p.window_x, p.window_y);
            tx.live_only();
        });
    // The onboarding takes the intro's place while it is up (it starts with none).
    if onboarding::active(s) {
        window = window.children(onboarding::overlay(s, cx, win));
        return window.child(onboarding::app(s, cx, root));
    }
    if let Some(i) = intro::overlay(s, cx, win) {
        window = window.child(i);
    }
    window.child(intro::app(s, cx, root))
}

fn banner(key: &'static str, bg: rune::Color) -> El {
    row().key(key).w_full().min_h(44.0).px(16.0).py(8.0).gap(12.0).center_y().radius(10.0).fill(bg)
}

/// Session and toast banners at the top of the content area.
fn banners(s: &State) -> El {
    let mut list: Vec<El> = Vec::new();
    let ses = &s.session;
    let msg = |t: String, c: rune::Color| text(t).font(BODY14).color(c).grow(1.0).min_w(0.0).wrap();

    if s.local_only() {
        // No Apple Music: nothing to start or sign in to.
    } else if let Some(err) = &ses.error {
        list.push(banner("banner-error", PAL.danger_bg).child(msg(err.clone(), PAL.danger_text)));
    } else if !ses.bridge_up {
        list.push(banner("banner-starting", PAL.chip).child(msg("Starting playback engine\u{2026}".into(), PAL.dim)));
    } else if !ses.authorized {
        if ses.login_window {
            list.push(
                banner("banner-login", PAL.chip)
                    .child(msg("Finish signing in in the Apple Music window".into(), PAL.text2))
                    .child(small_button("hide-login", "Hide window").on_press(|s: &mut State| s.show_login(false))),
            );
        } else {
            list.push(
                banner("banner-login", PAL.chip)
                    .child(msg("Sign in to Apple Music to play full songs and see your library".into(), PAL.text2))
                    .child(small_button("sign-in", "Sign in").on_press(|s: &mut State| s.sign_in())),
            );
        }
    }
    if let Some(n) = &s.update {
        list.push(update_notice(n));
    }
    if let Some(t) = &s.toast {
        list.push(
            banner("toast", PAL.nav_active).child(msg(t.clone(), PAL.text)).child(
                el().key("toast-dismiss").role("button").label("Dismiss").focusable(true).size(28.0, 28.0).radius(14.0).center()
                    .hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
                    .on_press(|s: &mut State| s.toast = None)
                    .child(icon(&icons::CLOSE, 14.0).color(PAL.muted)),
            ),
        );
    }
    if list.is_empty() {
        return col();
    }
    col().w_full().px(PAGE_PX).pt(16.0).gap(8.0).children(list)
}

/// A newer version is out (`update.rs`): a quiet banner. What's new and Download open the
/// browser; nothing is downloaded by the app. Package installs get no Download button. Below
/// `minimum_supported` it is a warning (danger colours) without Skip and ✕.
fn update_notice(n: &crate::update::Notice) -> El {
    let open = |key: &'static str, label: &'static str, url: String| {
        small_button(key, label).on_press_tx(move |s: &mut State, tx: &mut rune::Tx| {
            if !crate::update::is_web(&url) || tx.open_url(&url).is_err() {
                s.toast = Some("Couldn't open the link.".into());
            }
        })
    };
    let (bg, fg) = if n.required { (PAL.danger_bg, PAL.danger_text) } else { (PAL.chip, PAL.text2) };
    let mut b = banner("update-notice", bg).child(text(n.message()).font(BODY14).color(fg).grow(1.0).min_w(0.0).wrap());
    if crate::update::is_web(&n.notes_url) {
        b = b.child(open("update-notes", "What\u{2019}s new", n.notes_url.clone()));
    }
    if let Some(url) = &n.download {
        b = b.child(open("update-download", "Download", url.clone()));
    }
    if n.required {
        return b;
    }
    b.child(
        el().key("update-skip").role("button").label("Skip this version").focusable(true).h(30.0).px(10.0).radius(15.0).center()
            .hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
            .on_press(|s: &mut State| s.skip_update())
            .child(text("Skip this version").font(rune::TextStyle::new(13.0, 500.0)).color(PAL.muted)),
    )
    .child(
        el().key("update-dismiss").role("button").label("Dismiss").focusable(true).size(28.0, 28.0).radius(14.0).center()
            .hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
            .on_press(|s: &mut State| s.dismiss_update())
            .child(icon(&icons::CLOSE, 14.0).color(PAL.muted)),
    )
}

/// Scene helper: selects rows of the page's list by row key (a song's id names its first row),
/// and optionally opens the context menu.
pub fn scene_select(s: &mut State, ids: &[&str], menu_at: Option<[f32; 2]>) {
    let Some(list) = lists::list_id(s) else { return };
    s.ui.sel.list = list.clone();
    s.ui.sel.keys = ids.iter().map(|x| x.to_string()).collect();
    if let (Some(at), Some(first)) = (menu_at, ids.first()) {
        menu::open(s, &list, first, at);
    }
}

/// Scene helpers: mid-drag frames.
pub fn scene_drag_pin(s: &mut State, from: usize, to: usize, dy: f32) {
    dnd::scene_drag_pin(s, from, to, dy);
}

pub fn scene_drag_songs(s: &mut State, at: [f32; 2], over_playlist: Option<&str>) {
    let over = over_playlist.map(|id| dnd::Over { target: dnd::Target::Playlist(id.into()), ok: dnd::playlist_ok(s, id) });
    dnd::scene_drag_songs(s, at, over);
}

/// Scene helpers: open context menus.
pub fn scene_menu_pin(s: &mut State, id: &str, at: [f32; 2]) {
    ctx_menu::open_pin(s, id, at);
}

pub fn scene_menu_collection(s: &mut State, c: &crate::models::Collection, at: [f32; 2]) {
    ctx_menu::open_collection(s, c, at);
}

pub fn scene_submenu(s: &mut State) {
    if let Some(m) = &mut s.ui.menu {
        m.sub = true;
    }
}

/// Esc closes, in order: the New Playlist dialog, Settings (its search first), the context menu,
/// the filter, Now Playing.
fn escape(s: &mut State) -> bool {
    if new_playlist::is_open(s) {
        new_playlist::close(s);
    } else if s.ui.settings.open.is_some() {
        settings::escape(s);
    } else if s.ui.menu.is_some() {
        s.ui.menu = None;
    } else if s.ui.ctx.is_some() {
        s.ui.ctx = None;
    } else if s.ui.volume_open {
        s.ui.volume_open = false;
    } else if lists::filter_active(s) {
        filter::close(s);
    } else if s.ui.now_playing {
        close_now_playing(s);
    } else {
        return false;
    }
    true
}

pub fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if keys::track_focus(s, ev, tx) {
        return true;
    }
    // The onboarding is a Rune modal: the app's shortcuts wait; its tour has its own keys.
    if onboarding::active(s) {
        return onboarding::key(s, ev, tx);
    }
    // Every shortcut lives in `keys::BINDINGS` (Settings → Keyboard lists the same table).
    keys::handle(s, ev, tx)
}

#[cfg(test)]
pub use keys::slot_target;

/// Scenes: the `keys::BINDINGS` index of the action with this id.
pub fn keys_index(id: &str) -> Option<usize> {
    keys::BINDINGS.iter().position(|b| b.action.id() == id)
}

/// Scenes: the song menu as the player bar opens it.
pub fn scene_song_menu(s: &mut State, song: crate::models::Song, at: [f32; 2]) {
    menu::open_song(s, song, menu::MenuContext::NowPlaying, at);
}
