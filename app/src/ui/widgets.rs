//! Small reusable pieces: art tiles, cards, buttons, chips, sliders, status text.

use rune::{Cursor, Drag, El, Icon, Tx, col, el, icon, oklch, row, text};

use super::icons;
use crate::models::{Artwork, Collection, CollectionKind};
use crate::state::State;
use crate::theme::*;

/// `m:ss`.
pub fn fmt_time(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    format!("{}:{:02}", t / 60, t % 60)
}

/// "38 min" or "1 hr 5 min".
pub fn fmt_total(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if m >= 60 { format!("{} hr {} min", m / 60, m % 60) } else { format!("{m} min") }
}

fn hash(s: &str) -> u32 {
    s.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

fn initials(title: &str) -> String {
    title.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).take(2).collect::<String>().to_uppercase()
}

thread_local! {
    /// Set while building something far off screen (`off_screen`).
    static OFF_SCREEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Builds `f` as off screen: its covers draw if they are in memory, but none is asked for or held
/// (`Art::peek`). For the far cards of a carousel, which the view builds whether or not they show.
pub fn off_screen<R>(f: impl FnOnce() -> R) -> R {
    let was = OFF_SCREEN.replace(true);
    let r = f();
    OFF_SCREEN.set(was);
    r
}

pub enum Size {
    Fixed(f32),
    /// Full width of the parent, square.
    Fill,
    /// Fixed size, but fetching the shared small cover (`art::SMALL_PX`): the player bar, Up Next
    /// and sidebar thumbnails all hit the same cache entry, and Now Playing reuses it.
    Small(f32),
}

/// Square artwork with rounded corners; a hashed pastel tile with initials until the image arrives.
pub fn art(s: &State, artwork: Option<&Artwork>, title: &str, size: Size, radius: f32, initials_pt: f32) -> El {
    let (base, px) = match size {
        Size::Fixed(f) => (el().size(f, f), (f * 2.0).ceil() as u32),
        Size::Fill => (el().w_full().aspect_ratio(1.0), 320),
        Size::Small(f) => (el().size(f, f), crate::art::SMALL_PX),
    };
    let base = base.radius(radius);
    let off = OFF_SCREEN.get();
    if let Some(img) = artwork.and_then(|a| if off { s.art.peek(a, px) } else { s.art.get(a, px) }) {
        return base.fill(PAL.card_hover).pixels_fit(img.pixels(), rune::SurfaceFit::Fill);
    }
    let tile = base.fill(oklch(0.78, 0.075, (hash(title) % 360) as f32));
    if initials_pt > 0.0 {
        tile.child(
            text(initials(title))
                .font(rune::TextStyle::new(initials_pt, 600.0))
                .color(PAL.art_ink)
                .pin(Some(initials_pt * 0.55), None, None, Some(initials_pt * 0.5)),
        )
    } else {
        tile
    }
}

/// A small thumbnail that stays on screen (a sidebar playlist or pin; `owner` its id), drawn
/// from `held`: `Art::held`'s picture for it, looked up by the caller (so a memoized row can key
/// on it). Its first picture fades in over the tinted tile; after that a new link, an evicted
/// cover or a picture still decoding keeps the last one, so it never goes back to the tile.
pub fn held_art(held: Option<&(rune::Image, bool)>, title: &str, size: f32, radius: f32, initials_pt: f32) -> El {
    let tint = oklch(0.78, 0.075, (hash(title) % 360) as f32);
    let Some((img, fresh)) = held else {
        return el().size(size, size).radius(radius).fill(tint).child(
            text(initials(title))
                .font(rune::TextStyle::new(initials_pt, 600.0))
                .color(PAL.art_ink)
                .pin(Some(initials_pt * 0.55), None, None, Some(initials_pt * 0.5)),
        );
    };
    // The same key whatever the picture: a new one swaps in place, only the first mounts (and fades).
    let pic = el().key("thumb-pic").size(size, size).radius(radius).pixels_fit(img.pixels(), rune::SurfaceFit::Fill);
    let pic = if *fresh { pic.transition(rune::Transition::opacity()) } else { pic };
    el().size(size, size).radius(radius).fill(if *fresh { tint } else { PAL.card_hover }).child(pic)
}

/// Round avatar with the first letter of `name`, on a hashed tint (artists have no artwork).
pub fn avatar(name: &str, size: f32) -> El {
    let letter: String = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
    el().size(size, size).radius(size / 2.0).center().fill(oklch(0.46, 0.05, (hash(name) % 360) as f32))
        .child(text(letter).font(rune::TextStyle::new(size * 0.42, 600.0)).color(PAL.text2))
}

pub fn heading(label: impl Into<String>) -> El {
    text(label).font(H1).color(PAL.text)
}

pub fn subheading(label: impl Into<String>) -> El {
    text(label).font(H2).color(PAL.text)
}

pub fn muted(label: impl Into<String>) -> El {
    text(label).font(BODY14).color(PAL.muted)
}

/// A centred-ish message for Loading / Failed / empty states.
pub fn status(label: impl Into<String>, error: bool) -> El {
    row().w_full().py(24.0).child(
        text(label).font(BODY14).color(if error { PAL.danger_text } else { PAL.dim }).wrap(),
    )
}

/// Pill button with a leading icon (Play / Shuffle).
pub fn pill(key: &'static str, label: &'static str, ic: &'static Icon, bg: rune::Color, fg: rune::Color) -> El {
    row().key(key).role("button").label(label).focusable(true).h(44.0).px(22.0).gap(8.0).center().radius(22.0)
        .fill(bg).hover(crate::theme::ov(0.08)).press(rune::Color::BLACK.alpha(0.12)).cursor(Cursor::Pointer)
        .child(icon(ic, 16.0).color(fg))
        .child(text(label).font(BUTTON).color(fg))
}

pub fn small_button(key: &'static str, label: &'static str) -> El {
    el().key(key).role("button").label(label).focusable(true).h(30.0).px(14.0).radius(15.0).center()
        .fill(PAL.nav_active).hover(crate::theme::ov(0.08)).press(crate::theme::ov(0.12))
        .cursor(Cursor::Pointer)
        .child(text(label).font(rune::TextStyle::new(13.0, 600.0)).color(PAL.text))
}

pub fn chip(key: String, label: &'static str, on: bool) -> El {
    let (bg, fg) = if on { (PAL.text, PAL.ink) } else { (PAL.chip, PAL.text2) };
    let c = el().key(key).role("button").label(label).focusable(true).h(36.0).px(16.0).radius(18.0).center().fill(bg)
        .cursor(Cursor::Pointer);
    let c = if on { c } else { c.hover(PAL.chip_hover.alpha(0.9)) };
    c.child(text(label).font(SMALL13).color(fg))
}

/// A horizontal slider: 16pt hit area around a 4pt track. `frac` is the fill (0..1);
/// `on_drag` gets the pointer fraction and phase.
pub fn slider(
    key: &'static str,
    frac: f32,
    fill: rune::Color,
    on_drag: impl Fn(&mut State, f32, rune::Phase, bool, &mut Tx) + 'static,
) -> El {
    let frac = frac.clamp(0.0, 1.0);
    slider_hit(key, 16.0, frac, on_drag).child(
        row().w_full().h(4.0).radius(2.0).clip_rounded(2.0).fill(PAL.track)
            .child(el().grow(frac).h_full().fill(fill))
            .child(el().grow((1.0 - frac).max(0.0))),
    )
}

/// A slider's hit area, `h` tall, without its track (the caller adds one).
pub fn slider_hit(
    key: &'static str,
    h: f32,
    frac: f32,
    on_drag: impl Fn(&mut State, f32, rune::Phase, bool, &mut Tx) + 'static,
) -> El {
    el().key(key).role("slider").label(key).focusable(true).w_full().h(h).items_center().cursor(Cursor::Pointer)
        .range_value(frac.clamp(0.0, 1.0) as f64, 0.0, 1.0)
        .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| {
            let f = if d.w > 0.0 { (d.x / d.w).clamp(0.0, 1.0) } else { 0.0 };
            on_drag(s, f, d.phase, d.cancelled(), tx);
        })
}

