//! The Dock icon of the running app follows Settings → Appearance → App icon (macOS). AppKit
//! loads the bundle's icon once at launch, and `bundle_icon.rs` may swap that file later, so the
//! chosen image is always set explicitly (`setApplicationIconImage`), Dark included. The menu bar
//! icon is `tray_mac.rs`; the installed bundle's icon is `bundle_icon.rs`. Nothing headless.

use crate::settings::IconVariant;

#[cfg(target_os = "macos")]
mod imp {
    use super::IconVariant;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::time::Duration;

    /// 512 px, the bundle icon's proportions; redrawn from the SVGs by `packaging/macos/build.sh`.
    pub(super) fn png(icon: IconVariant) -> &'static [u8] {
        match icon {
            IconVariant::Dark => include_bytes!("../../assets/icons/macos/dock-dark.png"),
            IconVariant::Light => include_bytes!("../../assets/icons/macos/dock-light.png"),
            IconVariant::Coral => include_bytes!("../../assets/icons/macos/dock-coral.png"),
        }
    }

    /// The icon the view last asked for: 0 none yet, else 1 + its index in `IconVariant::ALL`.
    static WANT: AtomicU8 = AtomicU8::new(0);

    thread_local! {
        /// The icon last applied, so a frame does nothing unless it changed.
        static APPLIED: Cell<Option<IconVariant>> = const { Cell::new(None) };
    }

    fn code(icon: IconVariant) -> u8 {
        1 + IconVariant::ALL.iter().position(|&i| i == icon).unwrap_or(0) as u8
    }

    pub fn sync(icon: IconVariant) {
        if rune::is_headless() {
            return;
        }
        WANT.store(code(icon), Ordering::Relaxed);
        if APPLIED.get() != Some(icon) {
            apply(icon);
        }
    }

    /// AppKit puts the bundle icon back while it finishes launching and when it makes a window,
    /// after the first frame already set ours. Set it again now and a little later.
    pub fn reapply() {
        if rune::is_headless() {
            return;
        }
        std::thread::spawn(|| {
            for ms in [0, 300, 1500] {
                std::thread::sleep(Duration::from_millis(ms));
                dispatch2::DispatchQueue::main().exec_async(|| {
                    let want = WANT.load(Ordering::Relaxed);
                    if let Some(&icon) = want.checked_sub(1).and_then(|i| IconVariant::ALL.get(usize::from(i))) {
                        apply(icon);
                    }
                });
            }
        });
    }

    fn apply(icon: IconVariant) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let data = NSData::with_bytes(png(icon));
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
            return; // the PNG didn't decode: leave the icon, try again next frame
        };
        // SAFETY: on the main thread, a valid NSImage.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
        APPLIED.set(Some(icon));
    }
}

/// Called from `ui::view` with the chosen app icon; cheap when nothing changed.
#[cfg(target_os = "macos")]
pub fn sync(icon: IconVariant) {
    imp::sync(icon);
}
#[cfg(not(target_os = "macos"))]
pub fn sync(_icon: IconVariant) {}

/// macOS: set the icon again after AppKit may have reset it (launch, a new window, Dock reopen).
#[cfg(target_os = "macos")]
pub fn reapply() {
    imp::reapply();
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::IconVariant;

    #[test]
    fn each_app_icon_has_its_own_dock_image() {
        let tile = |i: IconVariant| {
            let img = image::load_from_memory(super::imp::png(i)).expect("decodes").to_rgba8();
            assert_eq!(img.dimensions(), (512, 512));
            *img.get_pixel(256, 60) // the tile above the swirl
        };
        assert!(tile(IconVariant::Dark)[0] < 0x30, "dark tile");
        assert!(tile(IconVariant::Light)[0] > 0xf0 && tile(IconVariant::Light)[2] > 0xf0, "white tile");
        let coral = tile(IconVariant::Coral);
        assert!(coral[0] > 0xf0 && coral[1] > 0xc0 && coral[1] < 0xe0, "pale coral tile, got {coral:?}");
    }
}
