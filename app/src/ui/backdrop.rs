//! Background pictures: behind the content area (`app_layer`) and behind full-screen Now Playing
//! (`np_layer`). Rendering and the effect live in `crate::backdrop`; this only maps settings to
//! requests and draws what the service returns.

use std::f32::consts::PI;

use rune::{Cx, El, SurfaceFit, Transition, el};

use crate::appearance::{Appearance, BgSource, Effect, NpBg};
use crate::art::NOW_PLAYING_PX;
use crate::backdrop::{Ground, Request, Shown, Slot, Source, Target};
use crate::models::Song;
use crate::state::{LibraryTab, Route, State};
use crate::theme::*;

/// What a slot should show: a source with its effect, opacity and a scrim over it.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub source: Source,
    pub effect: Effect,
    pub opacity: f32,
}

/// The cover of `song` as a source (nothing without a song or artwork).
fn cover_source(song: Option<&Song>) -> Option<Source> {
    Some(Source::Url(song?.artwork.as_ref()?.url(NOW_PLAYING_PX)))
}

/// The content-area background for `ap`; `None` draws nothing.
pub fn app_plan(ap: &Appearance, song: Option<&Song>) -> Option<Plan> {
    let source = match ap.source {
        BgSource::Off => return None,
        BgSource::Builtin => Source::Builtin,
        BgSource::Picture => Source::File(ap.picture.clone()?),
        BgSource::Cover => cover_source(song)?,
    };
    Some(Plan { source, effect: ap.effect, opacity: ap.opacity() })
}

/// `app_plan` for the theme in force: on light the opacity is the light theme's own
/// (`Appearance::light_opacity`, which goes higher than dark's). The effects and the built-in
/// swirl draw on the theme's own ground: see `ground`.
pub fn themed_app_plan(ap: &Appearance, song: Option<&Song>) -> Option<Plan> {
    let plan = app_plan(ap, song)?;
    Some(if is_light() { Plan { opacity: ap.opacity_for(true), ..plan } } else { plan })
}

/// The ground backgrounds are rendered on for the palette in force (Now Playing builds under
/// `dark_scope`, so it asks for dark).
pub(super) fn ground() -> Ground {
    if is_light() { Ground::Light } else { Ground::Dark }
}

/// The Now Playing background; `None` is the cover-tinted gradient. `scrim` says whether a dark
/// scrim goes over it.
pub fn np_plan(ap: &Appearance, song: Option<&Song>) -> Option<(Plan, bool)> {
    match ap.np {
        NpBg::Tint => None,
        NpBg::Cover => Some((Plan { source: cover_source(song)?, effect: ap.np_effect, opacity: 1.0 }, true)),
        NpBg::App => Some((app_plan(ap, song)?, false)),
    }
}

/// Device-pixel size of an element `size` points big.
pub(super) fn target(size: [f32; 2], scale: f32) -> Target {
    let px = |v: f32| (v * scale).round().max(1.0) as u32;
    Target { width: px(size[0]), height: px(size[1]), unit: scale.round().max(1.0) as u32 }
}

/// The canvas a window-sized background is rendered for, in points: the largest screen (so a
/// resize never re-renders), grown to the window if the window is larger (spanning screens).
/// Without screen information (headless) it is the window.
fn canvas(win: [f32; 2]) -> [f32; 2] {
    match crate::backdrop::screen() {
        Some(scr) => [scr[0].max(win[0]).ceil(), scr[1].max(win[1]).ceil()],
        None => win,
    }
}

/// A canvas-sized picture centred on a `win`-sized parent: like a wallpaper, resizing the window
/// crops it differently but never scales it. The parent must clip.
fn wallpaper(shown: &Shown, opacity: f32, canvas: [f32; 2], win: [f32; 2]) -> El {
    el().key(format!("bg-{}", shown.id)).size(canvas[0], canvas[1])
        .absolute(((win[0] - canvas[0]) / 2.0).round(), ((win[1] - canvas[1]) / 2.0).round())
        .opacity(opacity).transition(Transition::opacity())
        .pixels_fit(&shown.pixels, SurfaceFit::Fill)
}

