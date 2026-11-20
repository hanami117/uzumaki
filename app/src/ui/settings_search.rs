//! Settings rows: a static table of every row (its page, title, note and a few synonyms). The
//! pages take their titles and notes from it; search folds each row once into one lower-case,
//! accent-free string. A keystroke folds the query and
//! keeps the rows that contain every word of it (`find`); the view only filters that short list.

use std::sync::LazyLock;

use super::keys;
use super::settings::Tab;
use super::songs_table::fold;

/// One row of a settings page, as search finds it. `ui::settings::row_el` builds each one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    MusicSource,
    AppleMusic,
    /// Why explicit songs may be refused (Settings → Account; Apple Music only).
    ExplicitContent,
    Quality,
    Lyrics,
    /// Resume where you left off (Settings → Playback; also in local only mode).
    Resume,
    /// AutoPlay (Settings → Playback; Apple Music only), the ∞ in Up Next.
    AutoPlay,
    /// Reconnect to the last speaker (Settings → Speakers; also in local only mode).
    AirPlay,
    /// Chromecast on or off (Settings → Speakers; Unix only).
    Chromecast,
    Theme,
    AppIcon,
    Player,
    StartupAnimation,
    AnimatedArtwork,
    Background,
    Picture,
    Effect,
    ShowOn,
    CardStrength,
    ListStrength,
    Fade,
    NpBackground,
    NpCoverEffect,
    Sidebar,
    OpenOn,
    /// "Open at login" (`login.rs`).
    StartAtLogin,
    /// "Open in the background" (a start at login opens no window).
    StartHidden,
    Tray,
    CloseToTray,
    Notify,
    /// "Show on Discord" and its four choices (`discord/`).
    Discord,
    DiscordArtwork,
    DiscordButtons,
    DiscordHidePaused,
    DiscordHideTitle,
    DiscordStatus,
    /// "Check for updates daily": the switch, Check now and when the last check ran.
    CheckUpdates,
    Tour,
    ArtCache,
    LibraryCache,
    MusicFolders,
    NumberKeys,
    /// The version and copyright (Settings → About).
    About,
    /// The licences (Settings → About).
    Licences,
    /// getuzumaki.app and the source code (Settings → About).
    Links,
    /// Where to support the app (Settings → About).
    Support,
    /// A shortcut: `keys::BINDINGS[i]`.
    Binding(usize),
}

impl Row {
    /// The key of the row's wrapper (search scrolls to it and flashes it).
    pub fn key(self) -> String {
        let slug = match self {
            Row::Binding(i) => return format!("settings/row/kb-{i}"),
            Row::MusicSource => "music-source",
            Row::AppleMusic => "apple-music",
            Row::ExplicitContent => "explicit-content",
            Row::Quality => "quality",
            Row::Lyrics => "lyrics",
            Row::Resume => "resume",
            Row::AutoPlay => "autoplay",
            Row::AirPlay => "airplay",
            Row::Chromecast => "chromecast",
            Row::Theme => "theme",
            Row::AppIcon => "app-icon",
            Row::Player => "player",
            Row::StartupAnimation => "startup-animation",
            Row::Background => "background",
            Row::Picture => "picture",
            Row::Effect => "effect",
            Row::ShowOn => "show-on",
            Row::CardStrength => "card-strength",
            Row::ListStrength => "list-strength",
            Row::Fade => "fade",
            Row::NpBackground => "np-background",
            Row::NpCoverEffect => "np-cover-effect",
            Row::AnimatedArtwork => "animated-artwork",
            Row::Sidebar => "sidebar",
            Row::OpenOn => "open-on",
            Row::StartAtLogin => "start-at-login",
            Row::StartHidden => "start-hidden",
            Row::Tray => "tray",
            Row::CloseToTray => "close-to-tray",
            Row::Notify => "notify",
            Row::Discord => "discord",
            Row::DiscordArtwork => "discord-artwork",
            Row::DiscordButtons => "discord-buttons",
            Row::DiscordHidePaused => "discord-hide-paused",
            Row::DiscordHideTitle => "discord-hide-title",
            Row::DiscordStatus => "discord-status",
            Row::CheckUpdates => "check-updates",
            Row::Tour => "tour",
            Row::ArtCache => "art-cache",
            Row::LibraryCache => "library-cache",
            Row::MusicFolders => "music-folders",
            Row::NumberKeys => "number-keys",
            Row::About => "about",
            Row::Licences => "licences",
            Row::Links => "links",
            Row::Support => "support",
        };
        format!("settings/row/{slug}")
    }
}

