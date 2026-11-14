//! Settings search: a static table of every row (its page, title, note and a few synonyms),
//! folded once into one lower-case, accent-free string per row. A keystroke folds the query and
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
    Quality,
    Lyrics,
    /// Resume where you left off (Settings → Playback; also in local only mode).
    Resume,
    /// AutoPlay (Settings → Playback; Apple Music only), the ∞ in Up Next.
    AutoPlay,
    /// Reconnect to last AirPlay device (Settings → Playback; also in local only mode).
    AirPlay,
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
    Tray,
    CloseToTray,
    Notify,
    CheckUpdates,
    /// "Check now" and when the last check ran.
    CheckNow,
    Tour,
    ArtCache,
    LibraryCache,
    MusicFolders,
    NumberKeys,
    /// The version and licence (Settings → General → About).
    About,
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
            Row::Quality => "quality",
            Row::Lyrics => "lyrics",
            Row::Resume => "resume",
            Row::AutoPlay => "autoplay",
            Row::AirPlay => "airplay",
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
            Row::Tray => "tray",
            Row::CloseToTray => "close-to-tray",
            Row::Notify => "notify",
            Row::CheckUpdates => "check-updates",
            Row::CheckNow => "check-now",
            Row::Tour => "tour",
            Row::ArtCache => "art-cache",
            Row::LibraryCache => "library-cache",
            Row::MusicFolders => "music-folders",
            Row::NumberKeys => "number-keys",
            Row::About => "about",
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
    NotMac,
}

