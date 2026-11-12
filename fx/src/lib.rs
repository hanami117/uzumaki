//! Background effects for Uzumaki (Settings → Appearance): pure functions from a decoded picture
//! to the RGBA image the app draws behind its content or behind Now Playing.
//!
//! The effects are adapted from monocode's background effects (MIT; see THIRD_PARTY_NOTICES.md
//! at the repo root): dither, ASCII, halftone and scanlines. They run once per (picture, effect,
//! size) on a background thread, never per frame.
//!
//! CONTRACT (fixed by the main session; the engine agent implements the bodies, keeps the
//! signatures): `render`, `builtin`, `Effect`, `Target`.

use image::RgbaImage;
use serde::{Deserialize, Serialize};

/// A look applied to the background picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    /// The picture as it is (cover-cropped to the target).
    #[default]
    Plain,
    /// Ordered 4×4 Bayer dither in 2×2-point cells: each cell is either the colour at full
    /// brightness or near black.
    Dither,
    /// The picture drawn as 5×7 bitmap glyphs in 6×8-point cells, densest glyph where brightest.
    Ascii,
    /// Dots on a 4-point grid, sized by brightness.
    Halftone,
    /// Every third line darkened, as an old monitor.
    Scanlines,
    /// A soft, heavy blur (the colours only).
    Blur,
}

impl Effect {
    pub const ALL: [Effect; 6] = [Effect::Plain, Effect::Dither, Effect::Ascii, Effect::Halftone, Effect::Scanlines, Effect::Blur];

    /// One line for Settings, under the effect picker.
    pub fn description(self) -> &'static str {
        match self {
            Effect::Plain => "The picture as it is.",
            Effect::Dither => "Recreates the picture in solid colour pixels, like an old screen.",
            Effect::Ascii => "Redraws the picture as coloured text characters.",
            Effect::Halftone => "Recreates the picture with coloured print dots.",
            Effect::Scanlines => "Darkens every third line, like an old monitor.",
            Effect::Blur => "A soft haze of the picture's colours.",
        }
    }

    /// Display name for Settings.
    pub fn label(self) -> &'static str {
        match self {
            Effect::Plain => "None",
            Effect::Dither => "Dither",
            Effect::Ascii => "ASCII",
            Effect::Halftone => "Halftone",
            Effect::Scanlines => "Scanlines",
            Effect::Blur => "Blur",
        }
    }
}

/// The theme the background sits on. Dark is the original look (effects draw light on black);
/// Light draws the same effects as ink on near-white paper: dither and halftone dots in the
/// picture's colours on white, ASCII glyphs dark on light, scanlines lightened instead of
/// darkened. Transparent sources are flattened onto the theme's background colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Ground {
    #[default]
    Dark,
    Light,
}

impl Ground {
    /// The app's background colour on this ground (transparent pixels are flattened onto it).
    fn bg(self) -> [f32; 3] {
        match self {
            Ground::Dark => BG,
            Ground::Light => BG_LIGHT,
        }
    }
}

/// What to render: the size of the element it will fill, in device pixels, and `unit` device
/// pixels per point (1 at 100% scale, 2 at 200%; rounded, at least 1). Cell sizes (dither
/// cells, glyphs, dots, lines) are in points, so they look the same at any scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    pub unit: u32,
}

/// Cover-crops `src` to `target` (scaled to fill it, centred, the overflow cut off), then applies
/// `effect`. Returns straight-alpha RGBA, opaque.
///
/// The result is exactly `target.width × target.height`, except for `Effect::Blur`, which may
/// return a smaller image with the same aspect ratio (the app draws it stretched to fill, and a
/// blurred image loses nothing by being upscaled). A zero-sized target returns a 1×1 image.
pub fn render(src: &RgbaImage, effect: Effect, target: Target) -> RgbaImage {
    render_on(src, effect, target, Ground::Dark)
}