/// The picture, keyed by `Shown::id` so a new picture cross-fades with the old one. Opacity
/// is on the image itself (no group layer).
pub(super) fn picture(shown: &Shown, opacity: f32) -> El {
    el().key(format!("bg-{}", shown.id)).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).opacity(opacity)
        .transition(Transition::opacity())
        .pixels_fit(&shown.pixels, SurfaceFit::Fill)
}

/// Song-list pages (dense rows of text): collections, Library → Songs and Local Files.
pub fn is_list_page(route: &Route) -> bool {
    matches!(route, Route::Collection { .. } | Route::Library(LibraryTab::Songs) | Route::LocalFiles)
}

/// Behind everything in `app-root`, window-sized (only the content area shows it; the panels
/// around it are opaque): the picture, fading into the background colour towards the bottom when
/// `fade` is on. Call it every frame; it hides the slot when there is nothing to show.
pub fn app_layer(s: &State, cx: &Cx<State>) -> Option<El> {
    let ap = &s.settings.appearance;
    let Some(plan) = themed_app_plan(ap, s.playback.song.as_ref()) else {
        s.backdrops.hide(Slot::App);
        return None;
    };
    // No picture on this page; the slot stays rendered for the next card page.
    let opacity = ap.page_opacity(is_list_page(&s.route), is_light())?;
    let win = cx.layout_size("app-root")?;
    let canvas = canvas(win);
    let req = Request { source: plan.source, effect: plan.effect, ground: ground(), target: target(canvas, cx.env().scale) };
    let shown = s.backdrops.show(Slot::App, &req)?;
    let mut layer = el().key("bg-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).clip_rounded(0.0)
        .child(wallpaper(&shown, opacity, canvas, win));
    if ap.fade {
        let none = PAL.bg.alpha(0.0);
        layer = layer.child(
            el().key("bg-fade").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).linear_gradient(
                PI,
                [(0.0, none), (0.25, PAL.bg.alpha(0.15)), (0.7, PAL.bg.alpha(0.92)), (1.0, PAL.bg)],
            ),
        );
    }
    Some(layer)
}

/// True while Now Playing draws a picture (so `ui::view` knows when to free the slot).
pub fn np_active(s: &State) -> bool {
    s.ui.now_playing && np_plan(&s.settings.appearance, s.playback.song.as_ref()).is_some()
}

