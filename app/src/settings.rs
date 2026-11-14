//! User settings that persist across launches: sidebar pins, layout choices, playback and
//! window preferences (the Settings overlay, Ctrl+,).
//! Stored as JSON in `$XDG_CONFIG_HOME/uzumaki/settings.json` (else `~/.config/uzumaki/`).
//! Unknown or missing fields fall back to defaults, so old files keep loading.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::models::Artwork;
use crate::state::{Repeat, Route};

/// How the player bar sits at the bottom of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerStyle {
    /// Full width, edge to edge.
    #[default]
    Docked,
    /// A rounded capsule floating above the bottom edge, like Apple Music's.
    Floating,
}

/// Where the music comes from (chosen in the first-run onboarding; Settings → Account later).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MusicSource {
    /// Apple Music through the bridge; music folders still work.
    #[default]
    Apple,
    /// Music folders only: no Apple account. The Apple pages are hidden and Electron never
    /// starts.
    LocalOnly,
}

/// The Apple Music storefront (country store) of the signed-in account, read from MusicKit after
/// the guided sign-in. Shown read-only in Settings → Account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storefront {
    /// Apple's id, the country code: "it".
    pub id: String,
    /// Apple's name for it ("Italy"), once looked up.
    #[serde(default)]
    pub name: Option<String>,
}

impl Storefront {
    /// "Italy", or "IT" until the name is known.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.to_uppercase())
    }
}

/// The colour theme (Settings → Appearance).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Dark,
    Light,
    /// Follows the computer's appearance (`Cx::system_dark`).
    #[default]
    System,
}

impl Theme {
    /// Whether the light palette is in force, given the OS appearance.
    pub fn is_light(self, system_dark: bool) -> bool {
        match self {
            Theme::Dark => false,
            Theme::Light => true,
            Theme::System => !system_dark,
        }
    }
}

/// Which app icon the desktop shows (Settings → Appearance → App icon): the dock or taskbar, the
/// menu and the tray on Linux; the Dock, the Finder and the menu bar on macOS. This is the stored
/// choice; `resolve` turns it into the icon actually shown (`IconVariant`). The sidebar logo
/// follows the theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppIcon {
    /// The coral swirl on a dark tile: the icon `hicolor/` holds and `install.sh` installs.
    #[default]
    Dark,
    /// The swirl on a white tile.
    Light,
    /// A pale coral tile with a deep coral swirl; sits well on both light and dark docks.
    Coral,
    /// Follow theme: the Dark icon with the dark theme, the Light one with the light theme
    /// (with Theme = System, the computer's appearance).
    #[serde(alias = "auto")]
    Auto,
}

impl AppIcon {
    /// The Settings tiles, in order.
    pub const ALL: [AppIcon; 4] = [AppIcon::Dark, AppIcon::Light, AppIcon::Coral, AppIcon::Auto];

    pub fn label(self) -> &'static str {
        match self {
            AppIcon::Dark => "Dark",
            AppIcon::Light => "Light",
            AppIcon::Coral => "Coral",
            AppIcon::Auto => "Follow theme",
        }
    }

    /// The icon to show, given the theme and the OS appearance (`None` until the first frame
    /// reports it). `None` only when that is still unknown and the choice depends on it.
    pub fn resolve(self, theme: Theme, system_dark: Option<bool>) -> Option<IconVariant> {
        match self {
            AppIcon::Dark => Some(IconVariant::Dark),
            AppIcon::Light => Some(IconVariant::Light),
            AppIcon::Coral => Some(IconVariant::Coral),
            AppIcon::Auto => {
                let light = match theme {
                    Theme::Dark => false,
                    Theme::Light => true,
                    Theme::System => !system_dark?,
                };
                Some(if light { IconVariant::Light } else { IconVariant::Dark })
            }
        }
    }
}

/// An icon the desktop can actually show: one set of files under `assets/icons/variants/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconVariant {
    Dark,
    Light,
    Coral,
}

