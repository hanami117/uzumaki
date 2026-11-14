//! The Settings overlay (Ctrl+,): a modal panel with Account / Playback / General pages.

use rune::{Cx, Cursor, El, Key, KeyEvent, ScrollSpec, TextEdit, TextStyle, Tx, col, el, icon, row, text, text_input};

use super::icons;
use super::settings_search::{self as search};
pub use super::settings_search::Row;
use super::widgets::{chip, slider, small_button};
use crate::appearance::{BgSource, Effect, NpBg, ShowOn, opacity_range};
use crate::backdrop::{Request, Slot};
use crate::settings::{AppIcon, MusicSource, PlayerStyle, StartPage, Theme};
use crate::state::{CacheKind, State};
use crate::theme::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Account,
    Playback,
    Appearance,
    General,
    Local,
    /// Every keyboard shortcut (`ui::keys::BINDINGS`).
    Keyboard,
}

impl Tab {
    pub(super) const ALL: [Tab; 6] = [Tab::Account, Tab::Playback, Tab::Appearance, Tab::General, Tab::Local, Tab::Keyboard];

    fn info(self) -> (&'static str, &'static str, &'static rune::Icon) {
        match self {
            Tab::Account => ("settings/tab/account", "Account", &icons::USER),
            Tab::Playback => ("settings/tab/playback", "Playback", &icons::SLIDERS),
            Tab::Appearance => ("settings/tab/appearance", "Appearance", &icons::IMAGE),
            Tab::General => ("settings/tab/general", "General", &icons::GEAR),
            Tab::Local => ("settings/tab/local", "Local Files", &icons::LOCAL_FILES),
            Tab::Keyboard => ("settings/tab/keyboard", "Keyboard", &icons::KEYBOARD)
        }
    }

    /// The page's name.
    pub fn title(self) -> &'static str {
        self.info().1
    }
}

/// Overlay state.
#[derive(Default)]
pub struct SettingsUi {
    /// The open page, `None` while the overlay is closed.
    pub open: Option<Tab>,
    /// The page to reopen on.
    pub last: Tab,
    /// The "really sign out?" row is showing.
    pub confirm: bool,
    /// The search field atop the page list. While it holds text, the pages give way to the rows
    /// it finds (`settings_search`).
    pub query: TextEdit,
    /// What `query` finds: indices into `settings_search::INDEX`, updated on each edit.
    pub hits: Vec<usize>,
    /// The row a search jump landed on, tinted for `FLASH` seconds; `flash_at` is when it began
    /// (set by `poll`, which has the clock).
    pub flash: Option<Row>,
    flash_at: Option<f64>,
}

/// How long a row a search jumped to stays tinted.
const FLASH: f64 = 1.4;

pub fn open(s: &mut State) {
    s.ui.settings.open = Some(s.ui.settings.last);
    s.ui.settings.confirm = false;
    s.refresh_cache_sizes();
}

/// Opens the overlay on `tab`.
pub fn open_tab(s: &mut State, tab: Tab) {
    open(s);
    select(s, tab);
}

pub fn close(s: &mut State) {
    s.ui.settings.open = None;
    s.ui.recording = None;
    s.ui.settings.confirm = false;
    clear_search(s);
    s.ui.settings.flash = None;
}

/// Esc while Settings is open: a search is cleared first, the next Esc closes.
pub fn escape(s: &mut State) {
    if !s.ui.settings.query.text.is_empty() {
        clear_search(s);
    } else {
        close(s);
    }
}

/// Ctrl+F while Settings is open: focus its search field (the page's own filter is behind it).
pub fn focus_search(s: &mut State) {
    s.ui.settings.query.focused = true;
}

/// True while the search field holds more than spaces.
pub fn searching(s: &State) -> bool {
    !s.ui.settings.query.text.trim().is_empty()
}

fn clear_search(s: &mut State) {
    s.ui.settings.query.clear();
    s.ui.settings.hits.clear();
}

/// The rows the search shows: its hits that exist in the app's current state, as
/// `(page, row)` in page order.
pub fn results(s: &State) -> Vec<(Tab, Row)> {
    s.ui.settings.hits.iter().filter_map(|&i| search::INDEX.get(i)).filter(|e| available(s, e.row)).map(|e| (e.tab, e.row)).collect()
}

/// Leaves the search for `tab`, scrolled to `row`, which is tinted for a moment.
fn jump(s: &mut State, tab: Tab, row: Row, tx: &mut Tx) {
    select(s, tab);
    s.ui.settings.flash = Some(row);
    s.ui.settings.flash_at = None;
    tx.scroll_center(row.key());
}

/// Enter in the search field: go to the first result.
fn jump_first(s: &mut State, tx: &mut Tx) {
    if let Some(&(tab, row)) = results(s).first() {
        jump(s, tab, row, tx);
        tx.blur();
    }
}

/// Ends the tint of a row a search jumped to. True when that changes the view.
pub fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let st = &mut s.ui.settings;
    if st.flash.is_none() {
        return false;
    }
    let at = *st.flash_at.get_or_insert(t);
    if t - at >= FLASH {
        st.flash = None;
        st.flash_at = None;
        return true;
    }
    tx.wake_after(std::time::Duration::from_secs_f64(FLASH - (t - at)));
    false
}

pub fn toggle(s: &mut State) {
    if s.ui.settings.open.is_some() {
        close(s);
    } else {
        open(s);
    }
}

fn select(s: &mut State, tab: Tab) {
    s.ui.settings.open = Some(tab);
    s.ui.settings.last = tab;
    s.ui.settings.confirm = false;
    clear_search(s);
}

/// "0 B", "820 KB", "3.4 MB", "1.2 GB".
pub fn fmt_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let f = n as f64;
    if f < KB {
        format!("{n} B")
    } else if f < KB * KB {
        format!("{:.0} KB", f / KB)
    } else if f < KB * KB * KB {
        format!("{:.1} MB", f / (KB * KB))
    } else {
        format!("{:.1} GB", f / (KB * KB * KB))
    }
}

// ---- controls ----

fn segment(key: &'static str, label: &str, on: bool) -> El {
    let (bg, fg) = if on { (crate::theme::seg_on(), PAL.text) } else { (rune::Color::TRANSPARENT, PAL.muted) };
    let e = el().key(key).role("button").label(label).focusable(true).h(30.0).px(14.0).radius(8.0).center().fill(bg)
        .cursor(Cursor::Pointer);
    let e = if on { e } else { e.hover(crate::theme::ov(0.05)) };
    e.child(text(label).font(rune::TextStyle::new(13.0, 500.0)).color(fg))
}

fn segmented(key: &'static str, segs: Vec<El>) -> El {
    row().key(key).padding(3.0).gap(2.0).radius(11.0).fill(PAL.chip).shrink().children(segs)
}

/// One App icon choice: the icon itself, 64 pt, over its name (Follow theme: dark and light
/// halves). The chosen one has a coral ring;
/// the ring's place is reserved (transparent) on the others so nothing moves.
fn icon_tile(which: AppIcon, on: bool) -> El {
    let label = which.label();
    let key = match which {
        AppIcon::Dark => "settings/app-icon/dark",
        AppIcon::Light => "settings/app-icon/light",
        AppIcon::Coral => "settings/app-icon/coral",
        AppIcon::Auto => "settings/app-icon/auto",
    };
    let ring = if on { PAL.accent } else { rune::Color::TRANSPARENT };
    let tile = col().key(key).role("button").label(format!("{label} app icon")).focusable(true).w(98.0).py(8.0).gap(6.0).items_center()
        .radius(16.0).border(2.0, ring).cursor(Cursor::Pointer);
    let tile = if on { tile } else { tile.hover(crate::theme::ov(0.05)) };
    tile.child(super::widgets::app_icon_preview(which, 64.0))
        .child(text(label).font(rune::TextStyle::new(13.0, if on { 600.0 } else { 500.0 })).color(if on { PAL.text } else { PAL.muted }))
        .on_press(move |s: &mut State| s.set_app_icon(which))
}