/// `render` for a theme. `Ground::Dark` is exactly `render`. On `Ground::Light` the effects run
/// on the inverted picture and the result is inverted back: "light on black" becomes "ink on
/// white" with the picture's own hues (a cell lit at full brightness becomes a cell inked at full
/// strength). A pale picture's ink is deepened first (`deepen_ink`), so it shows on white as a
/// bright picture shows on black. Plain and Blur only differ in the colour transparent pixels
/// are flattened onto.
pub fn render_on(src: &RgbaImage, effect: Effect, target: Target, ground: Ground) -> RgbaImage {
    let (w, h) = (target.width.max(1), target.height.max(1));
    let unit = target.unit.max(1);
    if effect == Effect::Blur {
        // Work at about 1/8 size, the longer side at most BLUR_MAX.
        let k = (BLUR_MAX as f32 / w.max(h) as f32).min(1.0);
        let (bw, bh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        let mut img = cover(src, bw, bh, ground.bg());
        gaussian(&mut img, BLUR_SIGMA * bw.max(bh) as f32 / BLUR_MAX as f32);
        if ground == Ground::Light {
            paper_floor(&mut img);
        }
        return img;
    }
    let mut img = cover(src, w, h, ground.bg());
    let ink = ground == Ground::Light && effect != Effect::Plain;
    if ink {
        invert(&mut img);
        deepen_ink(&mut img);
    }
    match effect {
        Effect::Plain | Effect::Blur => {}
        Effect::Dither => dither(&mut img, unit),
        Effect::Ascii => ascii(&mut img, unit),
        Effect::Halftone => halftone(&mut img, unit),
        Effect::Scanlines => scanlines(&mut img, unit),
    }
    if ink {
        invert(&mut img);
    }
    if ground == Ground::Light {
        paper_floor(&mut img);
    }
    img
}

/// Darkest a light-ground background gets (sRGB, of 255), at full opacity. Text sits straight on
/// the picture (no backing), so this trades picture against text: at 90 a bright test picture
/// keeps every text element at 4.5:1 (titles 3:1) up to the 0.9 top strength with the darker
/// light ink over a picture (`theme::LIGHT_INK`), while showing about as much picture as an
/// unfloored one at 0.6. A very dark picture at high strength can still bring small grey text below
/// 4.5:1.
pub const LIGHT_FLOOR: u8 = 90;

/// Prints the picture pale on the light ground: every channel is mapped from `0..=255` onto
/// `LIGHT_FLOOR..=255` (`255 - (255 - c) * k`), keeping hues and the picture's whole tonal range
/// (darker stays darker), so the strength slider shows more of the picture without ever darkening
/// it past what text can stand on.
fn paper_floor(img: &mut RgbaImage) {
    let k = (255 - LIGHT_FLOOR) as f32 / 255.0;
    par_bands(img, 64, |_, rows| {
        for p in rows.chunks_exact_mut(4) {
            for c in &mut p[..3] {
                *c = 255 - ((255 - *c) as f32 * k).round() as u8;
            }
        }
    });
}

/// `255 - c` on the colour channels (alpha untouched), on all cores.
fn invert(img: &mut RgbaImage) {
    par_bands(img, 64, |_, rows| {
        for p in rows.chunks_exact_mut(4) {
            p[0] = !p[0];
            p[1] = !p[1];
            p[2] = !p[2];
        }
    });
}

/// How much ink the darkest parts of a picture get on the light ground (as luma, of 1), and the
/// most a pale picture's ink is multiplied by to get there.
const INK_TARGET: f32 = 0.85;
const INK_MAX_GAIN: f32 = 5.0;

/// On the inverted picture (bright = ink), scales the ink so the picture's darkest parts (98th
/// percentile of ink) print at about `INK_TARGET`. A pale picture (mostly white and pastel)
/// would otherwise be near-blank paper at any opacity; a picture that already has dark parts is
/// left alone, and so is a flat one (no range to show).
fn deepen_ink(img: &mut RgbaImage) {
    let (w, h) = img.dimensions();
    let step = ((w.max(h) / 160).max(1)) as usize;
    let mut l: Vec<f32> = Vec::new();
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            l.push(luma(&img.get_pixel(x, y).0));
        }
    }
    if l.len() < 2 {
        return;
    }
    l.sort_by(f32::total_cmp);
    let at = |q: f32| l[((l.len() - 1) as f32 * q) as usize];
    let (lo, hi) = (at(0.02), at(0.98));
    if hi - lo < 0.1 {
        return;
    }
    let gain = (INK_TARGET / hi).clamp(1.0, INK_MAX_GAIN);
    if gain <= 1.0 {
        return;
    }
    par_bands(img, 64, |_, rows| {
        for p in rows.chunks_exact_mut(4) {
            for c in &mut p[..3] {
                *c = (*c as f32 * gain).round().min(255.0) as u8;
            }
        }
    });
}

/// Longer side of the blurred image, and the Gaussian's sigma at that size.
const BLUR_MAX: u32 = 256;
const BLUR_SIGMA: f32 = 9.0;
/// The UI's background colour: transparent sources are flattened onto it.
const BG: [f32; 3] = [14.0, 14.0, 17.0];
/// The light theme's background colour (#fcfcfd).
const BG_LIGHT: [f32; 3] = [252.0, 252.0, 253.0];

