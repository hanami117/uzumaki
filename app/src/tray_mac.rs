//! The macOS menu bar icon: an `NSStatusItem` with Play/Pause, Previous, Next, Show and Quit,
//! the current song as a disabled first item and as the tooltip. A click opens the menu, as
//! for every menu bar item. Rune has no Mac tray, so this talks to AppKit itself.
//!
//! AppKit objects live on the main thread. `TrayCtl` is built and used from the UI thread (the
//! state's init and its update handlers run there, and Rune's UI thread is the main thread on
//! macOS); `MainThreadMarker::new()` fails anywhere else and `start` then returns None. Menu
//! actions go to the app through the handle, like the Linux tray.
//!
//! The icon is always a template image: black with alpha, derived from the app's tray swirl, so
//! macOS tints it for light and dark menu bars. It does not follow Settings → Appearance → App
//! icon (the menu bar keeps the plain swirl); the Dock and the bundle do.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{NSData, NSObject, NSSize, NSString};

use crate::models::Song;
use crate::settings::IconVariant;
use crate::state::{Handle, State};

/// Menu bar icon height in points; the image is twice that in pixels for Retina.
const ICON_PT: f64 = 18.0;
const ICON_PX: u32 = 36;

const PLAY_PAUSE: isize = 1;
const PREVIOUS: isize = 2;
const NEXT: isize = 3;
const SHOW: isize = 4;
const QUIT: isize = 5;
const CHECK_UPDATES: isize = 6;

struct Ivars {
    handle: Handle,
}

define_class!(
    // The target of every menu item; the item's tag says which one was chosen.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UzumakiTrayTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(trayAction:))]
        fn tray_action(&self, item: &NSMenuItem) {
            let handle = &self.ivars().handle;
            match item.tag() {
                PLAY_PAUSE => handle.update(|s: &mut State, _| s.toggle_play()),
                PREVIOUS => handle.update(|s: &mut State, _| s.previous()),
                NEXT => handle.update(|s: &mut State, _| s.next()),
                SHOW => super::show_window(handle),
                // The answer shows in the window (the notice, or a toast).
                CHECK_UPDATES => {
                    super::show_window(handle);
                    handle.update(|s: &mut State, _| s.check_updates_now());
                }
                QUIT => handle.quit(),
                _ => {}
            }
        }
    }
);

pub struct TrayCtl {
    item: Retained<NSStatusItem>,
    target: Retained<Target>,
    mtm: MainThreadMarker,
    playing: bool,
    /// "Title — Artist" or empty.
    song: String,
}

impl TrayCtl {
    /// None when not on the main thread (AppKit would crash there).
    pub fn start(handle: Handle, app_icon: IconVariant) -> Option<Self> {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("uzumaki: no menu bar icon: not on the main thread");
            return None;
        };
        let target = mtm.alloc::<Target>().set_ivars(Ivars { handle });
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            button.setToolTip(Some(&NSString::from_str("Uzumaki")));
        }
        let mut ctl = Self { item, target, mtm, playing: false, song: String::new() };
        ctl.set_app_icon(app_icon);
        ctl.rebuild();
        Some(ctl)
    }

    /// Shows the template swirl. The app icon choice is ignored here (see the module docs).
    pub fn set_app_icon(&mut self, _icon: IconVariant) {
        let Some(button) = self.item.button(self.mtm) else { return };
        if let Some(image) = template_image() {
            button.setTitle(&NSString::new());
            button.setImage(Some(&image));
        } else {
            button.setTitle(&NSString::from_str("U")); // the icon failed to decode
        }
    }

    /// Relabels Play/Pause.
    pub fn set_playing(&mut self, playing: bool) {
        if playing != self.playing {
            self.playing = playing;
            self.rebuild();
        }
    }

    /// The first menu item and the tooltip: "Title — Artist", or just "Uzumaki".
    pub fn set_song(&mut self, song: Option<&Song>) {
        let text = match song {
            Some(s) if !s.artist.is_empty() => format!("{} — {}", s.title, s.artist),
            Some(s) => s.title.clone(),
            None => String::new(),
        };
        if text != self.song {
            if let Some(button) = self.item.button(self.mtm) {
                let tip = if text.is_empty() { "Uzumaki" } else { &text };
                button.setToolTip(Some(&NSString::from_str(tip)));
            }
            self.song = text;
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let menu = NSMenu::new(self.mtm);
        // Without this AppKit would grey the song line and enable the rest by itself; be explicit.
        menu.setAutoenablesItems(false);
        if !self.song.is_empty() {
            let title = NSMenuItem::new(self.mtm);
            title.setTitle(&NSString::from_str(&self.song));
            title.setEnabled(false);
            menu.addItem(&title);
            menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        }
        self.add(&menu, if self.playing { "Pause" } else { "Play" }, PLAY_PAUSE);
        self.add(&menu, "Previous", PREVIOUS);
        self.add(&menu, "Next", NEXT);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        self.add(&menu, "Show Uzumaki", SHOW);
        self.add(&menu, "Check for Updates\u{2026}", CHECK_UPDATES);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        self.add(&menu, "Quit Uzumaki", QUIT);
        self.item.setMenu(Some(&menu));
    }

    fn add(&self, menu: &NSMenu, title: &str, tag: isize) {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                self.mtm.alloc::<NSMenuItem>(),
                &NSString::from_str(title),
                Some(sel!(trayAction:)),
                &NSString::new(),
            )
        };
        item.setTag(tag);
        let target: &AnyObject = &self.target;
        unsafe { item.setTarget(Some(target)) };
        menu.addItem(&item);
    }
}