/// Behind Now Playing's content, or `None` for the tinted gradient. `win` is the overlay size.
pub fn np_layer(s: &State, cx: &Cx<State>, win: [f32; 2]) -> Option<El> {
    let (plan, scrim) = np_plan(&s.settings.appearance, s.playback.song.as_ref())?;
    let canvas = canvas(win);
    let req = Request { source: plan.source, effect: plan.effect, ground: ground(), target: target(canvas, cx.env().scale) };
    let shown = s.backdrops.show(Slot::NowPlaying, &req)?;
    let full = || el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    let mut layer = full().key("np-bg").clip_rounded(0.0).child(wallpaper(&shown, plan.opacity, canvas, win));
    // The cover is bright: darken it, more towards the bottom where the controls and lyrics sit.
    // The app's own background (already faint) only gets a light veil.
    let (top, bottom) = if scrim { (0.5, 0.72) } else { (0.32, 0.46) };
    layer = layer.child(
        full().key("np-scrim").linear_gradient(
            PI,
            [(0.0, rune::Color::BLACK.alpha(top)), (1.0, rune::Color::BLACK.alpha(bottom))],
        ),
    );
    Some(layer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Artwork;
    use std::path::PathBuf;

    fn song(with_art: bool) -> Song {
        Song {
            id: "1".into(), catalog_id: None, title: "t".into(), artist: "a".into(), album: "b".into(), duration: 1.0,
            track_number: None, disc_number: None, explicit: false, playable: true, genre: None, year: None, album_id: None, artists: Vec::new(),
            artwork: with_art.then(|| Artwork { template: "https://x/{w}x{h}.{f}".into(), bg: None }),
        }
    }

    #[test]
    fn app_sources() {
        let mut ap = Appearance::default();
        assert_eq!(app_plan(&ap, None), None, "Off draws nothing");
        ap.source = BgSource::Builtin;
        ap.effect = Effect::Dither;
        let p = app_plan(&ap, None).unwrap();
        assert_eq!((p.source, p.effect, p.opacity), (Source::Builtin, Effect::Dither, ap.opacity()));
        ap.source = BgSource::Picture;
        assert_eq!(app_plan(&ap, None), None, "no picture chosen");
        ap.picture = Some(PathBuf::from("/p.png"));
        assert_eq!(app_plan(&ap, None).unwrap().source, Source::File("/p.png".into()));
        ap.source = BgSource::Cover;
        assert_eq!(app_plan(&ap, None), None, "nothing plays");
        assert_eq!(app_plan(&ap, Some(&song(false))), None, "no artwork");
        let url = format!("https://x/{NOW_PLAYING_PX}x{NOW_PLAYING_PX}.jpg");
        assert_eq!(app_plan(&ap, Some(&song(true))).unwrap().source, Source::Url(url));
    }

    #[test]
    fn now_playing_sources() {
        let mut ap = Appearance::default();
        assert_eq!(np_plan(&ap, Some(&song(true))), None, "tint");
        ap.np = NpBg::Cover;
        ap.np_effect = Effect::Halftone;
        let (p, scrim) = np_plan(&ap, Some(&song(true))).unwrap();
        assert_eq!((p.effect, p.opacity, scrim), (Effect::Halftone, 1.0, true));
        assert_eq!(np_plan(&ap, Some(&song(false))), None, "no artwork: tint");
        ap.np = NpBg::App;
        assert_eq!(np_plan(&ap, Some(&song(true))), None, "app source Off: tint");
        ap.source = BgSource::Builtin;
        let (p, scrim) = np_plan(&ap, None).unwrap();
        assert_eq!((p.source, p.opacity, scrim), (Source::Builtin, ap.opacity(), false));
    }

    #[test]
    fn light_theme_keeps_effects_and_has_its_own_opacity() {
        let mut ap = Appearance { source: BgSource::Cover, effect: Effect::Plain, opacity: 0.6, light_opacity: 0.85, ..Appearance::default() };
        let song = song(true);
        crate::theme::apply(false);
        assert_eq!(themed_app_plan(&ap, Some(&song)).unwrap().opacity, 0.6, "dark: its own");
        assert_eq!(ground(), Ground::Dark);
        crate::theme::apply(true);
        assert_eq!(ground(), Ground::Light);
        assert_eq!(themed_app_plan(&ap, Some(&song)).unwrap().opacity, 0.85, "light: its own, above the dark ceiling");
        ap.effect = Effect::Dither;
        ap.light_opacity = 0.35;
        let p = themed_app_plan(&ap, Some(&song)).unwrap();
        assert_eq!((p.effect, p.opacity), (Effect::Dither, 0.35), "drawn on the light ground");
        ap.source = BgSource::Builtin;
        assert!(themed_app_plan(&ap, None).is_some());
        assert_eq!(crate::theme::dark_scope(ground), Ground::Dark, "Now Playing stays dark");
        crate::theme::apply(false);
    }

    #[test]
    fn list_pages() {
        use crate::models::CollectionKind;
        assert!(is_list_page(&Route::Collection { kind: CollectionKind::Album, id: "1".into() }));
        assert!(is_list_page(&Route::Library(LibraryTab::Songs)));
        assert!(is_list_page(&Route::LocalFiles));
        assert!(!is_list_page(&Route::Library(LibraryTab::Albums)));
        assert!(!is_list_page(&Route::Library(LibraryTab::Playlists)));
        for r in [Route::Home, Route::Search, Route::New, Route::Radio, Route::RecentlyAdded, Route::Artists, Route::MadeForYou] {
            assert!(!is_list_page(&r));
        }
        assert!(!is_list_page(&Route::Artist { id: "1".into(), name: "n".into() }));
    }

    #[test]
    fn target_is_device_pixels() {
        let t = target([100.4, 50.0], 2.0);
        assert_eq!((t.width, t.height, t.unit), (201, 100, 2));
        assert_eq!(target([0.0, 0.0], 1.0).width, 1);
        assert_eq!(target([10.0, 10.0], 1.5).unit, 2);
    }
}