/// Scales `src` to fill `w × h`, centred, the overflow cut off, transparency flattened onto
/// `bg`. Opaque result.
fn cover(src: &RgbaImage, w: u32, h: u32, bg: [f32; 3]) -> RgbaImage {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 {
        return RgbaImage::from_pixel(w, h, image::Rgba([bg[0] as u8, bg[1] as u8, bg[2] as u8, 255]));
    }
    // The part of the source with the target's aspect ratio, centred (in source pixels).
    let (cw, ch) = if (sw as u64 * h as u64) > (sh as u64 * w as u64) {
        (((sh as u64 * w as u64 + h as u64 / 2) / h as u64).clamp(1, sw as u64) as u32, sh)
    } else {
        (sw, ((sw as u64 * h as u64 + w as u64 / 2) / w as u64).clamp(1, sh as u64) as u32)
    };
    let view = image::imageops::crop_imm(src, (sw - cw) / 2, (sh - ch) / 2, cw, ch);
    let view = view.to_image();
    let mut out = if (cw, ch) == (w, h) {
        view
    } else if cw < 2 * w && ch < 2 * h {
        bilinear(&view, w, h)
    } else {
        image::imageops::resize(&view, w, h, image::imageops::FilterType::Triangle)
    };
    for p in out.pixels_mut() {
        if p.0[3] != 255 {
            let a = p.0[3] as f32 / 255.0;
            for c in 0..3 {
                p.0[c] = (p.0[c] as f32 * a + bg[c] * (1.0 - a)).round() as u8;
            }
            p.0[3] = 255;
        }
    }
    out
}

/// Bilinear resize (every output pixel reads four source pixels; fine up to about 2x down), on
/// all cores.
fn bilinear(src: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (sw, sh) = src.dimensions();
    let (fx, fy) = (sw as f32 / w as f32, sh as f32 / h as f32);
    let raw = src.as_raw();
    let mut out = RgbaImage::new(w, h);
    par_bands(&mut out, 16, |y0, rows| {
        for (i, row) in rows.chunks_exact_mut(w as usize * 4).enumerate() {
            let sy = (((y0 + i as u32) as f32 + 0.5) * fy - 0.5).max(0.0);
            let (y_a, ty) = (sy as u32, sy.fract());
            let y_b = (y_a + 1).min(sh - 1);
            let y_a = y_a.min(sh - 1);
            for (x, p) in row.chunks_exact_mut(4).enumerate() {
                let sx = ((x as f32 + 0.5) * fx - 0.5).max(0.0);
                let (x_a, tx) = (sx as u32, sx.fract());
                let x_b = (x_a + 1).min(sw - 1);
                let x_a = x_a.min(sw - 1);
                let at = |x: u32, y: u32, c: usize| raw[((y * sw + x) * 4) as usize + c] as f32;
                for c in 0..4 {
                    let top = at(x_a, y_a, c) * (1.0 - tx) + at(x_b, y_a, c) * tx;
                    let bot = at(x_a, y_b, c) * (1.0 - tx) + at(x_b, y_b, c) * tx;
                    p[c] = (top * (1.0 - ty) + bot * ty + 0.5) as u8;
                }
            }
        }
    });
    out
}

fn luma(p: &[u8]) -> f32 {
    (p[0] as f32 * 0.2126 + p[1] as f32 * 0.7152 + p[2] as f32 * 0.0722) / 255.0
}

fn mix(a: u8, b: f32, t: f32) -> u8 {
    (a as f32 * (1.0 - t) + b * t).round().clamp(0.0, 255.0) as u8
}

// The dither, ASCII, halftone and scanline effects below are adapted from monocode's
// newThreadBackgroundEffects.worker.ts (https://github.com/hardbeat920/monocode, MIT licence),
// dark-theme variants (`render_on` gets the light ones by inverting around them). Cell sizes are scaled by `unit` (device pixels per point).

/// Runs `f(y0, rows)` over horizontal bands of `band` rows (the last may be shorter) on all
/// cores. `rows` is the band's RGBA bytes, its first row being image row `y0`.
fn par_bands(img: &mut RgbaImage, band: u32, f: impl Fn(u32, &mut [u8]) + Sync) {
    let stride = img.width() as usize * 4 * band as usize;
    let bands: Vec<(u32, &mut [u8])> = img.as_mut().chunks_mut(stride).enumerate().map(|(i, c)| (i as u32 * band, c)).collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8).min(bands.len()).max(1);
    if threads == 1 {
        // One core, or wasm32 (`available_parallelism` fails there and spawning a thread traps):
        // run the bands on this thread, no scope.
        bands.into_iter().for_each(|(y0, rows)| f(y0, rows));
        return;
    }
    let mut groups: Vec<Vec<(u32, &mut [u8])>> = (0..threads).map(|_| Vec::new()).collect();
    for (i, b) in bands.into_iter().enumerate() {
        groups[i % threads].push(b);
    }
    std::thread::scope(|s| {
        for group in groups {
            let f = &f;
            s.spawn(move || group.into_iter().for_each(|(y0, rows)| f(y0, rows)));
        }
    });
}