fn switch(key: &'static str, label: &'static str, on: bool, enabled: bool, set: impl Fn(&mut State, bool) + 'static) -> El {
    let knob = el().size(18.0, 18.0).radius(9.0).fill(rune::Color::WHITE);
    let track = row().key(key).role("switch").label(label).focusable(enabled).w(40.0).h(24.0).px(3.0).center_y().radius(12.0)
        .fill(if on { PAL.accent } else { PAL.track });
    let track = if on { track.justify_end() } else { track }.child(knob);
    if enabled {
        track.cursor(Cursor::Pointer).on_press(move |s: &mut State| set(s, !on))
    } else {
        track.opacity(0.4).cursor(Cursor::NotAllowed)
    }
}

fn danger_button(key: &'static str, label: &'static str, solid: bool) -> El {
    let (bg, fg) = if solid { (PAL.danger_text, PAL.ink) } else { (PAL.danger_bg, PAL.danger_text) };
    el().key(key).role("button").label(label).focusable(true).h(30.0).px(14.0).radius(15.0).center().fill(bg)
        .hover(crate::theme::ov(0.06)).press(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
        .child(text(label).font(rune::TextStyle::new(13.0, 600.0)).color(fg))
}

// ---- rows ----

fn divider() -> El {
    el().w_full().h(1.0).fill(PAL.divider)
}

/// A settings row: title + description left, control right (below them when `narrow`).
fn setting(narrow: bool, title: &str, desc: Option<String>, control: El) -> El {
    let mut info = col().gap(4.0).grow(1.0).min_w(0.0).child(text(title.to_string()).font(BODY14).color(PAL.text).wrap());
    if let Some(d) = desc {
        info = info.child(text(d).font(SMALL13).color(PAL.dim).wrap());
    }
    if narrow {
        col().w_full().py(16.0).gap(12.0).items_start().child(info).child(control)
    } else {
        row().w_full().py(16.0).gap(24.0).center_y().child(info).child(el().shrink().child(control))
    }
}

// ---- rows, by id (the pages and search build the same ones) ----

/// What a row needs to build itself.
struct Rx<'a> {
    s: &'a State,
    cx: &'a Cx<State>,
    narrow: bool,
    /// Built for the search results, out of its page's headings: titles say what they belong to.
    search: bool,
}

/// Whether `row` exists in the app's current state (local only mode has no Apple rows; the
/// cover effect shows only with the Cover background; App icon only on Linux and the Mac).
fn available(s: &State, row: Row) -> bool {
    match row {
        Row::AppleMusic | Row::Quality | Row::Lyrics | Row::AutoPlay => !s.local_only(),
        Row::NpCoverEffect => s.settings.appearance.np == NpBg::Cover,
        Row::AppIcon => cfg!(any(target_os = "linux", target_os = "macos")),
        Row::Binding(i) => i < super::keys::BINDINGS.len(),
        _ => true,
    }
}

/// `row` in its keyed wrapper (search scrolls to it), tinted coral while a search jump flashes
/// it, or `None` where it doesn't exist.
fn item(r: &Rx, row: Row) -> Option<El> {
    if !available(r.s, row) {
        return None;
    }
    let tint = if r.s.ui.settings.flash == Some(row) { PAL.accent.alpha(0.13) } else { rune::Color::TRANSPARENT };
    Some(col().key(row.key()).mx(-12.0).px(12.0).radius(10.0).fill(tint).child(row_el(r, row)))
}

/// The rows that exist of `rows`, a divider between each two.
fn rows(r: &Rx, rows: &[Row]) -> Vec<El> {
    let mut v = Vec::new();
    for e in rows.iter().filter_map(|&row| item(r, row)) {
        if !v.is_empty() {
            v.push(divider());
        }
        v.push(e);
    }
    v
}