impl Drop for TrayCtl {
    /// Removes the icon from the menu bar.
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

fn template_image() -> Option<Retained<NSImage>> {
    let (w, h, rgba) = template_pixels()?;
    let img = image::RgbaImage::from_raw(w, h, rgba)?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png))?;
    // The pixels are for 2x, so the point size is half and it stays sharp on Retina.
    image.setSize(NSSize::new(ICON_PT * f64::from(w) / f64::from(h), ICON_PT));
    image.setTemplate(true);
    Some(image)
}

/// The 64 px tray PNG of the Dark app icon (the same file as the Linux tray).
const TRAY_PNG: &[u8] = include_bytes!("../../assets/icons/variants/dark/tray.png");

/// The swirl of the tray PNG as black with alpha, cropped to its bounds and `ICON_PX` tall.
fn template_pixels() -> Option<(u32, u32, Vec<u8>)> {
    let src = image::load_from_memory_with_format(TRAY_PNG, image::ImageFormat::Png).ok()?.to_rgba8();
    Some(template_from(&src))
}

/// Red of the dark tile (#1d1d22) and of the coral swirl (#ff7a8a). Red runs between them
/// linearly across an antialiased edge, so it measures how much swirl a pixel has; the tile
/// and its rounded corners end up with no alpha.
const TILE_RED: f32 = 29.0;
const CORAL_RED: f32 = 255.0;

fn template_from(src: &image::RgbaImage) -> (u32, u32, Vec<u8>) {
    let mut mask = image::RgbaImage::new(src.width(), src.height());
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in src.enumerate_pixels() {
        let swirl = ((f32::from(p[0]) - TILE_RED) / (CORAL_RED - TILE_RED)).clamp(0.0, 1.0);
        let a = (swirl * f32::from(p[3])).round() as u8;
        mask.put_pixel(x, y, image::Rgba([0, 0, 0, a]));
        if a > 40 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if x0 > x1 {
        return (1, 1, vec![0; 4]); // no swirl found
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let cropped = image::imageops::crop_imm(&mask, x0, y0, w, h).to_image();
    // A square canvas so the swirl isn't stretched, then the menu bar size.
    let side = w.max(h);
    let mut square = image::RgbaImage::new(side, side);
    image::imageops::replace(&mut square, &cropped, i64::from((side - w) / 2), i64::from((side - h) / 2));
    let out = image::imageops::resize(&square, ICON_PX, ICON_PX, image::imageops::FilterType::Lanczos3);
    (ICON_PX, ICON_PX, out.into_raw())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_template_is_a_black_swirl_without_a_tile() {
        let (w, h, rgba) = super::template_pixels().expect("the tray PNG decodes");
        assert_eq!((w, h), (super::ICON_PX, super::ICON_PX));
        assert!(rgba.chunks(4).all(|p| p[..3] == [0, 0, 0]), "black only; macOS tints it");
        let alpha = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize];
        // The corners were the tile's: no opaque background is left.
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert_eq!(alpha(x, y), 0, "transparent corner");
        }
        let solid = rgba.chunks(4).filter(|p| p[3] > 200).count();
        let clear = rgba.chunks(4).filter(|p| p[3] < 20).count();
        assert!(solid > 100, "a swirl, got {solid} solid pixels");
        assert!(clear > 200, "and not a filled square, got {clear} clear pixels");
    }

    #[test]
    fn the_template_comes_from_the_dark_tray_png() {
        assert_eq!(super::TRAY_PNG, include_bytes!("../../assets/icons/uzumaki-tray.png"));
    }
}