impl IconVariant {
    pub const ALL: [IconVariant; 3] = [IconVariant::Dark, IconVariant::Light, IconVariant::Coral];

    /// The folder name under `assets/icons/variants/` (and `share/uzumaki/icons/`).
    pub fn dir(self) -> &'static str {
        match self {
            IconVariant::Dark => "dark",
            IconVariant::Light => "light",
            IconVariant::Coral => "coral",
        }
    }
}

/// The modifier of the number shortcuts (1–9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberMod {
    #[default]
    Ctrl,
    Alt,
    /// No number shortcuts.
    Off,
}

/// What the number shortcuts open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberKeys {
    /// Home, Albums, Songs, … (`ui::keys::PAGES`).
    #[default]
    Pages,
    /// The sidebar's pins, top to bottom.
    Pins,
}

/// What the right sidebar (Ctrl+Shift+B) shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RightTab {
    #[default]
    Lyrics,
    Queue,
}

/// Which page the app opens on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartPage {
    #[default]
    Home,
    /// The page that was showing when the app last ran (`Settings::last_route`).
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinKind {
    Album,
    Playlist,
    Artist,
    Station,
}

/// Something pinned to the sidebar's Pins group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub kind: PinKind,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artwork: Option<Artwork>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub pins: Vec<Pin>,
    /// Sidebar shrunk to an icon rail (Ctrl+B).
    pub sidebar_collapsed: bool,
    /// The Pins group is expanded.
    pub pins_open: bool,
    /// Full-screen Now Playing shows lyrics beside the cover (else the cover alone).
    pub now_playing_lyrics: bool,
    /// Full-screen Now Playing shows Up Next in that column instead (over `now_playing_lyrics`).
    pub now_playing_queue: bool,
    /// Word-by-word highlight when Apple has word timing (else whole lines light up).
    pub lyrics_words: bool,
    /// Stream at 64 kbps instead of 256 kbps AAC.
    pub data_saver: bool,
    pub start_page: StartPage,
    /// The page showing last, kept while `start_page` is `Last`.
    pub last_route: Option<Route>,
    /// Show the icon in the system tray.
    pub tray_icon: bool,
    /// Closing the window keeps the app (and music) running in the tray. Needs `tray_icon`.
    pub close_to_tray: bool,
    /// Shuffle and repeat, as last set in the player bar.
    pub shuffle: bool,
    pub repeat: Repeat,
    /// Folders scanned for local files (Settings → Local files), absolute.
    pub music_folders: Vec<PathBuf>,
    /// The right sidebar is showing (Ctrl+Shift+B), and on which tab.
    pub right_panel: bool,
    pub right_tab: RightTab,
    /// Changed shortcuts: action id (`ui::keys::Action::id`) → chord in Rune syntax ("alt+1");
    /// "" means no shortcut. Actions not listed keep their default.
    pub keybinds: BTreeMap<String, String>,
    /// The number shortcuts: modifier, and whether they open pages or pins.
    pub number_mod: NumberMod,
    pub number_keys: NumberKeys,
    /// Songs-table choices (view as Songs, sorts), remembered.
    pub table: crate::ui::TableView,
    /// Sidebar entries the user hid (Settings → Appearance, or right-click → Hide): their ids,
    /// see `ui::sidebar::NAV_IDS`.
    pub hidden_nav: Vec<String>,
    /// Docked or floating player bar (Settings → Appearance).
    pub player_style: PlayerStyle,
    /// Dark, light, or follow the system (Settings → Appearance).
    pub theme: Theme,
    /// The app icon on the desktop (Settings → Appearance → App icon).
    pub app_icon: AppIcon,
    /// Background picture, effect and Now Playing background (Settings → Appearance).
    pub appearance: crate::appearance::Appearance,
    /// The swirl that twists open at launch (Settings → Appearance; `ui::intro`). The first
    /// library sync shows its screen either way.
    pub startup_animation: bool,
    /// Apple's video loop of the album in the full-screen Now Playing cover, when it has one
    /// (Settings → Appearance; `motion`). Never with Reduce Motion.
    pub animated_artwork: bool,
    /// The same loop in place of the cover at the top of an album page (Settings → Appearance,
    /// under Animated artwork; `motion::want_page`). Only counts while `animated_artwork` is on.
    pub album_page_motion: bool,
    /// A desktop notification (title, artist and album, cover) when a new song starts while
    /// the window isn't in front (Settings → General; `notify.rs`). Off by default.
    pub notify_song_change: bool,
    /// Once per UTC day, ask updates.getuzumaki.app for the latest version (sending the version,
    /// OS, architecture and install channel, which the server counts) and show a notice when a
    /// newer one is out (Settings → General; `update.rs`). On by default; off sends nothing.
    pub check_updates: bool,
    /// Resume where you left off: the last song, its position and the queue come back paused
    /// at the next start (Settings → Playback; `resume.rs`). On by default.
    pub resume_playback: bool,
    /// AutoPlay: when the queue ends, similar songs keep playing (Settings → Playback, the ∞
    /// button in Up Next; `state/autoplay.rs`). On by default, as in Apple Music.
    pub autoplay: bool,
    /// Pictures the user chose for playlists, by playlist id: shown only in Uzumaki (Apple's
    /// web API can't change a playlist's artwork). `file://` artwork in `playlist_art_dir`.
    pub playlist_art: BTreeMap<String, Artwork>,
    /// Apple Music, or local files only (`ui::onboarding`'s Choice step).
    pub music_source: MusicSource,
    /// The first-run onboarding (`ui::onboarding`): `None` until it is finished or skipped, then
    /// the tour version it ran. Settings files written before it existed load as `Some(0)`
    /// (`Settings::from_json`), so existing users never see it.
    pub onboarded: Option<u32>,
    /// The signed-in account's storefront; `None` signed out or not known yet.
    pub storefront: Option<Storefront>,
}