fn row_el(r: &Rx, id: Row) -> El {
    let (s, narrow) = (r.s, r.narrow);
    let ap = &s.settings.appearance;
    let bg_on = ap.source != BgSource::Off;
    match id {
        Row::MusicSource => music_source(s, narrow),
        Row::AppleMusic => apple_music(s, narrow),
        Row::Quality => {
            let saver = s.settings.data_saver;
            setting(
                narrow,
                "Streaming quality",
                Some("Data saver uses about a quarter of the data. Lossless and Dolby Atmos aren't available in Apple Music on the web.".into()),
                segmented(
                    "settings/quality",
                    vec![
                        segment("settings/quality/best", "Best \u{b7} 256 kbps", !saver).on_press(|s: &mut State| s.set_data_saver(false)),
                        segment("settings/quality/saver", "Data saver \u{b7} 64 kbps", saver).on_press(|s: &mut State| s.set_data_saver(true)),
                    ],
                ),
            )
        }
        Row::AutoPlay => setting(
            narrow,
            "AutoPlay",
            Some("When the queue ends, similar music keeps playing. Also the \u{221e} button in Up Next. Not with Repeat or on radio stations.".to_string()),
            switch("settings/autoplay", "AutoPlay", s.settings.autoplay, true, |s, on| s.set_autoplay(on)),
        ),
        Row::Resume => setting(
            narrow,
            "Resume where you left off",
            Some("At start, the last song comes back paused where you stopped, with its queue. Nothing plays until you press Play.".to_string()),
            switch("settings/resume", "Resume where you left off", s.settings.resume_playback, true, |s, on| s.set_resume_playback(on)),
        ),
        Row::Lyrics => {
            let words = s.settings.lyrics_words;
            setting(
                narrow,
                "Lyrics highlight",
                Some("Word by word when Apple has word timing, otherwise whole lines.".into()),
                segmented(
                    "settings/lyrics",
                    vec![
                        segment("settings/lyrics/words", "Words", words).on_press(|s: &mut State| s.set_lyrics_words(true)),
                        segment("settings/lyrics/lines", "Lines", !words).on_press(|s: &mut State| s.set_lyrics_words(false)),
                    ],
                ),
            )
        }
        Row::Theme => {
            let system_light = !r.cx.system_dark();
            let theme_seg = |key: &'static str, label: &str, v: Theme| {
                segment(key, label, s.settings.theme == v).on_press(move |s: &mut State| s.set_theme(v))
            };
            setting(
                narrow,
                "Theme",
                Some("System follows your computer's light or dark appearance.".into()),
                segmented(
                    "settings/theme",
                    vec![
                        theme_seg("settings/theme/system", &format!("System ({})", if system_light { "Light" } else { "Dark" }), Theme::System),
                        theme_seg("settings/theme/light", "Light", Theme::Light),
                        theme_seg("settings/theme/dark", "Dark", Theme::Dark),
                    ],
                ),
            )
        }
        Row::AppIcon => {
            // Linux: the installed icon files (`app_icon.rs`). macOS: the Dock (`dock_icon.rs`)
            // and the installed .app (`bundle_icon.rs`); the menu bar keeps the template swirl.
            let note = if MAC {
                "In the Dock and the Finder."
            } else {
                "In the dock, taskbar and menu. The tray icon changes at the next start."
            };
            // Four tiles leave the title no room beside them: always stacked.
            setting(
                true,
                "App icon",
                Some(note.into()),
                row().key("settings/app-icon").gap(4.0).children(AppIcon::ALL.map(|i| icon_tile(i, s.settings.app_icon == i))),
            )
        }
        Row::Player => {
            let style_seg = |key: &'static str, label: &'static str, v: PlayerStyle| {
                segment(key, label, s.settings.player_style == v).on_press(move |s: &mut State| s.set_player_style(v))
            };
            setting(
                narrow,
                "Player",
                Some("The bar at the bottom: full width, or a floating capsule.".into()),
                segmented(
                    "settings/player/style",
                    vec![
                        style_seg("settings/player/docked", "Docked", PlayerStyle::Docked),
                        style_seg("settings/player/floating", "Floating", PlayerStyle::Floating),
                    ],
                ),
            )
        }
        Row::StartupAnimation => setting(
            narrow,
            "Startup animation",
            Some("The Uzumaki swirl twists open when the app starts. The first library sync always shows it.".into()),
            row().gap(10.0).center_y()
                .child(small_button("settings/intro/preview", "Preview").on_press(|s: &mut State| super::intro::preview(s)))
                .child(switch("settings/intro", "Startup animation", s.settings.startup_animation, true, |s, v| s.set_startup_animation(v))),
        ),
        Row::Background => {
            let seg = |key: &'static str, label: &'static str, v: BgSource| {
                segment(key, label, ap.source == v).on_press(move |s: &mut State| s.set_bg_source(v))
            };
            setting(
                narrow,
                "Background",
                Some("A picture behind the content area.".into()),
                segmented(
                    "settings/bg/source",
                    vec![
                        seg("settings/bg/off", "Off", BgSource::Off),
                        seg("settings/bg/builtin", "Uzumaki", BgSource::Builtin),
                        seg("settings/bg/picture", "Picture", BgSource::Picture),
                        seg("settings/bg/cover", "Cover", BgSource::Cover),
                    ],
                ),
            )
        }
        Row::Picture => {
            let picture = match &ap.picture {
                Some(p) => p.file_name().map_or_else(|| tilde(p), |n| n.to_string_lossy().into_owned()),
                None => "No picture chosen".into(),
            };
            let choose = small_button("settings/bg/choose", if ap.picture.is_some() { "Change\u{2026}" } else { "Choose Picture\u{2026}" })
                .on_press_tx(|_s: &mut State, tx: &mut rune::Tx| {
                    tx.open_file(
                        rune::FileDialogOptions { title: "Choose a background picture".into(), ..Default::default() }
                            .filter("Pictures", &["png", "jpg", "jpeg"]),
                        |s: &mut State, result: rune::FileDialogResult, _tx: &mut rune::Tx| match result {
                            Ok(Some(path)) => s.set_bg_picture(path),
                            Ok(None) => {} // cancelled
                            Err(e) => s.toast = Some(format!("Couldn't open the file picker: {e}")),
                        },
                    );
                });
            let mut controls = row().gap(8.0).child(choose);
            if ap.picture.is_some() {
                let remove = el().key("settings/bg/remove").role("button").label("Remove picture").focusable(true).h(30.0).px(14.0)
                    .radius(15.0).center().fill(PAL.chip).hover(crate::theme::ov(0.06)).cursor(Cursor::Pointer)
                    .on_press(|s: &mut State| s.remove_picture())
                    .child(text("Remove").font(rune::TextStyle::new(13.0, 600.0)).color(PAL.danger_text));
                controls = controls.child(remove);
            }
            setting(narrow, if r.search { "Background picture" } else { "Picture" }, Some(picture), controls)
        }
        Row::Effect => block(
            if r.search { "Background effect" } else { "Effect" },
            ap.effect.description(),
            effect_chips("settings/bg/effect", ap.effect, bg_on, |s, e| s.set_bg_effect(e)),
        ),
        Row::ShowOn => {
            let show_seg = |key: &'static str, label: &'static str, v: ShowOn| {
                segment(key, label, ap.show_on == v).on_press(move |s: &mut State| s.set_show_on(v))
            };
            setting(
                narrow,
                if r.search { "Show background on" } else { "Show on" },
                Some("Card pages only, or every page including song lists.".into()),
                segmented(
                    "settings/bg/show-on",
                    vec![
                        show_seg("settings/bg/show-cards", "Card pages", ShowOn::Cards),
                        show_seg("settings/bg/show-all", "All pages", ShowOn::All),
                    ],
                ),
            )
        }
        Row::CardStrength => setting(
            narrow,
            "Card page strength",
            Some(format!("Home, New, Radio, album grids and artist pages. {}", theme_note())),
            strength("settings/bg/opacity", narrow, ap.opacity_for(is_light()), bg_on, |s, v| s.set_bg_opacity(is_light(), v)),
        ),
        Row::ListStrength => setting(
            narrow,
            "Song list strength",
            Some(format!("Album and playlist pages, Songs and Local Files. {}", theme_note())),
            strength(
                "settings/bg/list-opacity",
                narrow,
                ap.list_opacity_for(is_light()),
                bg_on && ap.show_on == ShowOn::All,
                |s, v| s.set_bg_list_opacity(is_light(), v),
            ),
        ),
        Row::Fade => setting(
            narrow,
            "Fade toward the bottom",
            Some("Keeps long lists on the plain background.".into()),
            switch("settings/bg/fade", "Fade toward the bottom", ap.fade, bg_on, |s, on| s.set_bg_fade(on)),
        ),
        Row::NpBackground => {
            let np_seg = |key: &'static str, label: &'static str, v: NpBg| {
                segment(key, label, ap.np == v).on_press(move |s: &mut State| s.set_np_bg(v))
            };
            setting(
                narrow,
                if r.search { "Now Playing background" } else { "Background" },
                Some("Colour tints the screen with the cover. Same as app uses the picture above.".into()),
                segmented(
                    "settings/np/bg",
                    vec![
                        np_seg("settings/np/tint", "Colour", NpBg::Tint),
                        np_seg("settings/np/cover", "Cover", NpBg::Cover),
                        np_seg("settings/np/app", "Same as app", NpBg::App),
                    ],
                ),
            )
        }
        Row::AnimatedArtwork => setting(
            narrow,
            "Animated artwork",
            Some("Apple's video loop of the album plays in the Now Playing cover, when it has one. A still cover with Reduce Motion.".into()),
            switch("settings/motion", "Animated artwork", s.settings.animated_artwork, true, |s, on| s.set_animated_artwork(on)),
        ),
        Row::NpCoverEffect => block(
            if r.search { "Now Playing cover effect" } else { "Cover effect" },
            ap.np_effect.description(),
            effect_chips("settings/np/effect", ap.np_effect, true, |s, e| s.set_np_effect(e)),
        ),
        Row::Sidebar => {
            const NOTE: &str = "Turn off what you don't use. Right-click an entry in the sidebar to hide it from there.";
            if r.search {
                block("Sidebar", NOTE, sidebar_chips(s))
            } else {
                col().w_full()
                    .child(text("Sidebar").font(CARD_TITLE).color(PAL.text).mt(22.0).mb(2.0))
                    .child(text(NOTE).font(SMALL13).color(PAL.dim).wrap().mb(10.0))
                    .child(sidebar_chips(s).pb(16.0))
            }
        }
        Row::OpenOn => {
            let last = s.settings.start_page == StartPage::Last;
            setting(
                narrow,
                "Open on",
                None,
                segmented(
                    "settings/start",
                    vec![
                        segment("settings/start/home", "Home", !last).on_press(|s: &mut State| s.set_start_page(StartPage::Home)),
                        segment("settings/start/last", "Last page", last).on_press(|s: &mut State| s.set_start_page(StartPage::Last)),
                    ],
                ),
            )
        }
        Row::Tray => setting(
            narrow,
            if MAC { "Show icon in the menu bar" } else { "Show in system tray" },
            Some(if MAC {
                "Play, pause and skip from the menu bar.".into()
            } else {
                "Play, pause and skip from the tray. Needs a tray host (on GNOME, the AppIndicator extension).".into()
            }),
            switch("settings/tray", if MAC { "Show icon in the menu bar" } else { "Show in system tray" }, s.settings.tray_icon, true, |s, on| s.set_tray_icon(on)),
        ),
        Row::CloseToTray => setting(
            narrow,
            if MAC { "Close to the menu bar" } else { "Close to tray" },
            Some(super::keys::hint(
                s,
                if MAC {
                    "Closing the window also hides the Dock icon and keeps the music playing. Quit from the menu bar icon or with the Quit shortcut"
                } else {
                    "Closing the window keeps the music playing. Quit from the tray or with the Quit shortcut"
                },
                super::keys::Action::Quit,
            ) + "."),
            switch("settings/close-tray", if MAC { "Close to the menu bar" } else { "Close to tray" }, s.settings.close_to_tray, s.settings.tray_icon, |s, on| s.set_close_to_tray(on)),
        ),
        Row::Notify => setting(
            narrow,
            "Notify on song change",
            Some("Shows the song and its cover when a new one starts while Uzumaki isn't in front.".to_string()),
            switch("settings/notify", "Notify on song change", s.settings.notify_song_change, true, |s, on| s.set_notify_song_change(on)),
        ),
        Row::CheckUpdates => setting(
            narrow,
            "Check for updates",
            Some("Once a day, asks getuzumaki.app whether a new version is out, and says so. The check sends only the Uzumaki version, the system, the processor type and how Uzumaki was installed; the server keeps a daily count and nothing else. Nothing is downloaded.".to_string()),
            switch("settings/check-updates", "Check for updates", s.settings.check_updates, true, |s, on| s.set_check_updates(on)),
        ),
        Row::CheckNow => {
            let st = &s.update_check;
            let mut note = crate::update::last_checked(st.last_check);
            if let Some(n) = s.update.as_ref().filter(|n| n.required) {
                note = format!("{} {note}.", n.message());
            } else if let Some(result) = &st.result {
                note = format!("{result} {note}.");
            } else {
                note.push('.');
            }
            let button = if st.running {
                small_button("settings/check-now", "Checking\u{2026}").opacity(0.6)
            } else {
                small_button("settings/check-now", "Check now").on_press(|s: &mut State| s.check_updates_now())
            };
            setting(narrow, if r.search { "Check for updates now" } else { "Updates" }, Some(note), button)
        }
        Row::About => setting(
            narrow,
            if r.search { "About Uzumaki" } else { "Uzumaki" },
            Some(format!("Version {} ({}{}). {}. Free software under the GNU General Public License, version 3 or later.", crate::VERSION, crate::BUILD, uzumaki_ctl::dev_suffix(), crate::COPYRIGHT)),
            small_button("settings/website", "getuzumaki.app").on_press_tx(|s: &mut State, tx: &mut Tx| {
                if tx.open_url("https://getuzumaki.app").is_err() {
                    s.toast = Some("Couldn't open the link.".into());
                }
            }),
        ),
        Row::Tour => setting(
            narrow,
            "Welcome tour",
            Some("Plays the welcome and the shortcuts tour again.".to_string()),
            small_button("settings/tour", "Show again").on_press(|s: &mut State| super::onboarding::replay(s)),
        ),
        Row::ArtCache => cache_row(s, narrow, CacheKind::Art, if r.search { "Artwork cache" } else { "Artwork" }, "settings/clear/art"),
        Row::LibraryCache => cache_row(s, narrow, CacheKind::Library, if r.search { "Library cache" } else { "Library" }, "settings/clear/library"),
        Row::MusicFolders => music_folders(s, narrow),
        Row::NumberKeys => number_keys(s, narrow),
        Row::Binding(i) => binding(s, i),
    }
}