/// The system a row is listed on (the tray rows read differently on the Mac, App icon exists
/// only on Linux and the Mac).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
    Other,
}

impl Os {
    pub const fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum On {
    All,
    Mac,
    Linux,
    /// Windows (`Os::Other`).
    Win,
    NotMac,
}

impl On {
    fn has(self, os: Os) -> bool {
        match self {
            On::All => true,
            On::Mac => os == Os::Mac,
            On::Linux => os == Os::Linux,
            On::Win => os == Os::Other,
            On::NotMac => os != Os::Mac,
        }
    }
}

struct Spec {
    row: Row,
    tab: Tab,
    on: On,
    title: &'static str,
    note: &'static str,
    /// Synonyms: words people may type that the title and note don't hold (old labels too).
    words: &'static str,
}

const fn spec(row: Row, tab: Tab, title: &'static str, note: &'static str, words: &'static str) -> Spec {
    Spec { row, tab, on: On::All, title, note, words }
}

const fn only(on: On, row: Row, tab: Tab, title: &'static str, note: &'static str, words: &'static str) -> Spec {
    Spec { row, tab, on, title, note, words }
}

/// Every row of every page, in page order: the one place a row's page, title and note are written.
/// The pages (`ui::settings`) take their titles and notes from here, so search can't drift from
/// them. A note that changes with the state (a file name, the last update check) is the page's
/// own; the note here is then what search matches.
const SPECS: &[Spec] = &[
    // ---- General ----
    spec(Row::OpenOn, Tab::General, "Open on", "The page Uzumaki shows when it opens.", "startup start page launch home last page"),
    only(On::Linux, Row::StartAtLogin, Tab::General, "Open at login", "Adds Uzumaki to your desktop's autostart programs (~/.config/autostart).", "start uzumaki when i log in autostart auto start startup boot login log in logon sign in launch automatically session"),
    only(On::Mac, Row::StartAtLogin, Tab::General, "Open at login", "Listed in System Settings \u{203a} General \u{203a} Login Items.", "start uzumaki when i log in autostart auto start startup boot login log in logon launch login items automatically"),
    only(On::Win, Row::StartAtLogin, Tab::General, "Open at login", "Listed in Windows Settings \u{203a} Apps \u{203a} Startup.", "start uzumaki when i log in autostart auto start startup boot login log in logon sign in launch automatically task manager"),
    only(On::NotMac, Row::StartHidden, Tab::General, "Open in the background", "At login, no window: only the tray icon.", "start in the background hidden minimised minimized minimise minimize tray silent quiet no window autostart startup login"),
    only(On::Mac, Row::StartHidden, Tab::General, "Open in the background", "At login, no window: only the menu bar icon.", "start in the background hidden minimised minimized minimise minimize menu bar tray silent quiet no window dock autostart startup login"),
    only(On::Linux, Row::Tray, Tab::General, "Tray icon", "Play, pause and skip from the tray. On GNOME, needs the AppIndicator extension.", "show in system tray menu bar status icon notification area"),
    only(On::Mac, Row::Tray, Tab::General, "Menu bar icon", "Play, pause and skip from the menu bar.", "show icon in the menu bar tray status icon"),
    only(On::Win, Row::Tray, Tab::General, "Tray icon", "Play, pause and skip from the notification area, next to the clock.", "show in system tray menu bar status icon taskbar"),
    only(On::NotMac, Row::CloseToTray, Tab::General, "Keep playing when the window closes", "Quit from the tray icon or with the Quit shortcut.", "close to tray menu bar background minimize minimise quit"),
    only(On::Mac, Row::CloseToTray, Tab::General, "Keep playing when the window closes", "The Dock icon goes too. Quit from the menu bar icon or with the Quit shortcut.", "close to the menu bar tray background minimize minimise quit dock"),
    spec(Row::Sidebar, Tab::General, "Sidebar", "What the sidebar lists. You can also right-click an entry to hide it.", "navigation hide show entries menu edit sidebar"),
    // ---- Account ----
    spec(Row::MusicSource, Tab::Account, "Music source", MUSIC_SOURCE_APPLE, "offline local mode streaming"),
    spec(Row::AppleMusic, Tab::Account, "Apple Music", "Signed in, store.", "sign in sign out log in login logout apple id storefront country region store account"),
    spec(Row::ExplicitContent, Tab::Account, "Explicit content", "It is set on music.apple.com, with the same Apple Account. Screen Time may not reach it.", "explicit restricted restriction refused content_restricted screen time parental controls child age e clean"),
    // ---- Playback ----
    spec(Row::Quality, Tab::Playback, "Audio quality", "Data saver uses a quarter of the data. Apple Music on the web has no Lossless or Dolby Atmos.", "streaming quality best high bitrate kbps audio sound 256 64 mobile"),
    spec(Row::AutoPlay, Tab::Playback, "AutoPlay", "When the queue ends, similar music keeps playing (the \u{221e} in Up Next). Not with Repeat or on radio.", "auto play infinite endless continue similar songs radio station suggestions queue end infinity"),
    spec(Row::Resume, Tab::Playback, "Resume where you left off", "Uzumaki opens on the last song and its queue, paused.", "restore continue last song position queue startup launch remember session"),
    // ---- Speakers ----
    spec(Row::AirPlay, Tab::Speakers, "Reconnect to the last speaker", "At start, play on the AirPlay speaker you used last, if it is on. Off: always this computer.", "airplay device speaker homepod apple tv output cast wireless reconnect startup launch remember"),
    spec(Row::Chromecast, Tab::Speakers, "Chromecast", "Google TVs and Cast speakers on this network show up in the speaker menu, a couple of seconds behind. Off: Uzumaki doesn't look for them.", "chromecast google cast tv android tv google tv nest speaker output wireless stream network device"),
    // ---- Appearance ----
    spec(Row::Theme, Tab::Appearance, "Theme", "System follows your computer's light or dark appearance.", "dark mode light mode colour color appearance night"),
    spec(Row::Player, Tab::Appearance, "Player bar", "Full width at the bottom, or a floating capsule.", "player docked floating controls"),
    only(On::Linux, Row::AppIcon, Tab::Appearance, "App icon", "In the dock, taskbar and menu. The tray icon changes at the next start.", "icon logo dark light coral launcher"),
    only(On::Mac, Row::AppIcon, Tab::Appearance, "App icon", "In the Dock and the Finder.", "icon logo dark light coral launcher"),
    spec(Row::StartupAnimation, Tab::Appearance, "Startup animation", "The swirl that opens when Uzumaki starts.", "intro splash launch swirl motion"),
    spec(Row::Background, Tab::Appearance, "Background", "A picture behind the pages.", "wallpaper backdrop image cover"),
    spec(Row::Picture, Tab::Appearance, "Picture", "The background picture.", "wallpaper image photo file background choose"),
    spec(Row::Effect, Tab::Appearance, "Effect", "The background's effect.", "dither ascii halftone blur scanlines background"),
    spec(Row::ShowOn, Tab::Appearance, "Show on", "Card pages only, or song lists too.", "background pages"),
    spec(Row::CardStrength, Tab::Appearance, "Strength on card pages", "Home, New, Radio, grids and artist pages.", "card page strength background opacity transparency"),
    spec(Row::ListStrength, Tab::Appearance, "Strength on song lists", "Albums, playlists, Songs and Local Files.", "song list strength background opacity transparency"),
    spec(Row::Fade, Tab::Appearance, "Fade out at the bottom", "Long lists end on the plain background.", "fade toward the bottom background gradient"),
    // ---- Now Playing ----
    spec(Row::Lyrics, Tab::NowPlaying, "Highlight lyrics", "Word by word when the song has word timing, otherwise by line.", "lyrics highlight karaoke words lines sync"),
    spec(Row::AnimatedArtwork, Tab::NowPlaying, "Animated artwork", "Apple's video loop of an album, in Now Playing and, with Always, atop album pages. Off downloads no video.", "motion video moving cover album art loop album page always off still"),
    spec(Row::NpBackground, Tab::NowPlaying, "Background", "A colour from the cover, the cover itself, or the app's background.", "full screen fullscreen tint color colour same as app"),
    spec(Row::NpCoverEffect, Tab::NowPlaying, "Cover effect", "The effect on the cover behind Now Playing.", "full screen fullscreen dither blur background"),
    // ---- Notifications ----
    spec(Row::Notify, Tab::Notifications, "Song notifications", "When a new song starts and Uzumaki isn't in front.", "notify on song change notification alert popup toast"),
    spec(Row::Discord, Tab::Notifications, "Show on Discord", "Your Discord profile shows the song, like Spotify does. Needs the Discord app on this computer.", "discord rich presence activity status listening profile game"),
    spec(Row::DiscordStatus, Tab::Notifications, "Status text", "Next to your name in the member list. Needs a recent Discord.", "discord show in status line member list mini profile compact song title artist app name"),
    spec(Row::DiscordArtwork, Tab::Notifications, "Album cover", "On your Discord card. Off: the Uzumaki icon.", "discord show artwork cover album art picture image"),
    spec(Row::DiscordButtons, Tab::Notifications, "\u{201c}Open in Apple Music\u{201d} button", "On your Discord card, for songs from Apple Music.", "discord show buttons button link share"),
    spec(Row::DiscordHidePaused, Tab::Notifications, "Hide when paused", "Off: the card stays, without the progress bar.", "discord pause paused stop remove clear"),
    spec(Row::DiscordHideTitle, Tab::Notifications, "Hide the song title", "Shows \u{201c}Listening to music\u{201d} and the artist only.", "discord private privacy hide song title name secret"),
    // ---- Local Files ----
    spec(Row::MusicFolders, Tab::Local, "Music folders", "Songs in these folders (and their subfolders) show up under Local Files.", "folder directory files flac mp3 rescan scan import"),
    // ---- Keyboard ----
    spec(Row::NumberKeys, Tab::Keyboard, "Number keys 1\u{2013}9", "Your sidebar pins or pages.", "shortcuts keys hotkeys keybindings pins pages ctrl alt digits"),
    // ---- Advanced ----
    spec(Row::CheckUpdates, Tab::Advanced, "Check for updates daily", "Sends only the app version and system details. Nothing is downloaded.", "update updates upgrade new version release download check now last checked"),
    spec(Row::Tour, Tab::Advanced, "Welcome tour", "The welcome and the shortcuts tour.", "onboarding intro help tutorial shortcuts keys keyboard show again"),
    spec(Row::ArtCache, Tab::Advanced, "Artwork cache", "The downloaded covers.", "cache caches clear disk storage space covers images"),
    spec(Row::LibraryCache, Tab::Advanced, "Library copy", "The copy of your Apple Music library.", "library cache caches clear disk storage space offline"),
    // ---- About ----
    spec(Row::About, Tab::About, "About Uzumaki", "Version, build and copyright.", "version build about copyright credits"),
    spec(Row::Licences, Tab::About, "Licences", "GPL-3.0-or-later, castLabs Electron, symphonia, rodio, Rune, Inter.", "licence license licences licenses gpl gnu free software castlabs electron widevine symphonia rodio inter plex fonts third party not affiliated apple"),
    spec(Row::Links, Tab::About, "Links", "getuzumaki.app and the source code.", "website site getuzumaki.app github source code links repository"),
    spec(Row::Support, Tab::About, "Support Uzumaki", "Uzumaki is free and built in my spare time. If you enjoy it, you can support it on Ko-fi.", "donate donation tip coffee ko-fi kofi sponsor heart"),
];

/// Account → Music source, in Apple Music mode (the table's note) and in local only mode.
pub const MUSIC_SOURCE_APPLE: &str = "Apple Music, with your music folders beside it. Local files only stops Apple Music's engine and hides its pages; your sign-in is kept.";
pub const MUSIC_SOURCE_LOCAL: &str = "Only your music folders. Apple Music's engine doesn't run and its pages are hidden. Your Apple sign-in and library copy are kept for when you switch back.";

/// `row`'s entry on `os`, if it is listed there.
fn spec_on(os: Os, row: Row) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.row == row && s.on.has(os))
}