/// `$XDG_DATA_HOME/uzumaki/playlist-art`, else `~/.local/share/uzumaki/playlist-art`: kept
/// apart from the caches, which Settings can clear.
pub fn playlist_art_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("uzumaki").join("playlist-art")
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pins: Vec::new(),
            sidebar_collapsed: false,
            pins_open: true,
            now_playing_lyrics: true,
            now_playing_queue: false,
            lyrics_words: true,
            data_saver: false,
            start_page: StartPage::Home,
            last_route: None,
            tray_icon: true,
            close_to_tray: false,
            shuffle: false,
            repeat: Repeat::Off,
            music_folders: Vec::new(),
            right_panel: false,
            right_tab: RightTab::Lyrics,
            keybinds: BTreeMap::new(),
            number_mod: NumberMod::Ctrl,
            number_keys: NumberKeys::Pages,
            table: crate::ui::TableView::default(),
            hidden_nav: Vec::new(),
            player_style: PlayerStyle::Docked,
            theme: Theme::System,
            app_icon: AppIcon::Dark,
            appearance: Default::default(),
            startup_animation: true,
            animated_artwork: true,
            album_page_motion: true,
            notify_song_change: false,
            check_updates: true,
            resume_playback: true,
            autoplay: true,
            playlist_art: BTreeMap::new(),
            music_source: MusicSource::Apple,
            onboarded: None,
            storefront: None,
        }
    }
}

/// Saves run on background threads; this keeps them in order (and writing one at a time).
static SAVED_GEN: Mutex<u64> = Mutex::new(0);
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

/// `$XDG_CONFIG_HOME/uzumaki` (else `~/.config/uzumaki`), on macOS too: settings and hooks live here.
pub(crate) fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("uzumaki"))
}

fn path() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.json"))
}