/// Account → Music source: Apple Music, or local files only (switching keeps the sign-in and
/// the caches).
fn music_source(s: &State, narrow: bool) -> El {
    let local = s.local_only();
    let desc = if local {
        "Only your music folders. Apple Music's engine doesn't run and its pages are hidden. Your Apple sign-in and library copy are kept for when you switch back."
    } else {
        "Apple Music, with your music folders beside it. Local files only stops Apple Music's engine and hides its pages; your sign-in is kept."
    };
    setting(
        narrow,
        "Music source",
        Some(desc.into()),
        segmented(
            "settings/source",
            vec![
                segment("settings/source/apple", "Apple Music", !local).on_press(|s: &mut State| s.switch_to_apple()),
                segment("settings/source/local", "Local files only", local).on_press(|s: &mut State| {
                    s.set_music_source(MusicSource::LocalOnly);
                }),
            ],
        ),
    )
}

/// Account → Apple Music: the sign-in state, Sign in / Sign out, and the "really sign out?" box.
fn apple_music(s: &State, narrow: bool) -> El {
    let ses = &s.session;
    let status = if !ses.bridge_up {
        "Playback engine starting\u{2026}".to_string()
    } else if ses.authorized {
        // Read-only: the account's store, saved after sign-in.
        match &s.settings.storefront {
            Some(f) => format!("Signed in \u{b7} Store: {}", f.label()),
            None => "Signed in".to_string(),
        }
    } else if ses.login_window {
        "Finish signing in in the Apple Music window".to_string()
    } else {
        "Not signed in".to_string()
    };
    let control = if !ses.bridge_up {
        row()
    } else if ses.signing_out {
        row().child(text("Signing out\u{2026}").font(SMALL13).color(PAL.dim))
    } else if ses.authorized {
        if s.ui.settings.confirm {
            row()
        } else {
            row().child(danger_button("settings/sign-out", "Sign out", false).on_press(|s: &mut State| s.ui.settings.confirm = true))
        }
    } else if ses.login_window {
        row().child(small_button("settings/hide-login", "Hide window").on_press(|s: &mut State| s.show_login(false)))
    } else {
        row().child(small_button("settings/sign-in", "Sign in").on_press(|s: &mut State| s.sign_in()))
    };
    let mut v = col().w_full().child(setting(narrow, "Apple Music", Some(status), control));
    if ses.authorized && s.ui.settings.confirm && !ses.signing_out {
        v = v.child(
            col().key("settings/confirm").w_full().padding(14.0).gap(12.0).radius(10.0).fill(PAL.danger_bg).items_start().mb(8.0)
                .child(
                    text("Signing out removes Uzumaki's copy of your library. You'll need your Apple ID password to sign in again.")
                        .font(SMALL13).color(PAL.danger_text).wrap(),
                )
                .child(
                    row().gap(8.0)
                        .child(small_button("settings/cancel-sign-out", "Cancel").on_press(|s: &mut State| s.ui.settings.confirm = false))
                        .child(danger_button("settings/confirm-sign-out", "Sign out", true).on_press(|s: &mut State| {
                            s.ui.settings.confirm = false;
                            s.sign_out();
                        })),
                ),
        );
    }
    v
}