/// `row`'s title on this system, as its page and search show it ("" for a row this system lacks).
pub fn title(row: Row) -> &'static str {
    spec_on(Os::current(), row).map_or("", |s| s.title)
}

/// `row`'s note on this system ("" for a row this system lacks).
pub fn note(row: Row) -> &'static str {
    spec_on(Os::current(), row).map_or("", |s| s.note)
}

/// The page `row` is on.
pub fn tab(row: Row) -> Tab {
    match row {
        Row::Binding(_) => Tab::Keyboard,
        _ => SPECS.iter().find(|s| s.row == row).map_or(Tab::General, |s| s.tab),
    }
}

/// The rows the table lists on `tab`, in order, each once.
#[cfg(test)]
pub fn rows_of(tab: Tab) -> Vec<Row> {
    let mut v: Vec<Row> = Vec::new();
    for s in SPECS.iter().filter(|s| s.tab == tab) {
        if !v.contains(&s.row) {
            v.push(s.row);
        }
    }
    v
}

/// The words every shortcut row also answers to.
const BINDING_WORDS: &str = "shortcut shortcuts keys hotkey hotkeys keybinding keybindings key binding";

/// A row as search sees it.
pub struct Entry {
    pub row: Row,
    /// Page, title, note and synonyms, folded (`songs_table::fold`).
    hay: String,
}

