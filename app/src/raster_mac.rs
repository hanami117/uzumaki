//! Draws with AppKit into a square 8-bit RGBA bitmap and hands back its bytes (macOS).
//! Shared by `share_mac.rs` (sharing service icons) and `symbols.rs` (SF Symbols).

use objc2::AnyThread;
use objc2_app_kit::{NSBitmapFormat, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext};

/// A transparent `side` x `side` bitmap, `draw` run with it as the current graphics context, then
/// its bytes: premultiplied RGBA, rows packed. Main thread only (AppKit drawing).
pub fn draw_rgba(side: isize, draw: impl FnOnce()) -> Option<Vec<u8>> {
    // SAFETY: a fresh bitmap with its own buffer (null planes), 8-bit RGBA, rows packed.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            side * 4,
            32,
        )
    }?;
    let len = (side * side * 4) as usize;
    let data = rep.bitmapData();
    if data.is_null() {
        return None;
    }
    // SAFETY: the rep's own buffer, `len` long, alive while `rep` is.
    unsafe { std::ptr::write_bytes(data, 0, len) };
    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    draw();
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    // SAFETY: as above.
    Some(unsafe { std::slice::from_raw_parts(data, len) }.to_vec())
}