/// Local Files → Music folders: Add Folder and Rescan, then the folders.
fn music_folders(s: &State, narrow: bool) -> El {
    let rescan = small_button("settings/rescan", "Rescan");
    let rescan = if s.local_lib.scanning {
        rescan.opacity(0.4).cursor(Cursor::NotAllowed)
    } else {
        rescan.on_press(|s: &mut State| s.rescan_local())
    };
    let buttons = row().gap(8.0).child(
        small_button("settings/add-folder", "Add Folder\u{2026}")
            .on_press_tx(|_s: &mut State, tx: &mut rune::Tx| super::local_files::pick_folder(tx)),
    ).child(rescan);
    let mut v = col().w_full()
        .child(setting(narrow, "Music folders", Some("Songs in these folders (and their subfolders) show up under Local Files.".into()), buttons));
    let folders = &s.settings.music_folders;
    if folders.is_empty() {
        v = v.child(text("No folders yet.").font(SMALL13).color(PAL.dim).mb(8.0));
    }
    for (i, f) in folders.iter().enumerate() {
        v = v.child(
            row().key(format!("settings/folder/{i}")).w_full().min_h(40.0).px(12.0).gap(10.0).center_y().radius(8.0).fill(PAL.chip).mb(6.0)
                .child(icon(&icons::LOCAL_FILES, 16.0).color(PAL.dim))
                .child(text(tilde(f)).font(BODY14).color(PAL.text2).truncate().grow(1.0).min_w(0.0))
                .child(
                    el().key(format!("settings/folder-x/{i}")).role("button").label("Remove folder").focusable(true).size(28.0, 28.0)
                        .radius(14.0).center().hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
                        .on_press(move |s: &mut State| s.remove_music_folder(i))
                        .child(icon(&icons::CLOSE, 14.0).color(PAL.muted)),
                ),
        );
    }
    v
}

/// Keyboard → Number keys 1–9: the modifier, and pins or pages.
fn number_keys(s: &State, narrow: bool) -> El {
    use super::keys;
    use crate::settings::{NumberKeys, NumberMod};
    let (nm, nk) = (s.settings.number_mod, s.settings.number_keys);
    setting(
        narrow,
        "Number keys 1–9",
        Some({
            let targets: Vec<String> = (1..=9u8).filter_map(|n| keys::slot_target(s, n).map(|t| format!("{n} {t}"))).collect();
            match (nm, nk) {
                (NumberMod::Off, _) => "Off.".into(),
                (_, NumberKeys::Pins) if targets.is_empty() => {
                    "Your sidebar pins, top to bottom. Pin an album, playlist or artist to use them.".into()
                }
                (_, NumberKeys::Pins) => format!("Your pins: {}", targets.join(" \u{b7} ")),
                (_, NumberKeys::Pages) => targets.join(" \u{b7} "),
            }
        }),
        col().gap(8.0).items_end()
            .child(segmented(
                "settings/numbers/mod",
                vec![
                    segment("settings/numbers/ctrl", "Ctrl", nm == NumberMod::Ctrl).on_press(move |s: &mut State| s.set_number_shortcuts(NumberMod::Ctrl, nk)),
                    segment("settings/numbers/alt", "Alt", nm == NumberMod::Alt).on_press(move |s: &mut State| s.set_number_shortcuts(NumberMod::Alt, nk)),
                    segment("settings/numbers/off", "Off", nm == NumberMod::Off).on_press(move |s: &mut State| s.set_number_shortcuts(NumberMod::Off, nk)),
                ],
            ))
            .child(segmented(
                "settings/numbers/target",
                vec![
                    segment("settings/numbers/pages", "Pages", nk == NumberKeys::Pages).on_press(move |s: &mut State| s.set_number_shortcuts(nm, NumberKeys::Pages)),
                    segment("settings/numbers/pins", "Pins", nk == NumberKeys::Pins).on_press(move |s: &mut State| s.set_number_shortcuts(nm, NumberKeys::Pins)),
                ],
            )),
    )
}

/// Keyboard → one shortcut (`keys::BINDINGS[i]`): click its keys to record new ones, ↺ resets.
fn binding(s: &State, i: usize) -> El {
    use super::keys;
    let b = &keys::BINDINGS[i];
    let recording = s.ui.recording == Some(i);
    let mut keys_el = row().key(format!("kb/{i}/keys")).role("button").label(b.label).focusable(true)
        .gap(4.0).center_y().shrink().px(6.0).py(4.0).radius(7.0)
        .hover(crate::theme::ov(0.05)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| keys::start_recording(s, i));
    if recording {
        keys_el = keys_el.fill(PAL.nav_active)
            .child(text("Press the new keys…").font(rune::TextStyle::new(12.0, 600.0)).color(PAL.accent));
    } else {
        match keys::label(s, i) {
            Some(chord) => {
                for k in chord.split('+') {
                    keys_el = keys_el.child(keycap(k));
                }
            }
            None => keys_el = keys_el.child(text("Not set").font(SMALL13).color(PAL.dim)),
        }
    }
    let mut r = row().key(format!("kb/{i}")).w_full().min_h(36.0).py(2.0).gap(10.0).center_y()
        .child(text(b.label).font(BODY14).color(PAL.text).grow(1.0).min_w(0.0).wrap());
    if keys::is_custom(s, i) && !recording {
        let id = b.action.id();
        r = r.child(
            el().key(format!("kb/{i}/reset")).role("button").label("Reset to default").focusable(true)
                .size(26.0, 26.0).radius(7.0).center().hover(crate::theme::ov(0.06)).cursor(Cursor::Pointer)
                .help(format!("Reset to {}", rune::Chord::parse(b.chord).map(|c| c.label()).unwrap_or_default()))
                .on_press(move |s: &mut State| s.reset_keybind(id))
                .child(text("↺").font(rune::TextStyle::new(15.0, 500.0)).color(PAL.muted)),
        );
    }
    r.child(keys_el)
}

// ---- pages ----

fn account(r: &Rx) -> Vec<El> {
    rows(r, &[Row::MusicSource, Row::AppleMusic])
}

fn playback(r: &Rx) -> Vec<El> {
    rows(r, &[Row::Quality, Row::Lyrics, Row::AutoPlay, Row::Resume])
}

fn appearance(r: &Rx) -> Vec<El> {
    let mut v = vec![el().h(4.0), preview(r.s, r.cx), el().h(8.0)];
    v.extend(rows(
        r,
        &[
            Row::Theme,
            Row::AppIcon,
            Row::Player,
            Row::StartupAnimation,
            Row::Background,
            Row::Picture,
            Row::Effect,
            Row::ShowOn,
            Row::CardStrength,
            Row::ListStrength,
            Row::Fade,
        ],
    ));
    if let Some(e) = r.s.backdrops.error(Slot::App) {
        v.push(text(format!("Couldn't show the background: {e}")).font(SMALL13).color(PAL.danger_text).wrap().mb(8.0));
    }
    v.push(divider());
    v.push(text("Now Playing").font(CARD_TITLE).color(PAL.text).mt(22.0).mb(2.0));
    v.extend(rows(r, &[Row::NpBackground, Row::NpCoverEffect, Row::AnimatedArtwork]));
    v.push(divider());
    v.extend(item(r, Row::Sidebar));
    v
}

fn cache_row(s: &State, narrow: bool, kind: CacheKind, title: &str, key: &'static str) -> El {
    let size = match s.cache_sizes {
        Some(c) => fmt_bytes(c.of(kind)),
        None => "Measuring\u{2026}".into(),
    };
    let control = row().gap(14.0).center_y()
        .child(text(size).font(SMALL13.features("'tnum' 1")).color(PAL.muted))
        .child(small_button(key, "Clear").on_press(move |s: &mut State| s.clear_cache(kind)));
    setting(narrow, title, None, control)
}