impl Settings {
    /// Reads the file; defaults when it is missing or unreadable.
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read(p).ok()).map(|b| Self::from_json(&b)).unwrap_or_default()
    }

    /// Parses a settings file; defaults when it is unreadable. A file with no `onboarded` key was
    /// written before onboarding existed: that is an existing install, so it counts as done
    /// (`Some(0)`, written back on the next save).
    pub fn from_json(bytes: &[u8]) -> Self {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else { return Self::default() };
        let old = value.as_object().is_some_and(|o| !o.contains_key("onboarded"));
        let mut s: Self = serde_json::from_value(value).unwrap_or_default();
        if old && s.onboarded.is_none() {
            s.onboarded = Some(0);
        }
        s
    }

    /// Writes atomically on a background thread. Saves never interleave, and a save that
    /// lost the race to a newer one is dropped (the file never goes back to an older copy).
    pub fn save(&self) {
        let Some(path) = path() else { return };
        let Ok(json) = serde_json::to_vec_pretty(self) else { return };
        let generation = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            let mut saved = SAVED_GEN.lock().unwrap_or_else(|e| e.into_inner());
            if generation < *saved {
                return;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                *saved = generation;
            }
        });
    }

    /// Whether closing the window should hide it to the tray instead of quitting.
    pub fn closes_to_tray(&self) -> bool {
        self.tray_icon && self.close_to_tray
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pins.iter().any(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_defaults_to_system_and_resolves() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.theme, Theme::System, "old files follow the system");
        assert!(Theme::System.is_light(false) && !Theme::System.is_light(true));
        assert!(Theme::Light.is_light(true) && !Theme::Dark.is_light(false));
        let back: Settings = serde_json::from_str(&serde_json::to_string(&Settings { theme: Theme::Light, ..s }).unwrap()).unwrap();
        assert_eq!(back.theme, Theme::Light);
    }

    #[test]
    fn app_icon_defaults_to_dark_and_round_trips() {
        assert_eq!(Settings::default().app_icon, AppIcon::Dark);
        let old: Settings = serde_json::from_str(r#"{"theme":"Light"}"#).unwrap();
        assert_eq!(old.app_icon, AppIcon::Dark, "old files keep today's icon");
        for icon in AppIcon::ALL {
            let s = Settings { app_icon: icon, ..Settings::default() };
            let back: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
            assert_eq!(back.app_icon, icon);
        }
        let coral: Settings = serde_json::from_str(r#"{"app_icon":"Coral"}"#).unwrap();
        assert_eq!(coral.app_icon, AppIcon::Coral);
        // Follow theme is stored as the choice itself, never as the icon it resolved to.
        let auto = Settings { app_icon: AppIcon::Auto, ..Settings::default() };
        let json = serde_json::to_string(&auto).unwrap();
        assert!(json.contains(r#""app_icon":"Auto""#), "{json}");
        for text in [r#"{"app_icon":"Auto"}"#, r#"{"app_icon":"auto"}"#] {
            assert_eq!(serde_json::from_str::<Settings>(text).unwrap().app_icon, AppIcon::Auto);
        }
    }

    #[test]
    fn follow_theme_resolves_to_the_dark_or_light_icon() {
        use super::IconVariant as V;
        let r = |i: AppIcon, t: Theme, sys: Option<bool>| i.resolve(t, sys);
        // Fixed choices ignore the theme and need no appearance.
        for t in [Theme::Dark, Theme::Light, Theme::System] {
            for sys in [None, Some(true), Some(false)] {
                assert_eq!(r(AppIcon::Dark, t, sys), Some(V::Dark));
                assert_eq!(r(AppIcon::Light, t, sys), Some(V::Light));
                assert_eq!(r(AppIcon::Coral, t, sys), Some(V::Coral));
            }
        }
        // A forced theme decides alone.
        for sys in [None, Some(true), Some(false)] {
            assert_eq!(r(AppIcon::Auto, Theme::Dark, sys), Some(V::Dark));
            assert_eq!(r(AppIcon::Auto, Theme::Light, sys), Some(V::Light));
        }
        // Theme = System follows the OS, and waits until it is known.
        assert_eq!(r(AppIcon::Auto, Theme::System, Some(true)), Some(V::Dark));
        assert_eq!(r(AppIcon::Auto, Theme::System, Some(false)), Some(V::Light));
        assert_eq!(r(AppIcon::Auto, Theme::System, None), None);
    }

    #[test]
    fn song_notifications_are_off_by_default_and_old_files_load() {
        assert!(!Settings::default().notify_song_change);
        let s: Settings = serde_json::from_str(r#"{"tray_icon":false}"#).unwrap();
        assert!(!s.notify_song_change);
        let on: Settings = serde_json::from_str(r#"{"notify_song_change":true}"#).unwrap();
        assert!(on.notify_song_change);
    }

    #[test]
    fn old_or_partial_files_still_load() {
        let s: Settings = serde_json::from_str(r#"{"pins":[{"kind":"Playlist","id":"p.1","title":"Mix"}]}"#).unwrap();
        assert_eq!(s.pins.len(), 1);
        assert!(s.pins_open, "missing fields take defaults");
        assert!(s.now_playing_lyrics && !s.now_playing_queue);
        assert!(s.is_pinned("p.1"));
        assert!(!s.data_saver && s.tray_icon && !s.close_to_tray);
        assert_eq!(s.start_page, StartPage::Home);
    }

    #[test]
    fn onboarding_fields_load_and_round_trip() {
        // No file: a new install.
        let fresh = Settings::default();
        assert_eq!(fresh.onboarded, None);
        assert_eq!(fresh.music_source, MusicSource::Apple);
        // A file from before onboarding existed: an existing install, never shown it.
        let old = Settings::from_json(br#"{"tray_icon":false,"pins":[]}"#);
        assert_eq!(old.onboarded, Some(0));
        assert_eq!(old.music_source, MusicSource::Apple);
        assert!(!old.tray_icon);
        // A file with the key keeps it, whatever it says.
        assert_eq!(Settings::from_json(br#"{"onboarded":null}"#).onboarded, None);
        assert_eq!(Settings::from_json(br#"{"onboarded":3}"#).onboarded, Some(3));
        // A broken file gives defaults (the launch rule's cache checks cover that machine).
        assert_eq!(Settings::from_json(b"{not json").onboarded, None);
        // Round trips, through the same reader the app uses.
        for (src, done) in [(MusicSource::LocalOnly, Some(1)), (MusicSource::Apple, None), (MusicSource::Apple, Some(1))] {
            let s = Settings { music_source: src, onboarded: done, ..Settings::default() };
            let back = Settings::from_json(&serde_json::to_vec_pretty(&s).unwrap());
            assert_eq!(back, s);
        }
        assert!(serde_json::to_string(&Settings::default()).unwrap().contains(r#""onboarded":null"#), "saved, so the next load knows");
    }

    #[test]
    fn the_storefront_loads_and_round_trips() {
        assert_eq!(Settings::default().storefront, None);
        assert_eq!(Settings::from_json(br#"{"onboarded":1}"#).storefront, None, "older files have none");
        let s = Settings::from_json(br#"{"onboarded":1,"storefront":{"id":"it"}}"#);
        let f = s.storefront.clone().unwrap();
        assert_eq!((f.id.as_str(), f.name.as_deref(), f.label().as_str()), ("it", None, "IT"));
        let s = Settings { storefront: Some(Storefront { id: "it".into(), name: Some("Italy".into()) }), ..s };
        let back = Settings::from_json(&serde_json::to_vec_pretty(&s).unwrap());
        assert_eq!(back, s);
        assert_eq!(back.storefront.unwrap().label(), "Italy");
    }

    #[test]
    fn the_last_route_round_trips() {
        use crate::models::CollectionKind;
        let s = Settings {
            start_page: StartPage::Last,
            last_route: Some(Route::Collection { kind: CollectionKind::Playlist, id: "p.9".into() }),
            repeat: Repeat::One,
            ..Settings::default()
        };
        let back: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