/// The rows listed on `os`, with their folded text, in page order (`Tab::ALL`); the shortcuts
/// go in Keyboard's place.
pub fn index(os: Os) -> Vec<Entry> {
    let mut v = Vec::new();
    for t in Tab::ALL {
        v.extend(SPECS.iter().filter(|s| s.tab == t && s.on.has(os)).map(|s| Entry {
            row: s.row,
            hay: fold(&format!("{} {} {} {}", s.tab.title(), s.title, s.note, s.words)),
        }));
        if t == Tab::Keyboard {
            for (i, b) in keys::BINDINGS.iter().enumerate() {
                v.push(Entry {
                    row: Row::Binding(i),
                    hay: fold(&format!("{} {} {} {BINDING_WORDS}", Tab::Keyboard.title(), b.label, b.group)),
                });
            }
        }
    }
    v
}

/// This system's rows, built on first use.
pub static INDEX: LazyLock<Vec<Entry>> = LazyLock::new(|| index(Os::current()));

/// The rows (indices into `index`) holding every word of `query`, case and accents aside, in
/// page order. An empty query finds nothing.
pub fn find_in(index: &[Entry], query: &str) -> Vec<usize> {
    let q = fold(query);
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    index.iter().enumerate().filter(|(_, e)| words.iter().all(|w| e.hay.contains(w))).map(|(i, _)| i).collect()
}

