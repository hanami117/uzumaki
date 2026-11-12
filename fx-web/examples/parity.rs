//! Native half of the wasm parity check: prints an FNV-1a hash of every effect's output for a
//! synthetic picture, for `fx-web/parity.mjs` to compare with the wasm build's.
//! `cargo run -p uzumaki-fx-web --example parity`
use image::RgbaImage;
use uzumaki_fx::{Effect, Ground, Target, render_on};

fn main() {
    let src = RgbaImage::from_fn(600, 600, |x, y| {
        image::Rgba([(x * 255 / 600) as u8, (y * 255 / 600) as u8, ((x ^ y) & 255) as u8, 255])
    });
    for (i, e) in Effect::ALL.into_iter().enumerate() {
        for (w, h, unit) in [(1360, 860, 1), (700, 500, 2)] {
            let out = render_on(&src, e, Target { width: w, height: h, unit }, Ground::Dark);
            let hash = out.as_raw().iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3));
            println!("{i} {w}x{h}@{unit} -> {}x{} {hash:016x}", out.width(), out.height());
        }
    }
}
