//! Mockup icons (design-spec Appendix A), all on a 24-unit grid. Tinted by `.color()`.

use rune::Icon;
use std::sync::LazyLock;

macro_rules! stroke_icon {
    ($name:ident, $id:literal, $d:literal) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 24.0,
            body: concat!(
                r#"<path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d=""#,
                $d,
                r#""/>"#
            ),
        };
    };
}

/// The inside of an app-icon SVG (everything between `<svg>` and `</svg>`), as an `Icon` on the
/// SVGs' 512-unit grid. The text is leaked once: an `Icon` wants `&'static str`.
fn logo_icon(name: &'static str, svg: &'static str) -> Icon {
    let inner = &svg[svg.find('>').unwrap() + 1..svg.rfind("</svg>").unwrap()];
    Icon { name, view: 512.0, body: inner.trim() }
}

/// The app icon as the sidebar logo, four drawings: the small one (`uzumaki-small.svg`: fewer,
/// thicker turns, for 1x screens) and the rich one (`uzumaki.svg`, what the Dock shows; 2x and up),
/// each on its dark tile and on the light theme's white tile (the coral is 2.4 : 1 on white, so the
/// light ones use #d6304a). `widgets::logo_tile` picks one. Same tile and hairline in all four.
pub static LOGO: LazyLock<Icon> = LazyLock::new(|| logo_icon("cad-logo", include_str!("../../../assets/icons/uzumaki-small.svg")));
pub static LOGO_LIGHT: LazyLock<Icon> =
    LazyLock::new(|| logo_icon("cad-logo-light", include_str!("../../../assets/icons/uzumaki-small-light.svg")));
pub static LOGO_RICH: LazyLock<Icon> = LazyLock::new(|| logo_icon("cad-logo-rich", include_str!("../../../assets/icons/uzumaki.svg")));
pub static LOGO_RICH_LIGHT: LazyLock<Icon> =
    LazyLock::new(|| logo_icon("cad-logo-rich-light", include_str!("../../../assets/icons/uzumaki-light.svg")));

/// The coral app icon's rich drawing (`uzumaki-coral.svg`), for the App icon tiles in Settings
/// (drawn at 64 pt, where the rich drawing is the one the PNGs use). Not a theme logo.
pub static LOGO_RICH_CORAL: LazyLock<Icon> =
    LazyLock::new(|| logo_icon("cad-logo-rich-coral", include_str!("../../../assets/icons/uzumaki-coral.svg")));