fn general(r: &Rx) -> Vec<El> {
    let mut v = rows(r, &[Row::OpenOn, Row::Tray, Row::CloseToTray, Row::Notify, Row::CheckUpdates, Row::CheckNow, Row::Tour]);
    v.push(divider());
    v.push(text("Caches").font(CARD_TITLE).color(PAL.text).mt(22.0).mb(2.0));
    v.extend(rows(r, &[Row::ArtCache, Row::LibraryCache]));
    v.push(text("Cleared files are downloaded again when needed.").font(SMALL13).color(PAL.dim).wrap().mt(4.0));
    v.push(el().h(14.0));
    v.push(divider());
    v.push(text("About").font(CARD_TITLE).color(PAL.text).mt(22.0).mb(2.0));
    v.extend(rows(r, &[Row::About]));
    v
}

fn local(r: &Rx) -> Vec<El> {
    let lib = &r.s.local_lib;
    let status = if lib.scanning {
        format!("Scanning\u{2026} {} files", super::local_files::thousands(lib.scanned))
    } else if lib.songs.is_empty() && !lib.loaded {
        "Not scanned yet".to_string()
    } else {
        format!("{} songs", super::local_files::thousands(lib.songs.len()))
    };
    let mut v: Vec<El> = item(r, Row::MusicFolders).into_iter().collect();
    v.push(divider());
    v.push(text(status).font(BODY14).color(PAL.muted).mt(14.0));
    v.push(
        text("Plays FLAC, MP3, AAC/ALAC (M4A), Ogg Vorbis and WAV. Files stay on this computer.")
            .font(SMALL13).color(PAL.dim).wrap().mt(6.0),
    );
    v
}

/// Settings → Keyboard: the number shortcuts, then every shortcut from `keys::BINDINGS` by
/// group. Click a shortcut to record new keys; ↺ puts back the default.
fn keyboard(r: &Rx) -> Vec<El> {
    use super::keys;
    let mut v = vec![
        text("Click a shortcut, then press the new keys. Esc cancels, Backspace removes it. Shortcuts don't fire while you type in a text field.")
            .font(SMALL13).color(PAL.dim).wrap().pb(4.0),
    ];
    v.extend(item(r, Row::NumberKeys));
    for group in keys::GROUPS {
        v.push(text(group).font(SECTION11).color(PAL.dim).pt(18.0).pb(6.0));
        for (i, _) in keys::BINDINGS.iter().enumerate().filter(|(_, b)| b.group == group) {
            v.extend(item(r, Row::Binding(i)));
        }
        v.push(divider());
    }
    v.pop();
    if !r.s.settings.keybinds.is_empty() {
        v.push(
            row().pt(18.0).child(
                el().key("kb/reset-all").role("button").label("Reset all shortcuts").focusable(true).h(30.0).px(14.0).radius(15.0)
                    .center().fill(PAL.chip).hover(crate::theme::ov(0.06)).cursor(Cursor::Pointer)
                    .on_press(|s: &mut State| s.reset_keybinds())
                    .child(text("Reset all shortcuts").font(rune::TextStyle::new(13.0, 600.0)).color(PAL.text)),
            ),
        );
    }
    v
}

// ---- search ----

/// The field atop the page list. Enter goes to the first result; Esc clears it, then closes.
fn search_box(s: &State) -> El {
    let field = text_input(&s.ui.settings.query, TextStyle::new(13.0, 400.0))
        .key("settings/search").grow(1.0).min_w(0.0).single_line().color(PAL.text).placeholder("Search", PAL.dim)
        .on_edit(|s: &mut State| &mut s.ui.settings.query)
        .on_input_change(|s: &mut State, _tx: &mut Tx| s.ui.settings.hits = search::find(&s.ui.settings.query.text))
        .on_submit(jump_first)
        // The field would take Esc for itself (it only drops focus).
        .on_input_key(|s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
            if super::keys::track_focus(s, ev, tx) {
                return true; // the marker after a Tab (`keys::track_focus`)
            }
            if ev.key != Key::Escape {
                return false;
            }
            if s.ui.settings.query.text.is_empty() {
                close(s);
                tx.blur();
            } else {
                clear_search(s);
            }
            true
        });
    let field = super::caret::ink(field, s);
    let mut b = row().key("settings/search-box").role("textbox").w_full().h(30.0).pl(9.0).pr(5.0).gap(7.0).center_y().radius(8.0)
        .fill(PAL.field).border(1.0, PAL.field_border).cursor(Cursor::Text)
        .child(icon(&icons::SEARCH, 13.0).color(PAL.dim))
        .child(field);
    if !s.ui.settings.query.text.is_empty() {
        b = b.child(
            el().key("settings/search-clear").role("button").label("Clear search").size(20.0, 20.0).radius(10.0).center()
                .hover(crate::theme::ov(0.1)).cursor(Cursor::Pointer)
                .on_press(clear_search)
                .child(icon(&icons::CLOSE, 10.0).color(PAL.dim)),
        );
    }
    b
}

