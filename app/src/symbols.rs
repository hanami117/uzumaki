//! SF Symbols for the AirPlay picker (macOS), as Apple's own picker draws them.
//!
//! `NSImage imageWithSystemSymbolName:` + `NSImageSymbolConfiguration` give the glyph; it is drawn
//! once at 2x into straight-alpha RGBA (`rune::Pixels`, the technique of `share_mac.rs` on
//! `copy-link`) and tinted with the colour it is wanted in (Pixels have no tint of their own).
//! Rasterising happens in `warm`, on the main thread a moment after start, for every palette's
//! colours; `get` only reads the cache, so the view and poll never touch AppKit. A symbol not
//! cached yet (or any symbol off the Mac) is None: the caller keeps its line icon.

use rune::{Color, Pixels};

/// The symbols the picker uses.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sym {
    AirPlay,
    HomePod,
    HomePodMini,
    HomePodPair,
    AppleTv,
    Speaker,
    Tv,
    Computer,
}

impl Sym {
    #[cfg(target_os = "macos")]
    const ALL: [Sym; 8] = [Sym::AirPlay, Sym::HomePod, Sym::HomePodMini, Sym::HomePodPair, Sym::AppleTv, Sym::Speaker, Sym::Tv, Sym::Computer];

    #[cfg(any(target_os = "macos", test))]
    fn name(self) -> &'static str {
        match self {
            Sym::AirPlay => "airplayaudio",
            Sym::HomePod => "homepod.fill",
            Sym::HomePodMini => "homepodmini.fill",
            Sym::HomePodPair => "homepod.2.fill",
            Sym::AppleTv => "appletv.fill",
            Sym::Speaker => "hifispeaker.fill",
            Sym::Tv => "tv",
            Sym::Computer => "laptopcomputer",
        }
    }
}

/// Point size of the glyph and side of the square it is centred in (points).
#[cfg(target_os = "macos")]
pub const GLYPH: f32 = 17.0;
pub const CANVAS: f32 = 26.0;

#[cfg(not(target_os = "macos"))]
pub fn warm() {}
#[cfg(not(target_os = "macos"))]
pub fn load_now() {}
#[cfg(not(target_os = "macos"))]
pub fn get(_sym: Sym, _color: Color) -> Option<Pixels> {
    None
}

/// The cached glyph in `color`, if rasterised.
#[cfg(target_os = "macos")]
pub fn get(sym: Sym, color: Color) -> Option<Pixels> {
    mac::CACHE.with(|c| c.borrow().get(&(sym, color.hex())).cloned())
}

/// Rasterises every symbol in every palette colour, once, on the main thread, soon after start.
#[cfg(target_os = "macos")]
pub fn warm() {
    let Ok(when) = dispatch2::DispatchTime::try_from(std::time::Duration::from_millis(300)) else { return };
    let _ = dispatch2::DispatchQueue::main().after(when, mac::load);
}

/// Rasterises now (headless scenes run before the delayed `warm`).
#[cfg(target_os = "macos")]
pub fn load_now() {
    mac::load();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, MainThreadMarker};
    use objc2_app_kit::{
        NSCompositingOperation, NSImage,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use rune::{Color, Pixels};

    use super::{Sym, CANVAS, GLYPH};

    thread_local! {
        pub static CACHE: RefCell<HashMap<(Sym, String), Pixels>> = RefCell::new(HashMap::new());
    }

    pub fn load() {
        if MainThreadMarker::new().is_none() || CACHE.with(|c| !c.borrow().is_empty()) {
            return;
        }
        let started = std::time::Instant::now();
        let mut done = HashMap::new();
        for color in crate::theme::symbol_colors() {
            for sym in Sym::ALL {
                let key = (sym, color.hex());
                if !done.contains_key(&key)
                    && let Some(px) = raster(sym.name(), color)
                {
                    done.insert(key, px);
                }
            }
        }
        crate::framelog::event(|| format!("symbols: {} glyphs in {:?}", done.len(), started.elapsed()));
        CACHE.with(|c| *c.borrow_mut() = done);
    }

    /// `name` at `GLYPH` points, 2x, centred in a `CANVAS` square, filled with `color`.
    fn raster(name: &str, color: Color) -> Option<Pixels> {
        let side = (CANVAS * 2.0) as isize;
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
        // Point size and weight as the UI's text (medium, 2x); the symbol's own scale: medium.
        // SAFETY: NSImageSymbolConfiguration's class method, then NSImage's `imageWithSymbolConfiguration:`.
        let image: Retained<NSImage> = unsafe {
            let cfg: Retained<AnyObject> =
                msg_send![class!(NSImageSymbolConfiguration), configurationWithPointSize: f64::from(GLYPH * 2.0), weight: 0.23f64, scale: 2isize];
            msg_send![&*image, imageWithSymbolConfiguration: &*cfg]
        };
        let size = image.size();
        let mut rgba = crate::raster_mac::draw_rgba(side, || {
            let at = NSPoint::new((side as f64 - size.width) / 2.0, (side as f64 - size.height) / 2.0);
            image.drawInRect_fromRect_operation_fraction(NSRect::new(at, NSSize::new(size.width, size.height)), NSRect::ZERO, NSCompositingOperation::SourceOver, 1.0);
        })?;
        tint(&mut rgba, color);
        let px = Pixels::new();
        px.set(side as u32, side as u32, rgba);
        Some(px)
    }

    /// Keeps the drawing's coverage (alpha) and paints it `color`, as straight alpha.
    pub(super) fn tint(rgba: &mut [u8], color: Color) {
        let hex = color.hex();
        let c = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(255);
        let (r, g, b, a) = (c(1), c(3), c(5), u32::from(c(7)));
        for p in rgba.as_chunks_mut::<4>().0 {
            let cover = u32::from(p[3]);
            *p = [r, g, b, ((cover * a + 127) / 255) as u8];
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        assert_eq!(super::Sym::HomePodPair.name(), "homepod.2.fill");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tint_keeps_coverage() {
        let mut px = vec![0, 0, 0, 128, 9, 9, 9, 0];
        super::mac::tint(&mut px, rune::color::hex("#ff0000").unwrap());
        assert_eq!(px, [255, 0, 0, 128, 255, 0, 0, 0]);
    }
}