pub fn card_subtitle(c: &Collection) -> String {
    match c.kind {
        CollectionKind::Album => match &c.year {
            Some(y) if !c.subtitle.is_empty() => format!("{} \u{b7} {y}", c.subtitle),
            _ => c.subtitle.clone(),
        },
        CollectionKind::Playlist => c.subtitle.clone(),
    }
}

/// Album / playlist card of width `w`.
pub fn card(s: &State, c: &Collection, w: f32) -> El {
    let (c2, c3) = (c.clone(), c.clone());
    col().key(format!("card-{}", c.id)).role("button").label(c.title.clone()).focusable(true)
        .w(w).padding(8.0).gap(8.0).radius(12.0).hover(PAL.card_hover).press(crate::theme::ov(0.04))
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.open_collection(&c2))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| super::ctx_menu::open_collection(s, &c3, [p.x, p.y]))
        .child(art(s, s.cover_of(c), &c.title, Size::Fill, 8.0, 22.0))
        .child(
            col().gap(2.0).min_w(0.0)
                .child(text(c.title.clone()).font(CARD_TITLE).color(PAL.text).truncate())
                .child(text(card_subtitle(c)).font(SMALL13).color(PAL.muted).truncate()),
        )
}

/// Card grid geometry for a content width (min card 160, gap 12), like CSS auto-fill.
#[derive(Clone, Copy)]
pub struct Grid {
    pub n: usize,
    /// Card width.
    pub w: f32,
}