impl On {
    fn has(self, os: Os) -> bool {
        match self {
            On::All => true,
            On::Mac => os == Os::Mac,
            On::Linux => os == Os::Linux,
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
    /// Synonyms: words people may type that the title and note don't hold.
    words: &'static str,
}

const fn spec(row: Row, tab: Tab, title: &'static str, note: &'static str, words: &'static str) -> Spec {
    Spec { row, tab, on: On::All, title, note, words }
}

const fn only(on: On, row: Row, tab: Tab, title: &'static str, note: &'static str, words: &'static str) -> Spec {
    Spec { row, tab, on, title, note, words }
}

/// Every row of every page, in page order. Titles and notes follow the pages (`ui::settings`);
/// notes that change with the state are left out or given in their usual form.
const SPECS: &[Spec] = &[
    spec(Row::MusicSource, Tab::Account, "Music source", "Apple Music, or local files only.", "offline local mode streaming"),
    spec(Row::AppleMusic, Tab::Account, "Apple Music", "Signed in, store.", "sign in sign out log in login logout apple id storefront country region store"),
    spec(Row::Quality, Tab::Playback, "Streaming quality", "Data saver uses about a quarter of the data. Lossless and Dolby Atmos aren't available in Apple Music on the web.", "bitrate kbps audio sound 256 64 mobile"),
    spec(Row::Lyrics, Tab::Playback, "Lyrics highlight", "Word by word when Apple has word timing, otherwise whole lines.", "karaoke words lines sync"),
    spec(Row::AutoPlay, Tab::Playback, "AutoPlay", "When the queue ends, similar music keeps playing. Also the \u{221e} button in Up Next.", "auto play infinite endless continue similar songs radio station suggestions queue end infinity"),
    spec(Row::Resume, Tab::Playback, "Resume where you left off", "At start, the last song comes back paused where you stopped, with its queue. Nothing plays until you press Play.", "restore continue last song position queue startup launch remember session"),
    spec(Row::AirPlay, Tab::Playback, "Reconnect to last AirPlay device", "At start, the sound goes back to the AirPlay speaker you used last, if it is on. Off: Uzumaki always starts on this computer.", "airplay speaker homepod apple tv output cast wireless reconnect startup launch remember"),
    spec(Row::Theme, Tab::Appearance, "Theme", "System follows your computer's light or dark appearance.", "dark mode light mode colour color appearance night"),
    only(On::Linux, Row::AppIcon, Tab::Appearance, "App icon", "In the dock, taskbar and menu. The tray icon changes at the next start.", "icon logo dark light coral launcher"),
    only(On::Mac, Row::AppIcon, Tab::Appearance, "App icon", "In the Dock and the Finder.", "icon logo dark light coral launcher"),
    spec(Row::Player, Tab::Appearance, "Player", "The bar at the bottom: full width, or a floating capsule.", "player bar docked floating controls"),
    spec(Row::StartupAnimation, Tab::Appearance, "Startup animation", "The Uzumaki swirl twists open when the app starts. The first library sync always shows it.", "intro splash launch swirl motion"),
    spec(Row::AnimatedArtwork, Tab::Appearance, "Animated artwork", ANIMATED_ARTWORK, "motion video moving cover album art loop now playing album page always off still"),
    spec(Row::Background, Tab::Appearance, "Background", "A picture behind the content area.", "wallpaper backdrop image cover"),
    spec(Row::Picture, Tab::Appearance, "Picture", "Choose a background picture.", "wallpaper image photo file background"),
    spec(Row::Effect, Tab::Appearance, "Effect", "Background effect.", "dither ascii halftone blur scanlines background"),
    spec(Row::ShowOn, Tab::Appearance, "Show on", "Card pages only, or every page including song lists.", "background pages"),
    spec(Row::CardStrength, Tab::Appearance, "Card page strength", "Home, New, Radio, album grids and artist pages.", "background opacity strength transparency"),
    spec(Row::ListStrength, Tab::Appearance, "Song list strength", "Album and playlist pages, Songs and Local Files.", "background opacity strength transparency"),
    spec(Row::Fade, Tab::Appearance, "Fade toward the bottom", "Keeps long lists on the plain background.", "background gradient"),
    spec(Row::NpBackground, Tab::Appearance, "Now Playing background", "Colour tints the screen with the cover. Same as app uses the picture above.", "full screen fullscreen tint color"),
    spec(Row::NpCoverEffect, Tab::Appearance, "Now Playing cover effect", "The effect on the cover behind Now Playing.", "full screen fullscreen dither blur"),
    spec(Row::Sidebar, Tab::Appearance, "Sidebar", "Turn off what you don't use. Right-click an entry in the sidebar to hide it from there.", "navigation hide show entries menu"),
    spec(Row::OpenOn, Tab::General, "Open on", "Home or the last page.", "startup start page launch home last page"),
    only(On::NotMac, Row::Tray, Tab::General, "Show in system tray", "Play, pause and skip from the tray. Needs a tray host (on GNOME, the AppIndicator extension).", "tray menu bar status icon notification area"),
    only(On::Mac, Row::Tray, Tab::General, "Show icon in the menu bar", "Play, pause and skip from the menu bar.", "tray menu bar status icon"),
    only(On::NotMac, Row::CloseToTray, Tab::General, "Close to tray", "Closing the window keeps the music playing. Quit from the tray or with the Quit shortcut.", "tray menu bar background minimize minimise quit"),
    only(On::Mac, Row::CloseToTray, Tab::General, "Close to the menu bar", "Closing the window also hides the Dock icon and keeps the music playing. Quit from the menu bar icon or with the Quit shortcut.", "tray menu bar background minimize minimise quit"),
    spec(Row::Notify, Tab::General, "Notify on song change", "Shows the song and its cover when a new one starts while Uzumaki isn't in front.", "notification notifications alert popup toast"),
    spec(Row::CheckUpdates, Tab::General, "Check for updates", "Once a day, asks getuzumaki.app whether a new version is out. Nothing about you is sent.", "update updates upgrade new version release download"),
    spec(Row::CheckNow, Tab::General, "Check now", "Asks getuzumaki.app now whether a new version is out. Shows when it last checked.", "update updates upgrade new version release check for updates last checked"),
    spec(Row::Tour, Tab::General, "Welcome tour", "Plays the welcome and the shortcuts tour again.", "onboarding intro help tutorial shortcuts keys keyboard"),
    spec(Row::ArtCache, Tab::General, "Artwork cache", "Clear the downloaded covers.", "cache caches clear disk storage space covers images"),
    spec(Row::LibraryCache, Tab::General, "Library cache", "Clear the copy of your library.", "cache caches clear disk storage space offline"),
    spec(Row::About, Tab::General, "About Uzumaki", "Version and licence.", "version about licence license gpl website copyright"),
    spec(Row::MusicFolders, Tab::Local, "Music folders", "Songs in these folders (and their subfolders) show up under Local Files.", "folder directory files flac mp3 rescan scan import"),
    spec(Row::NumberKeys, Tab::Keyboard, "Number keys 1–9", "Your sidebar pins or pages.", "shortcuts keys hotkeys keybindings pins pages ctrl alt digits"),
];

/// The words every shortcut row also answers to.
const BINDING_WORDS: &str = "shortcut shortcuts keys hotkey hotkeys keybinding keybindings key binding";

/// A row as search sees it.
pub struct Entry {
    pub row: Row,
    pub tab: Tab,
    /// Page, title, note and synonyms, folded (`songs_table::fold`).
    hay: String,
}

/// The rows listed on `os`, with their folded text.
pub fn index(os: Os) -> Vec<Entry> {
    let mut v: Vec<Entry> = SPECS.iter().filter(|s| s.on.has(os)).map(|s| Entry {
        row: s.row,
        tab: s.tab,
        hay: fold(&format!("{} {} {} {}", s.tab.title(), s.title, s.note, s.words)),
    }).collect();
    for (i, b) in keys::BINDINGS.iter().enumerate() {
        v.push(Entry {
            row: Row::Binding(i),
            tab: Tab::Keyboard,
            hay: fold(&format!("{} {} {} {BINDING_WORDS}", Tab::Keyboard.title(), b.label, b.group)),
        });
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

/// Settings → Appearance → Animated artwork: what it does.
pub const ANIMATED_ARTWORK: &str = "Apple's video loop of an album, when it has one: in the Now Playing cover and, with Always, at the top of an album page. Off keeps still covers and downloads no video.";

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
        assert!(finds(Os::Linux, "playback", Row::Lyrics), "the page name");
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
        let tabs: Vec<Tab> = hits.iter().map(|&i| ix[i].tab).collect();
        assert!(tabs.contains(&Tab::Playback) && tabs.contains(&Tab::Keyboard), "{tabs:?}");
        let order = |t: Tab| Tab::ALL.iter().position(|x| *x == t).unwrap();
        assert!(tabs.windows(2).all(|w| order(w[0]) <= order(w[1])), "{tabs:?}");
    }
}