/// 4×4 Bayer dither in `2*unit` cells: each cell is its colour at full brightness or near black.
fn dither(img: &mut RgbaImage, unit: u32) {
    const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let cell = 2 * unit;
    let (w, h) = img.dimensions();
    par_bands(img, cell, |y0, rows| {
        let bh = rows.len() as u32 / (w * 4);
        let cy = y0 / cell;
        for cx in 0..w.div_ceil(cell) {
            let x0 = cx * cell;
            // Sample the cell's last pixel, like monocode.
            let o = (((cell - 1).min(bh - 1) * w + (x0 + cell - 1).min(w - 1)) * 4) as usize;
            let s = [rows[o], rows[o + 1], rows[o + 2]];
            let peak = s[0].max(s[1]).max(s[2]);
            let threshold = (BAYER[(cy % 4) as usize][(cx % 4) as usize] as f32 + 0.5) / 16.0;
            let gain = if peak as f32 / 255.0 > threshold { 255.0 / peak.max(1) as f32 } else { 0.08 };
            let px = s.map(|c| (c as f32 * gain).round().min(255.0) as u8);
            for y in 0..bh.min(h - y0) {
                for x in x0..(x0 + cell).min(w) {
                    let o = ((y * w + x) * 4) as usize;
                    rows[o..o + 3].copy_from_slice(&px);
                }
            }
        }
    });
}

/// Share of the texture in the result (monocode: 0.4). Stronger than upstream so it stays
/// readable at the 35% default opacity.
const TEXTURE: f32 = 0.7;

/// 5×7 glyphs in `6u × 8u` cells, the densest where the picture is brightest.
fn ascii(img: &mut RgbaImage, unit: u32) {
    const GLYPHS: [[u8; 7]; 10] = [
        [0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 4, 0],
        [0, 4, 0, 0, 4, 0, 0],
        [0, 0, 0, 14, 0, 0, 0],
        [0, 0, 14, 0, 14, 0, 0],
        [0, 4, 4, 31, 4, 4, 0],
        [0, 21, 14, 31, 14, 21, 0],
        [10, 10, 31, 10, 31, 10, 10],
        [17, 2, 4, 4, 8, 16, 17],
        [14, 17, 23, 21, 23, 16, 14],
    ];
    let (cw, ch) = (6 * unit, 8 * unit);
    let w = img.width();
    let (lo, span) = luma_range(img, cw, ch);
    par_bands(img, ch, |_, rows| {
        let bh = rows.len() as u32 / (w * 4);
        for cx in 0..w.div_ceil(cw) {
            let x0 = cx * cw;
            let o = ((((ch / 2).min(bh - 1)) * w + (x0 + cw / 2).min(w - 1)) * 4) as usize;
            let s = [rows[o], rows[o + 1], rows[o + 2]];
            // Brightness relative to this picture's own range (unlike monocode), so a dark
            // picture still gets its densest glyphs; and the ink lifted towards full brightness
            // (as dither does), so the glyphs read through the background's low opacity.
            let level = ((luma(&s) - lo) / span).clamp(0.0, 1.0);
            let glyph = &GLYPHS[((level.sqrt() * 9.0) as usize).min(9)];
            let peak = s[0].max(s[1]).max(s[2]).max(1) as f32;
            let gain = (210.0 / peak).clamp(1.0, 3.0);
            let s = s.map(|c| (c as f32 * gain).min(255.0) as u8);
            for y in 0..bh {
                let gy = y / unit;
                for x in x0..(x0 + cw).min(w) {
                    let gx = (x - x0) / unit;
                    let ink = gx < 5 && gy < 7 && glyph[gy as usize] & (1 << (4 - gx)) != 0;
                    let o = ((y * w + x) * 4) as usize;
                    for c in 0..3 {
                        rows[o + c] = mix(rows[o + c], if ink { s[c] as f32 } else { 0.0 }, TEXTURE);
                    }
                }
            }
        }
    });
}