/// A page's name over its rows in the results; a press opens the page at its first one.
fn group_head(tab: Tab, first: Row, top: bool) -> El {
    let (key, title, ic) = tab.info();
    row().key(key.replace("/tab/", "/group/")).role("button").label(format!("Go to {title}")).focusable(true)
        .mt(if top { 2.0 } else { 18.0 }).ml(-8.0).px(8.0).py(5.0).gap(7.0).center_y().radius(7.0)
        .hover(crate::theme::ov(0.05)).press(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
        .on_press_tx(move |s: &mut State, tx: &mut Tx| jump(s, tab, first, tx))
        .child(icon(ic, 13.0).color(PAL.accent))
        .child(text(title.to_uppercase()).font(SECTION11).color(PAL.dim))
        .child(text("\u{203a}").font(rune::TextStyle::new(13.0, 500.0)).color(PAL.dim))
}

/// The rows the search finds, by page, or a note when there are none.
fn search_results(r: &Rx) -> Vec<El> {
    let found = results(r.s);
    if found.is_empty() {
        let q = r.s.ui.settings.query.text.trim();
        return vec![
            col().key("settings/no-results").w_full().pt(72.0).gap(6.0).items_center()
                .child(text(format!("No settings match \u{201c}{q}\u{201d}")).font(BODY14).color(PAL.text2).wrap())
                .child(text("Try \u{201c}theme\u{201d}, \u{201c}lyrics\u{201d} or \u{201c}shortcuts\u{201d}.").font(SMALL13).color(PAL.dim)),
        ];
    }
    let mut v = Vec::new();
    let mut last = None;
    for (tab, row) in found {
        if last != Some(tab) {
            v.push(group_head(tab, row, last.is_none()));
            last = Some(tab);
        } else {
            v.push(divider());
        }
        v.extend(item(r, row));
    }
    v
}

fn tab_row(tab: Tab, active: bool) -> El {
    let (key, label, ic) = tab.info();
    let (bg, fg) = if active { (PAL.nav_active, PAL.text) } else { (rune::Color::TRANSPARENT, PAL.nav_text) };
    row().key(key).role("button").label(label).focusable(true).w_full().h(34.0).px(10.0).gap(11.0).center_y().radius(8.0)
        .fill(bg).hover(crate::theme::ov(0.05)).press(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| select(s, tab))
        .child(icon(ic, 17.0).color(PAL.accent))
        .child(text(label).font(NAV).color(fg))
}

/// The overlay, or `None` while closed. `win` is the window size in points.
pub fn overlay(s: &State, cx: &Cx<State>, win: [f32; 2]) -> Option<El> {
    // In local only mode Playback holds only Resume (its other rows are Apple Music's).
    let tab = s.ui.settings.open?;
    let w = 760.0_f32.min(win[0] - 48.0).max(320.0);
    let h = 540.0_f32.min(win[1] - 48.0).max(240.0);
    let narrow = w < 700.0;
    let searching = searching(s);
    let r = Rx { s, cx, narrow, search: searching };

    let (title, items) = if searching {
        ("Search", search_results(&r))
    } else {
        let items = match tab {
            Tab::Account => account(&r),
            Tab::Playback => playback(&r),
            Tab::Appearance => appearance(&r),
            Tab::General => general(&r),
            Tab::Local => local(&r),
            Tab::Keyboard => keyboard(&r),
        };
        (tab.title(), items)
    };

    let mut side = col().w(200.0).h_full().fill(PAL.sidebar).px(10.0).pt(24.0).gap(2.0)
        .child(text("Settings").font(H2).color(PAL.text).px(10.0).pb(12.0))
        .child(search_box(s).mb(10.0));
    for t in Tab::ALL {
        side = side.child(tab_row(t, !searching && t == tab));
    }

    let head = row().w_full().px(28.0).pt(22.0).pb(8.0).center_y()
        .child(text(title).font(H2).color(PAL.text).grow(1.0))
        .child(
            el().key("settings/close").role("button").label("Close settings").focusable(true).size(30.0, 30.0).radius(15.0).center()
                .hover(crate::theme::ov(0.08)).cursor(Cursor::Pointer)
                .on_press(|s: &mut State| close(s))
                .child(icon(&icons::CLOSE, 15.0).color(PAL.muted)),
        );
    let body = col().key(format!("settings/body/{title}")).w_full().grow(1.0).min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(col().w_full().px(28.0).pb(24.0).children(items));
    let main = col().grow(1.0).min_w(0.0).h_full().child(head).child(body);

    let panel = row().key("settings/panel").absolute((win[0] - w) / 2.0, (win[1] - h) / 2.0).size(w, h).radius(14.0)
        .clip_rounded(14.0).fill(PAL.player).border(1.0, PAL.field_border).shadow(12.0, 40.0, crate::theme::shadow(0.5))
        // Presses on the panel's empty space stop here instead of reaching the scrim (which closes).
        .on_press(|_: &mut State| {})
        .child(side)
        .child(el().w(1.0).h_full().fill(PAL.divider))
        .child(main);

    Some(
        el().key("settings-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .child(
                el().key("settings/scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(crate::theme::scrim(0.55))
                    .on_press(|s: &mut State| close(s)),
            )
            .child(panel),
    )
}

/// Scenes and tests: put `q` in the search field, as typing it would.
pub fn set_query(s: &mut State, q: &str) {
    s.ui.settings.query = TextEdit::new(q);
    s.ui.settings.hits = search::find(q);
}

/// A row of effect chips (they wrap), dimmed and inert while `enabled` is false.
fn effect_chips(prefix: &'static str, current: Effect, enabled: bool, set: fn(&mut State, Effect)) -> El {
    let mut r = row().key(prefix).w_full().flex_wrap().gap(8.0);
    for e in Effect::ALL {
        let c = chip(format!("{prefix}/{}", e.label()), e.label(), enabled && e == current);
        r = r.child(if enabled { c.on_press(move |s: &mut State| set(s, e)) } else { c.cursor(Cursor::NotAllowed) });
    }
    if enabled { r } else { r.opacity(0.4) }
}

/// True while the Appearance preview tile has something to draw (so `ui::view` knows when to free
/// the slot).
pub fn preview_active(s: &State) -> bool {
    s.ui.settings.open == Some(Tab::Appearance)
        && super::backdrop::themed_app_plan(&s.settings.appearance, s.playback.song.as_ref()).is_some()
}

/// The tray settings read differently on the Mac (menu bar, Dock icon).
const MAC: bool = cfg!(target_os = "macos");

const PREVIEW_KEY: &str = "settings/bg/preview";

/// The preview tile: the background at the card-page strength over the plain background, drawn
/// through `Slot::Preview` at the tile's own size.
fn preview(s: &State, cx: &Cx<State>) -> El {
    let ap = &s.settings.appearance;
    let tile = el().key(PREVIEW_KEY).w_full().h(150.0).radius(12.0).clip_rounded(12.0).fill(PAL.bg)
        .border(1.0, PAL.field_border);
    let full = || el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    let Some(plan) = super::backdrop::themed_app_plan(ap, s.playback.song.as_ref()) else {
        let msg = match ap.source {
            BgSource::Off => "No background",
            BgSource::Picture => "No picture chosen",
            _ => "Shows the playing song's cover",
        };
        return tile.center().child(text(msg).font(SMALL13).color(PAL.muted));
    };
    // The first frame has no layout yet: just the empty tile.
    let Some(size) = cx.layout_size(PREVIEW_KEY) else { return tile };
    let pct = (plan.opacity * 100.0).round();
    let req = Request {
        source: plan.source,
        effect: plan.effect,
        ground: super::backdrop::ground(),
        target: super::backdrop::target(size, cx.env().scale),
    };
    let tile = match s.backdrops.show(Slot::Preview, &req) {
        Some(shown) => tile.child(full().key("preview-bg").child(super::backdrop::picture(&shown, plan.opacity))),
        None => tile,
    };
    tile.child(
        el().pin(Some(12.0), None, None, Some(10.0)).px(8.0).py(3.0).radius(6.0).fill(rune::Color::BLACK.alpha(0.45))
            .child(text(format!("Preview at {pct}%")).font(SMALL13.features("'tnum' 1")).color(if is_light() { rune::Color::WHITE } else { PAL.text2 })),
    )
}

/// Under the strength sliders: the light and dark themes each keep their own strengths, and the
/// sliders show the one in force.
fn theme_note() -> &'static str {
    if is_light() { "For the light theme (dark has its own)." } else { "For the dark theme (light has its own)." }
}

/// A strength slider (0..=1 of the opacity range of the theme in force) with its percentage,
/// dimmed and inert unless `enabled`. Moves apply live; the release saves.
fn strength(key: &'static str, narrow: bool, value: f32, enabled: bool, set: fn(&mut State, f32)) -> El {
    let (min, max) = opacity_range(is_light());
    let frac = (value - min) / (max - min);
    let r = row().w(if narrow { 260.0 } else { 220.0 }).gap(12.0).center_y()
        .child(el().grow(1.0).child(slider(key, frac, PAL.accent, move |s, f, phase, cancelled, _tx| {
            if !enabled {
                return;
            }
            if !(phase == rune::Phase::Up && cancelled) {
                set(s, min + f * (max - min));
            }
            if phase == rune::Phase::Up {
                s.save_appearance();
            }
        })))
        .child(text(format!("{}%", (value * 100.0).round())).font(SMALL13.features("'tnum' 1")).color(PAL.muted).w(36.0));
    if enabled { r } else { r.opacity(0.4).cursor(Cursor::NotAllowed).disabled(true) }
}

/// A titled block with an optional muted description, then full-width content (effect chips).
fn block(title: &str, desc: &str, content: El) -> El {
    col().w_full().py(16.0).gap(12.0)
        .child(col().gap(4.0).child(text(title.to_string()).font(BODY14).color(PAL.text))
            .child(text(desc.to_string()).font(SMALL13).color(PAL.dim).wrap()))
        .child(content)
}


/// A path for display, with the home folder as `~`.
pub fn tilde(path: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        if let Ok(rest) = path.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
        }
    }
    path.display().to_string()
}


