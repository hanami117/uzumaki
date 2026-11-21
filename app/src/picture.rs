//! Decoding the pictures the user picks: playlist covers and the background. PNG, JPEG and JPEG XL
//! (`.jxl`).
//!
//! Nothing is kept as JPEG XL: a playlist picture is decoded here, cropped, scaled and saved as a
//! JPEG (`state::playlist_edit::save_picture`), and a background is decoded into pixels
//! (`backdrop::load`), so drawing never needs a JXL decoder. The decoder is `jxl-oxide`, pure Rust
//! (no C library, no system package).

use image::{DynamicImage, ImageDecoder, ImageReader};

/// What the file pickers offer. The decoders are in the `image` crate (png, jpeg) and
/// `jxl-oxide`.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "jxl"];

/// Beyond this a JPEG XL is refused before it is decoded: the render holds several `f32`
/// channels, so a small file could ask for gigabytes.
const JXL_MAX_PIXELS: u64 = 64_000_000;
/// Memory `jxl-oxide` may allocate for one picture.
const JXL_MAX_ALLOC: usize = 1 << 30;

/// True for a bare JPEG XL codestream (`FF 0A`) and for the ISO box container (`JXL `).
pub fn is_jxl(bytes: &[u8]) -> bool {
    const CONTAINER: [u8; 12] = [0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A];
    bytes.starts_with(&[0xFF, 0x0A]) || bytes.starts_with(&CONTAINER)
}

/// Decodes `bytes` whatever the format (found from the content, not the name). `upright` turns
/// the picture the way its EXIF orientation says; a JPEG XL is always upright (its own
/// orientation is applied by the decoder). An animated JPEG XL gives its first frame.
pub fn decode(bytes: &[u8], upright: bool) -> Result<DynamicImage, String> {
    if is_jxl(bytes) {
        return decode_jxl(bytes);
    }
    if !upright {
        return image::load_from_memory(bytes).map_err(|e| format!("not a readable picture ({e})"));
    }
    let unreadable = |e: &dyn std::fmt::Display| format!("not a readable picture ({e})");
    let mut decoder = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| unreadable(&e))?
        .into_decoder()
        .map_err(|e| unreadable(&e))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| unreadable(&e))?;
    img.apply_orientation(orientation); // phone photos are often stored sideways
    Ok(img)
}