/// The 2nd and 98th percentile of the picture's brightness, sampled at cell centres, as
/// (low, high - low). A picture with almost no range keeps the absolute scale.
fn luma_range(img: &RgbaImage, cw: u32, ch: u32) -> (f32, f32) {
    let (w, h) = img.dimensions();
    let mut l: Vec<f32> = Vec::with_capacity(((w / cw + 1) * (h / ch + 1)) as usize);
    for y in (ch / 2..h).step_by(ch as usize) {
        for x in (cw / 2..w).step_by(cw as usize) {
            l.push(luma(&img.get_pixel(x, y).0));
        }
    }
    if l.len() < 2 {
        return (0.0, 1.0);
    }
    l.sort_by(f32::total_cmp);
    let at = |q: f32| l[((l.len() - 1) as f32 * q) as usize];
    let (lo, hi) = (at(0.02), at(0.98));
    if hi - lo < 0.1 { (0.0, 1.0) } else { (lo, hi - lo) }
}

/// Dots on a `4u` grid, sized by brightness.
fn halftone(img: &mut RgbaImage, unit: u32) {
    let cell = 4 * unit;
    let w = img.width();
    let mid = (cell as f32 - 1.0) / 2.0;
    par_bands(img, cell, |_, rows| {
        let bh = rows.len() as u32 / (w * 4);
        for cx in 0..w.div_ceil(cell) {
            let x0 = cx * cell;
            let o = ((((cell / 2).min(bh - 1)) * w + (x0 + cell / 2).min(w - 1)) * 4) as usize;
            let s = [rows[o], rows[o + 1], rows[o + 2]];
            let radius = 2.0 * unit as f32 * (0.3 + 0.7 * luma(&s).sqrt());
            for y in 0..bh {
                for x in x0..(x0 + cell).min(w) {
                    let d = ((x - x0) as f32 - mid).hypot(y as f32 - mid);
                    let coverage = (radius + 0.5 - d).clamp(0.0, 1.0);
                    let o = ((y * w + x) * 4) as usize;
                    for c in 0..3 {
                        rows[o + c] = mix(rows[o + c], s[c] as f32 * coverage, TEXTURE);
                    }
                }
            }
        }
    });
}

/// Every third line (`unit` pixels tall) darkened.
fn scanlines(img: &mut RgbaImage, unit: u32) {
    let w = img.width() as usize;
    par_bands(img, 3 * unit, |y0, rows| {
        for (i, row) in rows.chunks_exact_mut(w * 4).enumerate() {
            if (y0 + i as u32) / unit % 3 == 0 {
                for p in row.chunks_exact_mut(4) {
                    for c in &mut p[..3] {
                        *c = (*c as f32 * 0.35).round() as u8;
                    }
                }
            }
        }
    });
}

/// Separable Gaussian blur, edges clamped.
fn gaussian(img: &mut RgbaImage, sigma: f32) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let r = (sigma * 3.0).ceil().max(1.0) as usize;
    let mut k: Vec<f32> = (0..=2 * r).map(|i| (-((i as f32 - r as f32).powi(2)) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    let src: Vec<f32> = img.as_raw().iter().map(|&v| v as f32).collect();
    let mut tmp = vec![0f32; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for (i, kv) in k.iter().enumerate() {
                let sx = (x + i).saturating_sub(r).min(w - 1);
                let o = (y * w + sx) * 4;
                for c in 0..3 {
                    acc[c] += src[o + c] * kv;
                }
            }
            tmp[(y * w + x) * 4..][..3].copy_from_slice(&acc);
        }
    }
    let raw = img.as_mut();
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for (i, kv) in k.iter().enumerate() {
                let sy = (y + i).saturating_sub(r).min(h - 1);
                let o = (sy * w + x) * 4;
                for c in 0..3 {
                    acc[c] += tmp[o + c] * kv;
                }
            }
            let o = (y * w + x) * 4;
            for c in 0..3 {
                raw[o + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
            raw[o + 3] = 255;
        }
    }
}

/// Uzumaki's built-in background picture, `width × height`: a coral (#ff7a8a) and violet swirl
/// on near black (#0e0e11), echoing the app icon. Deterministic (headless scenes use it).
pub fn builtin(width: u32, height: u32) -> RgbaImage {
    builtin_on(width, height, Ground::Dark)
}