pub static PLAY: Icon = Icon { name: "cad-play", view: 24.0, body: r#"<path d="M7 4v16l13-8z" fill="currentColor"/>"# };
pub static PAUSE: Icon =
    Icon { name: "cad-pause", view: 24.0, body: r#"<path d="M6 4h4v16H6zM14 4h4v16h-4z" fill="currentColor"/>"# };
pub static PREV: Icon =
    Icon { name: "cad-prev", view: 24.0, body: r#"<path d="M6 5h2v14H6zM20 5v14L9 12z" fill="currentColor"/>"# };
pub static NEXT: Icon =
    Icon { name: "cad-next", view: 24.0, body: r#"<path d="M16 5h2v14h-2zM4 5v14l11-7z" fill="currentColor"/>"# };
// Crossing arrows: two curved lanes that swap sides, arrowheads on the right.
stroke_icon!(
    SHUFFLE,
    "cad-shuffle",
    "M4 7h3c5 0 5 10 10 10h3M4 17h3c1.6 0 2.7-.8 3.6-2M14 9c.9-1.3 2-2 3-2h3M18 4l2.5 3L18 10M18 14l2.5 3L18 20"
);
// Volume: one speaker for every level, so only the waves change as the volume moves.
// The waves are arcs round (10, 12), 45° either side of horizontal.
stroke_icon!(VOLUME_MUTE, "cad-volume-mute", "M9.5 5.5 5 9H1.5v6H5l4.5 3.5zM14.5 9.5l5 5M19.5 9.5l-5 5");
stroke_icon!(VOLUME_LOW, "cad-volume-low", "M9.5 5.5 5 9H1.5v6H5l4.5 3.5zM13.18 8.82a4.5 4.5 0 0 1 0 6.36");
stroke_icon!(
    VOLUME_MID,
    "cad-volume-mid",
    "M9.5 5.5 5 9H1.5v6H5l4.5 3.5zM13.18 8.82a4.5 4.5 0 0 1 0 6.36M15.66 6.34a8 8 0 0 1 0 11.32"
);
stroke_icon!(
    VOLUME_HIGH,
    "cad-volume-high",
    "M9.5 5.5 5 9H1.5v6H5l4.5 3.5zM13.18 8.82a4.5 4.5 0 0 1 0 6.36M15.66 6.34a8 8 0 0 1 0 11.32M18.13 3.87a11.5 11.5 0 0 1 0 16.26"
);

/// The speaker for a volume in 0..=1: crossed out at 0 (what shows as 0%), then one, two or
/// three waves by thirds.
pub fn volume(v: f64) -> &'static Icon {
    if v < 0.005 {
        &VOLUME_MUTE
    } else if v < 1.0 / 3.0 {
        &VOLUME_LOW
    } else if v < 2.0 / 3.0 {
        &VOLUME_MID
    } else {
        &VOLUME_HIGH
    }
}
stroke_icon!(REPEAT, "cad-repeat", "M17 2l4 4-4 4M3 11V9a3 3 0 0 1 3-3h15M7 22l-4-4 4-4M21 13v2a3 3 0 0 1-3 3H3");
stroke_icon!(LYRICS, "cad-lyrics", "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2zM8 8h8M8 12h5");
stroke_icon!(QUEUE, "cad-queue", "M3 6h13M3 12h13M3 18h9M19 14v6M16 17h6");
stroke_icon!(CHEVRON_DOWN, "cad-chevron-down", "M6 9l6 6 6-6");
stroke_icon!(CHEVRON_RIGHT, "cad-chevron-right", "M9 6l6 6-6 6");
stroke_icon!(SEARCH, "cad-search", "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM21 21l-5-5");
pub static MORE: Icon = Icon {
    name: "cad-more",
    view: 24.0,
    body: r#"<circle cx="5" cy="12" r="2" fill="currentColor"/><circle cx="12" cy="12" r="2" fill="currentColor"/><circle cx="19" cy="12" r="2" fill="currentColor"/>"#,
};
stroke_icon!(CLOSE, "cad-close", "M6 6l12 12M18 6 6 18");
stroke_icon!(CHECK, "cad-check", "M5 12.5l4.5 4.5L19 7.5");
/// A small stroked uzumaki (semicircles of growing radius), the library-sync spinner.
pub static SWIRL: Icon = Icon {
    name: "cad-swirl",
    view: 24.0,
    body: r#"<path fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" d="M10.5 12a1.5 1.5 0 0 1 3 0a3 3 0 0 1-6 0a4.5 4.5 0 0 1 9 0a6 6 0 0 1-12 0a7.5 7.5 0 0 1 15 0"/>"#,
};
pub static SORT_UP: Icon = Icon { name: "cad-sort-up", view: 24.0, body: r#"<path d="M12 5 21 19H3z" fill="currentColor"/>"# };
pub static SORT_DOWN: Icon = Icon { name: "cad-sort-down", view: 24.0, body: r#"<path d="M12 19 21 5H3z" fill="currentColor"/>"# };

/// Thin line icons for the sidebar (24 grid, 1.7 stroke, round caps), our own drawings.
macro_rules! line_icon {
    ($name:ident, $id:literal, $($d:literal),+) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 24.0,
            body: concat!($(
                r#"<path fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" d=""#,
                $d,
                r#""/>"#
            ),+),
        };
    };
}

line_icon!(HOME, "cad-home", "M4 11.2 12 4l8 7.2", "M6 9.8V19a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V9.8", "M10 20v-5h4v5");
line_icon!(NEW, "cad-new", "M4.5 4.5h5v5h-5zM14.5 4.5h5v5h-5zM4.5 14.5h5v5h-5zM14.5 14.5h5v5h-5z");
line_icon!(
    RADIO,
    "cad-radio",
    "M10.6 12a1.4 1.4 0 1 0 2.8 0 1.4 1.4 0 1 0-2.8 0",
    "M8.6 9.4a4 4 0 0 0 0 5.2M15.4 9.4a4 4 0 0 1 0 5.2",
    "M5.8 6.6a8 8 0 0 0 0 10.8M18.2 6.6a8 8 0 0 1 0 10.8"
);
line_icon!(PIN, "cad-pin", "M9 4.5h6l-.8 5.2 2.8 3.3H7l2.8-3.3z", "M12 13v6.5");
line_icon!(CLOCK, "cad-clock", "M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16z", "M12 8v4.2l2.8 1.8");
line_icon!(ARROW_UP, "cad-arrow-up", "M12 19V5.5", "M6.5 11 12 5.5l5.5 5.5");
line_icon!(
    ARTISTS,
    "cad-artists",
    "M12 14.5a3 3 0 0 0 3-3V6.5a3 3 0 0 0-6 0v5a3 3 0 0 0 3 3z",
    "M6.5 11.5a5.5 5.5 0 0 0 11 0",
    "M12 17v3M9 20h6"
);
line_icon!(
    ALBUMS,
    "cad-albums",
    "M7.5 9h9a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 16.5 20h-9A1.5 1.5 0 0 1 6 18.5v-8A1.5 1.5 0 0 1 7.5 9z",
    "M8 6.2h8M9.5 3.5h5"
);
line_icon!(SONGS, "cad-songs", "M9.5 17.2V5.5l8 2v3.2", "M9.5 17.2a2.7 2.7 0 1 1-5.4 0 2.7 2.7 0 0 1 5.4 0z");
line_icon!(
    MADE_FOR_YOU,
    "cad-made-for-you",
    "M6 4.5h12A1.5 1.5 0 0 1 19.5 6v12a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 18V6A1.5 1.5 0 0 1 6 4.5z",
    "M12 11.2a2.4 2.4 0 1 0 0-4.8 2.4 2.4 0 0 0 0 4.8z",
    "M7.8 17.5a4.2 4.2 0 0 1 8.4 0"
);
line_icon!(
    ALL_PLAYLISTS,
    "cad-all-playlists",
    "M5 5h2v2H5zM11 5h2v2h-2zM17 5h2v2h-2zM5 11h2v2H5zM11 11h2v2h-2zM17 11h2v2h-2zM5 17h2v2H5zM11 17h2v2h-2zM17 17h2v2h-2z"
);
// Apple's favourite star: an outline, and filled for starred items.
stroke_icon!(STAR, "cad-star", "M12 3.6l2.6 5.3 5.8.85-4.2 4.1 1 5.8L12 16.9l-5.2 2.75 1-5.8-4.2-4.1 5.8-.85z");
pub static STAR_FILLED: Icon = Icon {
    name: "cad-star-filled",
    view: 24.0,
    body: r#"<path fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linejoin="round" d="M12 3.6l2.6 5.3 5.8.85-4.2 4.1 1 5.8L12 16.9l-5.2 2.75 1-5.8-4.2-4.1 5.8-.85z"/>"#,
};
line_icon!(
    FAVORITES,
    "cad-favorites",
    "M12 4l2.5 5.2 5.6.8-4.1 4 1 5.6L12 16.9l-5 2.7 1-5.6-4.1-4 5.6-.8z"
);
line_icon!(
    PLAYLIST,
    "cad-playlist",
    "M4 7.5h10M4 12h10M4 16.5h5.5",
    "M18.5 17V9l2.5 1",
    "M18.5 17a2 2 0 1 1-4 0 2 2 0 0 1 4 0z"
);
line_icon!(
    SIDEBAR,
    "cad-sidebar",
    "M6 5h12a1.5 1.5 0 0 1 1.5 1.5v11A1.5 1.5 0 0 1 18 19H6a1.5 1.5 0 0 1-1.5-1.5v-11A1.5 1.5 0 0 1 6 5z",
    "M9.5 5v14"
);
stroke_icon!(CHEVRON_LEFT, "cad-chevron-left", "M15 6l-6 6 6 6");
pub static DISCLOSE_OPEN: Icon = Icon { name: "cad-disclose-open", view: 24.0, body: r#"<path d="M5 8h14l-7 8.5z" fill="currentColor"/>"# };
pub static DISCLOSE_CLOSED: Icon =
    Icon { name: "cad-disclose-closed", view: 24.0, body: r#"<path d="M8 5v14l8.5-7z" fill="currentColor"/>"# };

line_icon!(
    GEAR,
    "cad-gear",
    "M12 5.8a6.2 6.2 0 1 0 0 12.4 6.2 6.2 0 0 0 0-12.4z",
    "M12 9.8a2.2 2.2 0 1 0 0 4.4 2.2 2.2 0 0 0 0-4.4z",
    "M12 3.5v2.3M12 18.2v2.5M3.5 12h2.3M18.2 12h2.5M18 6l-1.6 1.6M7.6 16.4 6 18M6 6l1.6 1.6M16.4 16.4 18 18"
);
line_icon!(
    USER,
    "cad-user",
    "M12 11.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z",
    "M5 20a7 7 0 0 1 14 0"
);
line_icon!(
    SLIDERS,
    "cad-sliders",
    "M4 8h9M17 8h3M4 16h3M11 16h9",
    "M15 6a2 2 0 1 0 0 4 2 2 0 0 0 0-4z",
    "M9 14a2 2 0 1 0 0 4 2 2 0 0 0 0-4z"
);
line_icon!(
    IMAGE,
    "cad-image",
    "M6 5h12a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z",
    "M4.5 16l4.5-4.5 3.5 3.5 2.5-2.5 4.5 4.5",
    "M15.2 9.2a1 1 0 1 1-2 0 1 1 0 0 1 2 0z"
);

line_icon!(
    KEYBOARD,
    "cad-keyboard",
    "M5.5 6.5h13A1.5 1.5 0 0 1 20 8v8a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 16V8a1.5 1.5 0 0 1 1.5-1.5z",
    "M7.5 10h.01M10.5 10h.01M13.5 10h.01M16.5 10h.01M8 14h8"
);
stroke_icon!(PLUS, "cad-plus", "M12 5v14M5 12h14");
stroke_icon!(
    FOLDER,
    "cad-folder",
    "M4 8A1.5 1.5 0 0 1 5.5 6.5H10l2 2.2h6.5A1.5 1.5 0 0 1 20 10.2V17.5A1.5 1.5 0 0 1 18.5 19h-13A1.5 1.5 0 0 1 4 17.5z"
);

line_icon!(
    LOCAL_FILES,
    "cad-local-files",
    "M4 8A1.5 1.5 0 0 1 5.5 6.5H10l2 2.2h6.5A1.5 1.5 0 0 1 20 10.2V17.5A1.5 1.5 0 0 1 18.5 19h-13A1.5 1.5 0 0 1 4 17.5z",
    "M11.4 15.4v-4l3-.8v3.2",
    "M11.4 15.4a1.1 1.1 0 1 1-2.2 0 1.1 1.1 0 0 1 2.2 0zM14.4 13.2a1.1 1.1 0 1 1-2.2 0 1.1 1.1 0 0 1 2.2 0z"
);
// Small "stored on this computer" marker (a drive) shown beside local songs.
line_icon!(
    LOCAL_MARK,
    "cad-local-mark",
    "M4.5 14.5l2-8h11l2 8",
    "M4.5 14.5h15v3.5a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1z",
    "M16.5 16.8h.01"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_icon_follows_the_level() {
        let name = |v: f64| volume(v).name;
        assert_eq!(name(0.0), "cad-volume-mute");
        assert_eq!(name(0.004), "cad-volume-mute", "what shows as 0% is muted");
        assert_eq!(name(0.01), "cad-volume-low");
        assert_eq!(name(0.33), "cad-volume-low");
        assert_eq!(name(0.34), "cad-volume-mid");
        assert_eq!(name(0.66), "cad-volume-mid");
        assert_eq!(name(0.67), "cad-volume-high");
        assert_eq!(name(1.0), "cad-volume-high");
    }

    /// The four logos are the SVG files' contents, and each light one is its dark one with the
    /// tile and the swirl recoloured; Rune caches icons by name, so the names differ.
    #[test]
    fn logos_are_the_svg_files() {
        let inner = |svg: &'static str| svg[svg.find('>').unwrap() + 1..svg.rfind("</svg>").unwrap()].trim();
        assert_eq!(LOGO.body, inner(include_str!("../../../assets/icons/uzumaki-small.svg")));
        assert_eq!(LOGO_RICH.body, inner(include_str!("../../../assets/icons/uzumaki.svg")));
        for (dark, light) in [(&LOGO, &LOGO_LIGHT), (&LOGO_RICH, &LOGO_RICH_LIGHT)] {
            let recoloured = dark.body.replace("#1d1d22", "#ffffff").replace("#34343c", "#d9d9df").replace("#ff7a8a", "#d6304a");
            assert_eq!(light.body, recoloured);
        }
        assert_eq!(LOGO_RICH_CORAL.body, inner(include_str!("../../../assets/icons/uzumaki-coral.svg")));
        let names = [LOGO.name, LOGO_LIGHT.name, LOGO_RICH.name, LOGO_RICH_LIGHT.name];
        for (i, n) in names.iter().enumerate() {
            assert!(!names[i + 1..].contains(n), "Rune caches icons by name");
        }
    }
}
