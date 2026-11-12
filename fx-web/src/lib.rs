//! The app's background effects (`uzumaki-fx`) for the website's demo, as a WebAssembly module
//! with a plain C ABI: no wasm-bindgen, no threads (`uzumaki-fx` runs its bands on the calling
//! thread when it has one core, which is what `std::thread::available_parallelism` reports on
//! wasm32), no JS glue beyond reading and writing linear memory.
//!
//! Build: `cargo build -p uzumaki-fx-web --target wasm32-unknown-unknown --profile web`
//! (`docs/ROADMAP.md`, "fx-web", has the whole recipe).
//!
//! Usage from JS (all numbers are u32/i32):
//! ```text
//! src = fx_alloc(sw*sh*4);            // write the decoded RGBA picture into memory at `src`
//! out = fx_render(src, sw, sh, w, h, unit, effect, ground); // consumes `src`; 0 on bad input
//! ow = fx_out_width(); oh = fx_out_height();                 // Blur is smaller than w×h
//! ...read ow*oh*4 bytes at `out`...
//! fx_free(out, ow*oh*4);
//! ```
//! Memory may grow during `fx_render`: take a fresh `memory.buffer` view after it.
//!
//! What the app does (`app/src/backdrop.rs::render_request`, `app/src/ui/backdrop.rs`), which
//! the site mirrors:
//! - the picture is the cover at `NOW_PLAYING_PX`, rendered with `uzumaki_fx::render_on(&src,
//!   effect, Target { width, height, unit }, ground)`, where width×height is the element in
//!   device pixels and `unit` = the display scale rounded (at least 1). Dark theme = `ground` 0.
//!   Nothing else happens in this function: no darkening or tint is baked into the pixels.
//! - the look is composed on top by the view: the content-area wallpaper is drawn at opacity
//!   0.35 (`Appearance::default().opacity`) over the app background `#0e0e11`, fading out
//!   towards the bottom; Now Playing's "Cover" mode draws it at opacity 1.0 under a black
//!   scrim, 0.5 at the top to 0.72 at the bottom ("Same as app" uses 0.32 to 0.46 over the
//!   app's 0.35 picture). Those are CSS in the site.
//! - effects (`Effect::ALL` order, the ids here): 0 None, 1 Dither, 2 ASCII, 3 Halftone,
//!   4 Scanlines, 5 Blur. The app defaults are None for the content area and Blur for Now
//!   Playing; the site's default is Halftone.

use std::sync::atomic::{AtomicU32, Ordering};

use image::RgbaImage;
use uzumaki_fx::{Effect, Ground, Target};

static OUT_W: AtomicU32 = AtomicU32::new(0);
static OUT_H: AtomicU32 = AtomicU32::new(0);

/// `len` bytes of linear memory for the caller to fill or read. 0 when `len` is 0.
#[unsafe(no_mangle)]
pub extern "C" fn fx_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let mut v: Vec<u8> = Vec::with_capacity(len);
    // Zeroed, so the buffer is a fully initialised `Vec<u8>` of `len` when it comes back.
    v.resize(len, 0);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Frees a buffer from `fx_alloc` or `fx_render`; `len` is the length it was made with.
///
/// # Safety
/// `ptr` must come from `fx_alloc(len)` / `fx_render` (with the output length) and not be freed twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fx_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        drop(unsafe { Vec::from_raw_parts(ptr, len, len) });
    }
}

/// Width of the last `fx_render` result.
#[unsafe(no_mangle)]
pub extern "C" fn fx_out_width() -> u32 {
    OUT_W.load(Ordering::Relaxed)
}

/// Height of the last `fx_render` result.
#[unsafe(no_mangle)]
pub extern "C" fn fx_out_height() -> u32 {
    OUT_H.load(Ordering::Relaxed)
}

/// Number of effects (ids `0..count`).
#[unsafe(no_mangle)]
pub extern "C" fn fx_effect_count() -> u32 {
    Effect::ALL.len() as u32
}

/// Renders the straight-alpha RGBA picture at `src` (`sw × sh`, from `fx_alloc(sw*sh*4)`;
/// **consumed**, don't free it) as `effect` for an element of `w × h` device pixels at `unit`
/// device pixels per point, on `ground` (0 dark, 1 light). Returns a new `fx_alloc`-style buffer
/// of `fx_out_width() × fx_out_height()` RGBA pixels (free it with `fx_free`), or null when an
/// argument is invalid (the source is still consumed).
///
/// # Safety
/// `src` must come from `fx_alloc(sw*sh*4)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fx_render(src: *mut u8, sw: u32, sh: u32, w: u32, h: u32, unit: u32, effect: u32, ground: u32) -> *mut u8 {
    let len = (sw as usize).saturating_mul(sh as usize).saturating_mul(4);
    if src.is_null() || len == 0 {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { Vec::from_raw_parts(src, len, len) };
    let Some(effect) = Effect::ALL.get(effect as usize).copied() else {
        return std::ptr::null_mut();
    };
    let Some(src) = RgbaImage::from_raw(sw, sh, bytes) else {
        return std::ptr::null_mut();
    };
    let ground = if ground == 0 { Ground::Dark } else { Ground::Light };
    let out = uzumaki_fx::render_on(&src, effect, Target { width: w, height: h, unit }, ground);
    drop(src);
    OUT_W.store(out.width(), Ordering::Relaxed);
    OUT_H.store(out.height(), Ordering::Relaxed);
    let mut raw = out.into_raw();
    raw.shrink_to_fit();
    let p = raw.as_mut_ptr();
    std::mem::forget(raw);
    p
}