/// `builtin` for a theme: on `Ground::Light` a deeper red (#d6304a, the light accent) and violet
/// (#7c5cf0) swirl on the light background (#fcfcfd). `Ground::Dark` is exactly `builtin`.
pub fn builtin_on(width: u32, height: u32, ground: Ground) -> RgbaImage {
    let (w, h) = (width.max(1), height.max(1));
    let (coral, violet, bg) = match ground {
        Ground::Dark => ([255.0, 122.0, 138.0], [167.0, 139.0, 250.0], BG),
        Ground::Light => ([214.0, 48.0, 74.0], [124.0, 92.0, 240.0], BG_LIGHT),
    };
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    // Shape is independent of the size: coordinates are relative to the half diagonal.
    let scale = (w as f32).hypot(h as f32) / 2.0;
    let (cx, cy) = (w as f32 * 0.6, h as f32 * 0.46);
    RgbaImage::from_fn(w, h, |x, y| {
        let (dx, dy) = ((x as f32 + 0.5 - cx) / scale, (y as f32 + 0.5 - cy) / scale);
        let r = dx.hypot(dy).max(1e-4);
        let theta = dy.atan2(dx);
        // Logarithmic spiral: bands are constant along `arms*theta/2π + k*ln r`.
        let phase = 2.0 * theta + 6.4 * r.ln();
        let band = 0.5 + 0.5 * phase.sin();
        let band = band * band * (3.0 - 2.0 * band);
        let fade = smooth(0.03, 0.28, r) * (1.0 - 0.55 * smooth(0.5, 1.1, r));
        let strength = 0.9 * band * fade;
        let hue = 0.5 + 0.5 * (theta + 2.2 * r.ln() - 0.8).sin();
        let hue = smooth(0.2, 0.8, hue);
        // Ordered noise of under one level hides 8-bit banding in the slow dark gradients.
        let n = (((x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663)).wrapping_mul(2654435761)) >> 24) as f32 / 255.0 - 0.5;
        let mut px = [0u8; 4];
        for c in 0..3 {
            let col = coral[c] * (1.0 - hue) + violet[c] * hue;
            let v = bg[c] + (col - bg[c]) * strength + n;
            px[c] = v.round().clamp(0.0, 255.0) as u8;
        }
        px[3] = 255;
        image::Rgba(px)
    })
}