impl Grid {
    pub fn new(avail: f32) -> Self {
        let n = (((avail + 12.0) / 172.0).floor() as usize).clamp(2, 10);
        Self { n, w: ((avail - 12.0 * (n as f32 - 1.0)) / n as f32).floor() }
    }

    /// Estimated height of one grid row (card + the vertical padding that makes the row gap).
    pub fn row_height(&self) -> f32 {
        self.w + 8.0 + 8.0 + 8.0 + 18.0 + 2.0 + 17.0 + 12.0
    }
}

/// Row `i` of a grid of `n` columns (vertical padding supplies the 12pt row gap).
pub fn card_row(s: &State, items: &[Collection], g: Grid, i: usize) -> El {
    let mut r = row().w_full().gap(12.0).py(6.0).items_start();
    for c in items.iter().skip(i * g.n).take(g.n) {
        r = r.child(card(s, c, g.w));
    }
    r
}

/// The app icon, `size` points across as drawn (the SVG's tile fills 464/512 of its box, its
/// hairline included); on the light theme its white-tile variant. From 2x displays on it is the
/// rich drawing (the one the Dock shows), on 1x the small one, whose thicker turns hold at 30 px.
pub fn logo_tile(size: f32) -> El {
    let (light, rich) = (crate::theme::is_light(), crate::art::display_scale() >= 1.5);
    let logo: &Icon = match (rich, light) {
        (false, false) => &icons::LOGO,
        (false, true) => &icons::LOGO_LIGHT,
        (true, false) => &icons::LOGO_RICH,
        (true, true) => &icons::LOGO_RICH_LIGHT,
    };
    el().size(size, size).center().child(icon(logo, (size * 512.0 / 464.0).round()))
}

/// The app icon as it looks on the desktop, `size` points across (the tile fills 464/512 of the
/// box), whatever the theme: the rich drawing, which the 48 px and larger PNGs come from, drawn
/// as a vector at the screen's scale so it stays sharp at 2x.
pub fn app_icon_image(which: crate::settings::IconVariant, size: f32) -> El {
    use crate::settings::IconVariant;
    let logo: &Icon = match which {
        IconVariant::Dark => &icons::LOGO_RICH,
        IconVariant::Light => &icons::LOGO_RICH_LIGHT,
        IconVariant::Coral => &icons::LOGO_RICH_CORAL,
    };
    el().size(size, size).center().child(icon(logo, (size * 512.0 / 464.0).round()))
}

/// The Settings tile picture of an App icon choice. Follow theme shows both icons it switches
/// between, split down the middle: the dark one on the left, the light one on the right.
pub fn app_icon_preview(which: crate::settings::AppIcon, size: f32) -> El {
    use crate::settings::{AppIcon, IconVariant};
    let fixed = match which {
        AppIcon::Dark => IconVariant::Dark,
        AppIcon::Light => IconVariant::Light,
        AppIcon::Coral => IconVariant::Coral,
        AppIcon::Auto => {
            let half = size / 2.0;
            let side = |v: IconVariant, left: f32| {
                el().absolute(left, 0.0).size(half, size).clip_rounded(0.0)
                    .child(el().absolute(-left, 0.0).child(app_icon_image(v, size)))
            };
            return el().size(size, size).child(side(IconVariant::Dark, 0.0)).child(side(IconVariant::Light, half));
        }
    };
    app_icon_image(fixed, size)
}