/// `find_in` over this system's rows.
pub fn find(query: &str) -> Vec<usize> {
    find_in(&INDEX, query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(os: Os, q: &str) -> Vec<Row> {
        let ix = index(os);
        find_in(&ix, q).into_iter().map(|i| ix[i].row).collect()
    }

    fn finds(os: Os, q: &str, row: Row) -> bool {
        rows(os, q).contains(&row)
    }

    #[test]
    fn matches_titles_notes_and_page_names() {
        assert_eq!(rows(Os::Linux, "streaming quality"), vec![Row::Quality]);
        assert!(finds(Os::Linux, "atmos", Row::Quality), "a word from the note");
        assert!(finds(Os::Linux, "now playing", Row::Lyrics), "the page name");
        assert!(finds(Os::Linux, "advanced", Row::ArtCache));
        assert!(finds(Os::Linux, "local files", Row::MusicFolders));
        assert!(rows(Os::Linux, "").is_empty(), "an empty query finds nothing");
        assert!(rows(Os::Linux, "   ").is_empty());
        assert!(rows(Os::Linux, "zzzz").is_empty());
    }

    #[test]
    fn every_word_must_match() {
        assert_eq!(rows(Os::Linux, "card strength"), vec![Row::CardStrength]);
        assert!(rows(Os::Linux, "theme quality").is_empty());
    }

    #[test]
    fn synonyms() {
        for (q, row) in [
            ("icon", Row::AppIcon),
            ("tray", Row::Tray),
            ("menu bar", Row::Tray),
            ("theme", Row::Theme),
            ("dark", Row::Theme),
            ("light", Row::Theme),
            ("dark", Row::AppIcon),
            ("lyrics", Row::Lyrics),
            ("bitrate", Row::Quality),
            ("wallpaper", Row::Background),
            ("sign out", Row::AppleMusic),
            ("cache", Row::ArtCache),
            ("cache", Row::LibraryCache),
            ("shortcuts", Row::NumberKeys),
            ("log in", Row::StartAtLogin),
            ("autostart", Row::StartAtLogin),
            ("start at login", Row::StartAtLogin),
            ("startup", Row::StartAtLogin),
            ("background", Row::StartHidden),
            ("minimised", Row::StartHidden),
            ("minimized", Row::StartHidden),
            ("start hidden", Row::StartHidden),
        ] {
            assert!(finds(Os::Linux, q, row), "{q:?} finds {row:?}");
            assert!(finds(Os::Mac, q, row), "{q:?} finds {row:?} on the Mac");
        }
        let sidebar = keys::BINDINGS.iter().position(|b| b.action == keys::Action::ToggleSidebar).unwrap();
        assert!(finds(Os::Linux, "keys", Row::Binding(sidebar)));
        assert!(finds(Os::Linux, "shortcuts sidebar", Row::Binding(sidebar)));
    }

    #[test]
    fn case_and_accents_dont_matter() {
        assert!(finds(Os::Linux, "THEME", Row::Theme));
        assert!(finds(Os::Linux, "thème", Row::Theme));
        assert!(finds(Os::Linux, "Thémé", Row::Theme));
        assert!(finds(Os::Linux, "LÝRÏCS", Row::Lyrics));
        assert!(finds(Os::Linux, "  WÄLLPAPER ", Row::Background));
    }

    #[test]
    fn platform_rows_match_only_where_they_exist() {
        // The Linux tray note names GNOME's AppIndicator; the Mac one doesn't exist there.
        assert!(finds(Os::Linux, "appindicator", Row::Tray));
        assert!(!finds(Os::Mac, "appindicator", Row::Tray));
        // The Mac's App icon note names the Finder.
        assert!(finds(Os::Mac, "finder", Row::AppIcon));
        assert!(!finds(Os::Linux, "finder", Row::AppIcon));
        // No App icon setting elsewhere.
        assert!(!finds(Os::Other, "app icon", Row::AppIcon));
        assert!(finds(Os::Other, "tray", Row::Tray));
        // Each system lists each row once.
        for os in [Os::Mac, Os::Linux, Os::Other] {
            let ix = index(os);
            for (i, e) in ix.iter().enumerate() {
                assert!(!ix[..i].iter().any(|o| o.row == e.row), "{:?} twice on {os:?}", e.row);
            }
        }
    }

    #[test]
    fn results_keep_page_order() {
        let ix = index(Os::Linux);
        let hits = find_in(&ix, "lyrics");
        let tabs: Vec<Tab> = hits.iter().map(|&i| tab(ix[i].row)).collect();
        assert!(tabs.contains(&Tab::NowPlaying) && tabs.contains(&Tab::Keyboard), "{tabs:?}");
        let order = |t: Tab| Tab::ALL.iter().position(|x| *x == t).unwrap();
        assert!(tabs.windows(2).all(|w| order(w[0]) <= order(w[1])), "{tabs:?}");
    }

    /// Every row has a title and a note on each system that has it (App icon: not Windows).
    #[test]
    fn every_row_has_its_text_everywhere() {
        for os in [Os::Mac, Os::Linux, Os::Other] {
            for sp in SPECS {
                if sp.row == Row::AppIcon && os == Os::Other {
                    assert!(spec_on(os, sp.row).is_none());
                    continue;
                }
                let e = spec_on(os, sp.row).unwrap_or_else(|| panic!("{:?} on {os:?}", sp.row));
                assert!(!e.title.is_empty() && !e.note.is_empty(), "{:?} on {os:?}", sp.row);
            }
        }
    }

    /// Old labels still find their rows (people remember them).
    #[test]
    fn old_labels_still_find_their_rows() {
        for (q, row) in [
            ("streaming quality", Row::Quality),
            ("lyrics highlight", Row::Lyrics),
            ("start uzumaki when i log in", Row::StartAtLogin),
            ("start in the background", Row::StartHidden),
            ("close to tray", Row::CloseToTray),
            ("notify on song change", Row::Notify),
            ("show in status", Row::DiscordStatus),
            ("show artwork", Row::DiscordArtwork),
            ("show buttons", Row::DiscordButtons),
            ("card page strength", Row::CardStrength),
            ("fade toward the bottom", Row::Fade),
            ("reconnect to last airplay device", Row::AirPlay),
            ("check now", Row::CheckUpdates),
            ("library cache", Row::LibraryCache),
        ] {
            assert!(finds(Os::Linux, q, row), "{q:?} finds {row:?}");
        }
        assert!(finds(Os::Mac, "close to the menu bar", Row::CloseToTray));
        assert!(finds(Os::Mac, "show icon in the menu bar", Row::Tray));
    }

    /// The labels each system shows for the tray rows.
    #[test]
    fn tray_labels_per_system() {
        let t = |os, row| spec_on(os, row).unwrap().title;
        assert_eq!(t(Os::Mac, Row::Tray), "Menu bar icon");
        assert_eq!(t(Os::Linux, Row::Tray), "Tray icon");
        assert_eq!(t(Os::Other, Row::Tray), "Tray icon");
        for os in [Os::Mac, Os::Linux, Os::Other] {
            assert_eq!(t(os, Row::CloseToTray), "Keep playing when the window closes");
            assert_eq!(t(os, Row::StartAtLogin), "Open at login");
        }
        assert!(spec_on(Os::Other, Row::Tray).unwrap().note.contains("notification area"));
        assert!(spec_on(Os::Other, Row::StartAtLogin).unwrap().note.contains("Windows Settings"));
    }
}