/// The startup intro's swirl (`ui/intro.rs`): the built-in background's spiral and colours,
/// centred in a `size × size` square, on transparency. It fades out toward the middle (the
/// hole the app comes out of) and is fully clear at the inscribed circle, so it can turn and
/// grow without showing its corners. Straight-alpha RGBA bytes, as `rune::Pixels::set` takes.
/// Shown at the window's diagonal, its spiral matches `builtin`'s density.
pub fn intro_swirl(size: u32) -> Vec<u8> {
    const CORAL: [f32; 3] = [255.0, 122.0, 138.0];
    const VIOLET: [f32; 3] = [167.0, 139.0, 250.0];
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let n = size.max(2);
    let half = n as f32 / 2.0;
    let mut img = RgbaImage::new(n, n);
    par_bands(&mut img, 32, |y0, rows| {
        for (i, px) in rows.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i as u32 % n) as f32, (y0 + i as u32 / n) as f32);
            let (dx, dy) = ((x + 0.5 - half) / half, (y + 0.5 - half) / half);
            let r = dx.hypot(dy).max(1e-4);
            let theta = dy.atan2(dx);
            let phase = 2.0 * theta + 6.4 * r.ln();
            let band = 0.5 + 0.5 * phase.sin();
            let band = band * band * (3.0 - 2.0 * band);
            let fade = smooth(0.04, 0.34, r) * (1.0 - smooth(0.55, 1.0, r));
            let hue = smooth(0.2, 0.8, 0.5 + 0.5 * (theta + 2.2 * r.ln() - 0.8).sin());
            for c in 0..3 {
                px[c] = (CORAL[c] * (1.0 - hue) + VIOLET[c] * hue).round() as u8;
            }
            px[3] = (255.0 * 0.88 * band * fade).round() as u8;
        }
    });
    img.into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intro_swirl_is_clear_at_the_centre_and_corners() {
        let n = 64;
        let px = intro_swirl(n);
        assert_eq!(px.len(), (n * n * 4) as usize);
        let alpha = |x: u32, y: u32| px[((y * n + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "corner");
        assert_eq!(alpha(n / 2, n / 2), 0, "centre");
        assert!((0..n).any(|x| alpha(x, n / 4) > 100), "the arms show");
    }

    fn photo(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, ((x ^ y) & 255) as u8, 255]))
    }
    fn t(width: u32, height: u32, unit: u32) -> Target {
        Target { width, height, unit }
    }

    #[test]
    fn exact_size_except_blur() {
        let src = photo(300, 200);
        for e in Effect::ALL.into_iter().filter(|e| *e != Effect::Blur) {
            for (w, h, u) in [(97, 61, 1), (200, 300, 2), (13, 5, 3)] {
                assert_eq!(render(&src, e, t(w, h, u)).dimensions(), (w, h), "{e:?}");
            }
        }
    }

    #[test]
    fn blur_is_smaller_with_same_aspect() {
        let img = render(&photo(300, 200), Effect::Blur, t(1360, 860, 1));
        let (w, h) = img.dimensions();
        assert!(w < 1360 && h < 860 && w.max(h) <= 256);
        assert!((w as f32 / h as f32 - 1360.0 / 860.0).abs() < 0.02, "{w}x{h}");
        // Small targets are not upscaled.
        assert_eq!(render(&photo(30, 20), Effect::Blur, t(40, 30, 1)).dimensions(), (40, 30));
    }

    #[test]
    fn zero_target_is_one_pixel() {
        for e in Effect::ALL {
            assert_eq!(render(&photo(10, 10), e, t(0, 0, 1)).dimensions(), (1, 1), "{e:?}");
            assert_eq!(render(&photo(10, 10), e, t(0, 7, 0)).dimensions(), (1, 7), "{e:?}");
        }
    }

    #[test]
    fn builtin_is_deterministic_and_opaque() {
        assert_eq!(builtin(120, 80), builtin(120, 80));
        assert!(builtin(120, 80).pixels().all(|p| p.0[3] == 255));
        assert_eq!(builtin(0, 0).dimensions(), (1, 1));
    }

    #[test]
    fn dither_cells_are_uniform_blocks() {
        for unit in [1, 2, 3] {
            let cell = 2 * unit;
            let img = render(&photo(300, 200), Effect::Dither, t(101, 67, unit));
            let (w, h) = img.dimensions();
            for y in 0..h {
                for x in 0..w {
                    let (ox, oy) = (x / cell * cell, y / cell * cell);
                    assert_eq!(img.get_pixel(x, y), img.get_pixel(ox, oy));
                }
            }
        }
    }

    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf29ce484222325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100000001b3))
    }
    /// A picture with a gradient, a pattern and a half-transparent strip (flattened onto the ground).
    fn golden_src() -> RgbaImage {
        let mut src = RgbaImage::from_fn(300, 200, |x, y| {
            image::Rgba([(x * 255 / 300) as u8, (y * 255 / 200) as u8, ((x ^ y) & 255) as u8, if x < 20 { 120 } else { 255 }])
        });
        src.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        src
    }

    /// The dark output is byte for byte what it was before the light ground existed (hashes taken
    /// from the code at light-theme 7ac798b), through both `render` and `render_on(.., Dark)`.
    #[test]
    fn dark_output_is_unchanged() {
        let want = [
            (Effect::Plain, 1, 0x5c267d3230acd989u64),
            (Effect::Plain, 2, 0x5c267d3230acd989),
            (Effect::Dither, 1, 0x1a09aacd1510ea7d),
            (Effect::Dither, 2, 0xe7d09d8b940bc7e9),
            (Effect::Ascii, 1, 0x16a7436168abf256),
            (Effect::Ascii, 2, 0x28f532f25174147c),
            (Effect::Halftone, 1, 0x9b7a3952c04963bc),
            (Effect::Halftone, 2, 0x750b25bceb325cc3),
            (Effect::Scanlines, 1, 0xcd3f0eb863f7461f),
            (Effect::Scanlines, 2, 0xf800ce69675146bd),
            (Effect::Blur, 1, 0x22b5b4f46762fbff),
            (Effect::Blur, 2, 0x22b5b4f46762fbff),
        ];
        let src = golden_src();
        for (e, unit, hash) in want {
            let target = t(173, 111, unit);
            assert_eq!(fnv(render(&src, e, target).as_raw()), hash, "{e:?} unit {unit}");
            assert_eq!(fnv(render_on(&src, e, target, Ground::Dark).as_raw()), hash, "{e:?} unit {unit}");
        }
        assert_eq!(fnv(builtin(173, 111).as_raw()), 0xf9b16c72834b5afa);
        assert_eq!(fnv(builtin_on(173, 111, Ground::Dark).as_raw()), 0xf9b16c72834b5afa);
        assert_eq!(fnv(&intro_swirl(64)), 0x59efac148d494477);
    }

    fn mean_luma(img: &RgbaImage) -> f32 {
        img.pixels().map(|p| luma(&p.0)).sum::<f32>() / (img.width() * img.height()) as f32
    }

    /// On the light ground the effects are ink on white: mostly bright, with the dark parts of the
    /// picture still showing; transparency flattens onto the light bg.
    #[test]
    fn light_ground_is_ink_on_white() {
        let src = golden_src();
        let target = t(173, 111, 1);
        let pale = RgbaImage::from_pixel(50, 50, image::Rgba([240, 236, 230, 255]));
        let black = RgbaImage::from_pixel(50, 50, image::Rgba([10, 10, 10, 255]));
        let floor = LIGHT_FLOOR as f32 / 255.0;
        for e in [Effect::Dither, Effect::Ascii, Effect::Halftone, Effect::Scanlines] {
            let light = render_on(&src, e, target, Ground::Light);
            assert!(light.pixels().all(|p| p.0[3] == 255), "{e:?} opaque");
            assert!(light.pixels().any(|p| luma(&p.0) < floor + 0.1), "{e:?} has ink");
            // A pale picture is (nearly) blank paper.
            assert!(mean_luma(&render_on(&pale, e, target, Ground::Light)) > 0.85, "{e:?} pale on light");
            // A black picture is inked, down to the floor.
            assert!(mean_luma(&render_on(&black, e, target, Ground::Light)) < 0.85, "{e:?} black on light");
        }
        // Dither: every cell is either the picture's colour at full strength (down to the floor)
        // or near-white paper.
        let light = render_on(&src, Effect::Dither, target, Ground::Light);
        assert!(light.pixels().all(|p| p.0[..3].iter().min() == Some(&LIGHT_FLOOR) || p.0[..3].iter().all(|&c| c >= 230)));
        let clear = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 0]));
        // (The light bg #fcfcfd, printed pale like everything on light: a hair lighter.)
        assert_eq!(render_on(&clear, Effect::Plain, t(4, 4, 1), Ground::Light).get_pixel(1, 1).0, [253, 253, 254, 255]);
        // The light swirl is on the light background.
        let b = builtin_on(120, 80, Ground::Light);
        assert!(mean_luma(&b) > 0.6 && b.pixels().all(|p| p.0[3] == 255));
        assert_eq!(b, builtin_on(120, 80, Ground::Light));
    }

    /// On light every effect, Plain and Blur included, is printed pale: no channel below
    /// `LIGHT_FLOOR` (text sits on it), the picture's tones kept in order. Plain is the picture
    /// mapped onto `LIGHT_FLOOR..=255`.
    #[test]
    fn light_ground_never_goes_below_the_floor() {
        let src = golden_src();
        let black = RgbaImage::from_pixel(60, 40, image::Rgba([0, 0, 0, 255]));
        for e in Effect::ALL {
            for img in [&src, &black] {
                let light = render_on(img, e, t(173, 111, 1), Ground::Light);
                assert!(light.pixels().all(|p| p.0[..3].iter().all(|&c| c >= LIGHT_FLOOR)), "{e:?}");
            }
        }
        let opaque = photo(300, 200);
        let (dark, light) = (render(&opaque, Effect::Plain, t(173, 111, 1)), render_on(&opaque, Effect::Plain, t(173, 111, 1), Ground::Light));
        let k = (255 - LIGHT_FLOOR) as f32 / 255.0;
        for (d, l) in dark.pixels().zip(light.pixels()) {
            for c in 0..3 {
                assert_eq!(l.0[c], 255 - ((255 - d.0[c]) as f32 * k).round() as u8);
            }
        }
    }

    /// A pale picture with some range (white to pastel) still prints visible ink on light: its
    /// palest-to-darkest range is deepened. Dark is untouched (see `dark_output_is_unchanged`).
    #[test]
    fn a_pale_picture_prints_visible_ink_on_light() {
        let pale = RgbaImage::from_fn(200, 120, |x, _| {
            let v = 250 - (x * 50 / 200) as u8;
            image::Rgba([v, v, 255, 255])
        });
        let target = t(173, 111, 1);
        for e in [Effect::Dither, Effect::Ascii, Effect::Halftone, Effect::Scanlines] {
            let light = render_on(&pale, e, target, Ground::Light);
            let darkest = light.pixels().map(|p| luma(&p.0)).fold(1.0f32, f32::min);
            assert!(darkest < 0.75, "{e:?}: darkest {darkest}");
            assert!(mean_luma(&light) > 0.6, "{e:?}: still mostly paper");
        }
        // A picture that already has dark parts is not deepened.
        let mut inv = cover(&golden_src(), 173, 111, BG_LIGHT);
        invert(&mut inv);
        let before = inv.clone();
        deepen_ink(&mut inv);
        assert_eq!(inv, before);
    }

    #[test]
    fn outputs_are_opaque() {
        let mut src = photo(64, 64);
        src.pixels_mut().for_each(|p| p.0[3] = 90);
        for e in Effect::ALL {
            assert!(render(&src, e, t(80, 50, 1)).pixels().all(|p| p.0[3] == 255), "{e:?}");
        }
    }
}