/// First frame of a JPEG XL, as 8-bit sRGB (or grey) with alpha if it has one.
fn decode_jxl(bytes: &[u8]) -> Result<DynamicImage, String> {
    use jxl_oxide::{AllocTracker, EnumColourEncoding, JxlImage, RenderingIntent};

    let unreadable = |e: &dyn std::fmt::Display| format!("not a readable JPEG XL picture ({e})");
    let mut image = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(JXL_MAX_ALLOC))
        .read(bytes)
        .map_err(|e| unreadable(&e))?;
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return Err("the picture is empty".into());
    }
    if u64::from(w) * u64::from(h) > JXL_MAX_PIXELS {
        return Err(format!("the picture is too big ({w}x{h})"));
    }
    let format = image.pixel_format();
    if format.has_black() {
        return Err("CMYK JPEG XL pictures aren't supported".into());
    }
    if !format.is_grayscale() {
        // Wide-gamut and HDR files are brought to sRGB, which is what the screen draws.
        image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    }
    let render = image.render_frame(0).map_err(|e| unreadable(&e))?; // first frame only
    let mut stream = render.stream();
    let (sw, sh, channels) = (stream.width(), stream.height(), stream.channels());
    let mut pixels = vec![0u8; sw as usize * sh as usize * channels as usize];
    stream.write_to_buffer(&mut pixels);
    let wrong = || "the decoder returned an unexpected picture".to_string();
    match channels {
        1 => image::GrayImage::from_raw(sw, sh, pixels).map(DynamicImage::ImageLuma8),
        2 => image::GrayAlphaImage::from_raw(sw, sh, pixels).map(DynamicImage::ImageLumaA8),
        3 => image::RgbImage::from_raw(sw, sh, pixels).map(DynamicImage::ImageRgb8),
        4 => image::RgbaImage::from_raw(sw, sh, pixels).map(DynamicImage::ImageRgba8),
        _ => None,
    }
    .ok_or_else(wrong)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with libjxl's `cjxl` from synthetic pictures; see fixtures/pictures/README.md.
    const TWO: &[u8] = include_bytes!("../fixtures/pictures/two-24x16.jxl");
    const GRAD: &[u8] = include_bytes!("../fixtures/pictures/gradient-48x32.jxl");
    const GRAY: &[u8] = include_bytes!("../fixtures/pictures/gray-16x16.jxl");
    const ALPHA: &[u8] = include_bytes!("../fixtures/pictures/alpha-16x16.jxl");

    #[test]
    fn the_pickers_offer_jxl() {
        for ext in ["png", "jpg", "jpeg", "jxl"] {
            assert!(EXTENSIONS.contains(&ext), "{ext}");
        }
    }

    #[test]
    fn a_jxl_is_recognised_by_its_content() {
        assert!(is_jxl(TWO) && is_jxl(GRAD) && is_jxl(GRAY) && is_jxl(ALPHA));
        assert!(!is_jxl(b"\x89PNG\r\n\x1a\n") && !is_jxl(b"\xff\xd8\xff\xe0") && !is_jxl(b""));
    }

    #[test]
    fn a_lossless_jxl_decodes_to_the_same_pixels() {
        let img = decode(TWO, true).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (24, 16));
        assert_eq!(img.get_pixel(2, 8).0, [255, 0, 0], "left half is red");
        assert_eq!(img.get_pixel(21, 8).0, [0, 0, 255], "right half is blue");
        // The same through the app's non-upright path (the background).
        assert_eq!(decode(TWO, false).unwrap().to_rgb8(), img);
    }

    #[test]
    fn a_lossy_gradient_stays_close_to_its_colours() {
        let img = decode(GRAD, true).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (48, 32));
        let (top, bottom) = (img.get_pixel(24, 1).0, img.get_pixel(24, 30).0);
        assert!(top[0] > 220 && top[2] < 90, "reddish at the top: {top:?}");
        assert!(bottom[2] > 200 && bottom[0] < 90, "bluish at the bottom: {bottom:?}");
    }

    #[test]
    fn grey_and_transparent_jxl_pictures_decode() {
        let gray = decode(GRAY, true).unwrap();
        assert_eq!((gray.width(), gray.height()), (16, 16));
        let (a, b) = (gray.to_luma8().get_pixel(8, 1).0[0], gray.to_luma8().get_pixel(8, 14).0[0]);
        assert!(a > b + 150, "white fades to black: {a} {b}");
        let alpha = decode(ALPHA, true).unwrap();
        assert!(alpha.color().has_alpha());
        let rgba = alpha.to_rgba8();
        assert_eq!(rgba.get_pixel(2, 8).0, [255, 0, 0, 255]);
        assert_eq!(rgba.get_pixel(13, 8).0[3], 0, "the right half is transparent");
    }

    #[test]
    fn broken_or_cut_jxl_files_are_errors_not_panics() {
        assert!(decode(&TWO[..TWO.len() / 2], true).is_err());
        assert!(decode(&GRAD[..20], false).is_err());
        assert!(decode(b"\xff\x0a not really", true).is_err());
        assert!(decode(b"not an image", true).is_err());
    }

    #[test]
    fn other_formats_still_decode() {
        let mut png = Vec::new();
        let src = image::RgbImage::from_pixel(5, 3, image::Rgb([9, 99, 199]));
        DynamicImage::ImageRgb8(src.clone()).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        assert_eq!(decode(&png, true).unwrap().to_rgb8(), src);
        assert_eq!(decode(&png, false).unwrap().to_rgb8(), src);
    }
}