/// One key of a chord, drawn as a small keycap.
fn keycap(k: &str) -> El {
    el().h(22.0).min_w(22.0).px(7.0).radius(5.0).center().fill(PAL.chip).border(1.0, PAL.field_border)
        .child(text(k.to_string()).font(rune::TextStyle::new(12.0, 500.0)).color(PAL.text2))
}


/// Settings → Appearance → Sidebar: one chip per entry; a lit chip shows in the sidebar.
fn sidebar_chips(s: &State) -> El {
    let mut chips = row().key("settings/sidebar").w_full().gap(8.0).flex_wrap();
    // Local only mode: the Apple entries are hidden anyway (their choices are kept).
    let local = s.local_only();
    for (id, label) in super::sidebar::nav_choices(local) {
        let on = !s.nav_hidden(id);
        let (bg, fg) = if on { (PAL.nav_active, PAL.text) } else { (rune::Color::TRANSPARENT, PAL.dim) };
        chips = chips.child(
            row().key(format!("settings/sidebar/{id}")).role("switch").label(label).focusable(true).h(30.0).px(12.0).gap(6.0)
                .center_y().radius(15.0).fill(bg).border(1.0, PAL.chip_hover).hover(crate::theme::ov(0.05))
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut State| s.set_nav_hidden(id, on))
                .child(text(if on { "\u{2713}" } else { "+" }).font(rune::TextStyle::new(12.0, 600.0)).color(if on { PAL.accent } else { PAL.dim }))
                .child(text(label).font(rune::TextStyle::new(13.0, 500.0)).color(fg)),
        );
    }
    chips
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Key, KeyEvent, Runtime};

    #[test]
    fn bytes() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(820 * 1024), "820 KB");
        assert_eq!(fmt_bytes(3_565_158), "3.4 MB");
        assert_eq!(fmt_bytes(1_288_490_189), "1.2 GB");
    }

    /// Settings open on Account over the home scene, in a headless window with the app's keys.
    fn window() -> Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "settings-account").unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(crate::ui::on_key);
        rt.poll = Some(crate::ui::poll);
        settle(&mut rt, 30);
        rt
    }

    fn settle(rt: &mut Runtime<State>, frames: usize) {
        rt.frame(0.0);
        for _ in 0..frames {
            rt.frame(1.0 / 60.0);
        }
    }

    fn press(rt: &mut Runtime<State>, key: Key) {
        rt.key(KeyEvent { key, mods: Default::default() });
        settle(rt, 30);
    }

    fn type_text(rt: &mut Runtime<State>, t: &str) {
        for c in t.chars() {
            rt.key(KeyEvent { key: Key::Text(c.to_string()), mods: Default::default() });
            rt.frame(1.0 / 60.0);
        }
        settle(rt, 10);
    }

    fn focus_field(rt: &mut Runtime<State>) {
        rt.update(|s, _| focus_search(s));
        settle(rt, 5);
    }

    fn click(rt: &mut Runtime<State>, key: &str) {
        let r = rt.presented(key).unwrap_or_else(|| panic!("{key} on screen")).0;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt, 30);
    }

    #[test]
    fn typing_shows_the_real_rows_by_page() {
        let mut rt = window();
        assert!(rt.presented("settings/search").is_some());
        assert!(rt.presented(&Row::MusicSource.key()).is_some(), "an empty query shows the page");
        focus_field(&mut rt);
        type_text(&mut rt, "Lyrics");
        assert!(searching(&rt.state));
        assert!(rt.presented(&Row::Lyrics.key()).is_some(), "Playback's row");
        assert!(rt.presented("settings/lyrics/words").is_some(), "with its working control");
        assert!(rt.presented("settings/group/playback").is_some());
        assert!(rt.presented("settings/group/keyboard").is_some(), "shortcuts too");
        assert!(rt.presented(&Row::MusicSource.key()).is_none(), "the page gave way");
        // The control works from the results.
        let before = rt.state.settings.lyrics_words;
        click(&mut rt, if before { "settings/lyrics/lines" } else { "settings/lyrics/words" });
        assert_eq!(rt.state.settings.lyrics_words, !before);
        assert!(searching(&rt.state), "still searching");
    }

    #[test]
    fn no_results_says_so() {
        let mut rt = window();
        rt.update(|s, _| set_query(s, "xylophone"));
        settle(&mut rt, 10);
        assert!(rt.presented("settings/no-results").is_some());
    }

    #[test]
    fn esc_clears_the_search_then_closes() {
        // From the focused field.
        let mut rt = window();
        focus_field(&mut rt);
        type_text(&mut rt, "tray");
        press(&mut rt, Key::Escape);
        assert!(rt.state.ui.settings.query.text.is_empty(), "the first Esc clears");
        assert!(rt.state.ui.settings.open.is_some(), "Settings stays open");
        assert!(rt.presented(&Row::MusicSource.key()).is_some(), "back to the page");
        press(&mut rt, Key::Escape);
        assert!(rt.state.ui.settings.open.is_none(), "the second Esc closes");

        // With the focus elsewhere (a control in the results was clicked).
        let mut rt = window();
        rt.update(|s, _| set_query(s, "tray"));
        settle(&mut rt, 10);
        click(&mut rt, "settings/tray");
        press(&mut rt, Key::Escape);
        assert!(rt.state.ui.settings.query.text.is_empty());
        assert!(rt.state.ui.settings.open.is_some());
        press(&mut rt, Key::Escape);
        assert!(rt.state.ui.settings.open.is_none());
    }

    #[test]
    fn enter_opens_the_first_result_and_flashes_it() {
        let mut rt = window();
        focus_field(&mut rt);
        type_text(&mut rt, "fade");
        let first = results(&rt.state)[0];
        assert_eq!(first, (Tab::Appearance, Row::Fade));
        rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
        settle(&mut rt, 40);
        let st = &rt.state.ui.settings;
        assert_eq!(st.open, Some(Tab::Appearance));
        assert!(st.query.text.is_empty(), "the search is over");
        assert_eq!(st.flash, Some(Row::Fade));
        // Scrolled to it: the row sits inside the page's viewport (it starts well below it).
        let row = rt.presented(&Row::Fade.key()).expect("fade row").0;
        let body = rt.presented("settings/body/Appearance").expect("body").0;
        assert!(row[1] >= body[1] - 0.5 && row[1] + row[3] <= body[1] + body[3] + 0.5, "{row:?} in {body:?}");
        // The tint goes after a moment.
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.ui.settings.flash, None);
    }

    #[test]
    fn a_page_header_opens_that_page() {
        let mut rt = window();
        rt.update(|s, _| set_query(s, "tray"));
        settle(&mut rt, 10);
        click(&mut rt, "settings/group/general");
        let st = &rt.state.ui.settings;
        assert_eq!(st.open, Some(Tab::General));
        assert!(st.query.text.is_empty());
        assert_eq!(st.flash, Some(Row::Tray), "its first match");
    }

    #[test]
    fn rows_missing_in_local_only_mode_dont_show() {
        let mut s = State::offline();
        s.settings.music_source = MusicSource::LocalOnly;
        set_query(&mut s, "lyrics");
        assert!(results(&s).iter().all(|(t, _)| *t == Tab::Keyboard), "{:?}", results(&s));
    }

    #[test]
    fn closing_forgets_the_search() {
        let mut s = State::offline();
        open(&mut s);
        set_query(&mut s, "tray");
        close(&mut s);
        open(&mut s);
        assert!(!searching(&s) && s.ui.settings.hits.is_empty());
    }
}
