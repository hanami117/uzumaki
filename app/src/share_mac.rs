//! macOS: the system's sharing services for a link, for the Share submenu (as in Apple Music:
//! Copy Link, then AirDrop, Messages, Mail, Notes, Reminders, Freeform and any share extension).
//!
//! `NSSharingService.sharingServicesForItems:` lists the services for an NSURL, with their
//! titles and icons. Reading them takes a few hundred ms (the icons most of it), so `warm` reads
//! them once, a little after start, on the main thread; the menus only read the cached list.
//! The icons are drawn once into RGBA (`rune::Pixels`) for the in-app menu.
//!
//! A chosen service gets the resolved link through `performWithItems:`. The delegate gives it the
//! app's window (sheets such as Messages' and AirDrop's attach to it) and the point the menu was
//! opened at (where they zoom from). AppKit objects stay on the main thread: everything here runs
//! there (Rune's UI thread is the main thread on macOS), and does nothing anywhere else.
//!
//! `sharingServicesForItems:` is deprecated since macOS 13 in favour of the picker's ready-made
//! NSMenu (`standardShareMenuItem`), which only an AppKit menu can show; our menus are drawn by
//! Rune, so this keeps the list call (it still works, and the titles and icons are the same).

// Tests never touch the system's services (`share::perform` only records there): the native
// path is compiled but unused in test builds.
#![cfg_attr(test, allow(dead_code))]

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSCompositingOperation, NSSharingContentScope, NSSharingService, NSSharingServiceDelegate, NSSharingServicePicker, NSWindow,
};
use objc2_foundation::{NSRectEdge, NSArray, NSError, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};

use crate::share::Service;

/// Icon size in points in the menu; drawn at twice that for Retina.
const ICON_PX: isize = 32;

struct Cache {
    list: Rc<[Service]>,
    services: Vec<Retained<NSSharingService>>,
}

thread_local! {
    static CACHE: RefCell<Option<Cache>> = const { RefCell::new(None) };
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
}

/// Reads the services a little after start, on the main thread.
pub fn warm() {
    let Ok(when) = dispatch2::DispatchTime::try_from(std::time::Duration::from_secs(3)) else { return };
    let _ = dispatch2::DispatchQueue::main().after(when, load);
}

/// The services read so far.
pub fn loaded() -> Option<Rc<[Service]>> {
    CACHE.with(|c| c.borrow().as_ref().map(|c| c.list.clone()))
}

/// Reads the system's services for a link, once (main thread only; nothing elsewhere).
pub fn load() {
    if CACHE.with(|c| c.borrow().is_some()) {
        return;
    }
    let Some(_mtm) = MainThreadMarker::new() else { return };
    let started = std::time::Instant::now();
    let Some(items) = items_for("https://music.apple.com/") else { return };
    // SAFETY: an array of NSURL, as the method takes.
    #[allow(deprecated)]
    let found = unsafe { NSSharingService::sharingServicesForItems(&items) };
    let mut list = Vec::new();
    let mut services = Vec::new();
    for service in found.iter() {
        list.push(Service { title: service.menuItemTitle().to_string(), icon: icon_of(&service) });
        services.push(service);
    }
    crate::framelog::event(|| format!("share: {} services in {:?}", list.len(), started.elapsed()));
    CACHE.with(|c| *c.borrow_mut() = Some(Cache { list: list.into(), services }));
}

/// `[NSURL(url)]`, or None when `url` isn't a URL.
fn items_for(url: &str) -> Option<Retained<NSArray>> {
    let url = NSURL::URLWithString(&NSString::from_str(url))?;
    let obj: Retained<AnyObject> = Retained::into_super(Retained::into_super(url));
    Some(NSArray::from_retained_slice(&[obj]))
}

/// The service's icon drawn into straight-alpha RGBA, `ICON_PX` square.
fn icon_of(service: &NSSharingService) -> Option<rune::Pixels> {
    let image = service.image();
    let side = ICON_PX as f64;
    let mut rgba = crate::raster_mac::draw_rgba(ICON_PX, || {
        image.drawInRect_fromRect_operation_fraction(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(side, side)),
            NSRect::ZERO,
            NSCompositingOperation::Copy,
            1.0,
        );
    })?;
    unpremultiply(&mut rgba);
    let px = rune::Pixels::new();
    px.set(ICON_PX as u32, ICON_PX as u32, rgba);
    Some(px)
}

/// AppKit's bitmaps are premultiplied; Rune takes straight alpha.
fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

struct Ivars {
    /// Where the menu was opened, in screen coordinates.
    anchor: Cell<NSRect>,
}

define_class!(
    // Gives a performing service the app's window and the point it zooms from.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UzumakiShareDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSSharingServiceDelegate for Delegate {
        #[unsafe(method(sharingService:sourceFrameOnScreenForShareItem:))]
        fn source_frame(&self, _service: &NSSharingService, _item: &AnyObject) -> NSRect {
            self.ivars().anchor.get()
        }

        #[unsafe(method_id(sharingService:sourceWindowForShareItems:sharingContentScope:))]
        fn source_window(
            &self,
            _service: &NSSharingService,
            _items: &NSArray,
            _scope: NonNull<NSSharingContentScope>,
        ) -> Option<Retained<NSWindow>> {
            crate::content_view().and_then(|v| v.window())
        }

        #[unsafe(method(sharingService:didFailToShareItems:error:))]
        fn did_fail(&self, service: &NSSharingService, _items: &NSArray, error: &NSError) {
            eprintln!("share: {} failed: {}", service.title(), error.localizedDescription());
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { anchor: Cell::new(NSRect::ZERO) });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

/// Hands `url` to service `index` of the cached list; `at` is the menu's point in window points.
pub fn perform(index: usize, url: &str, at: [f32; 2]) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(service) = CACHE.with(|c| c.borrow().as_ref().and_then(|c| c.services.get(index).cloned())) else { return };
    let Some(items) = items_for(url) else {
        eprintln!("share: not a URL: {url}");
        return;
    };
    let delegate = DELEGATE.with(|d| d.borrow_mut().get_or_insert_with(|| Delegate::new(mtm)).clone());
    delegate.ivars().anchor.set(anchor_on_screen(at));
    service.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // SAFETY: an array of NSURL, which every listed service accepts (they were listed for one).
    unsafe {
        if service.canPerformWithItems(Some(&items)) {
            service.performWithItems(&items);
        } else {
            eprintln!("share: {} can't share {url}", service.title());
        }
    }
}

thread_local! {
    /// The picker on screen (AppKit doesn't keep it alive by itself).
    static PICKER: RefCell<Option<Retained<NSSharingServicePicker>>> = const { RefCell::new(None) };
}

/// The native sharing picker for `url`, anchored at the Share item: `at` is its top left in window
/// points, the rect as wide as a menu and one item high; the picker opens to its right.
pub fn picker(url: &str, at: [f32; 2]) {
    let Some(_mtm) = MainThreadMarker::new() else { return };
    let Some(view) = crate::content_view() else { return };
    let Some(items) = items_for(url) else {
        eprintln!("share: not a URL: {url}");
        return;
    };
    let (x, y) = (f64::from(at[0]), f64::from(at[1]));
    let (w, h) = (f64::from(crate::ui::MENU_W), f64::from(crate::ui::MENU_ITEM_H));
    let y = if view.isFlipped() { y } else { view.bounds().size.height - y - h };
    // SAFETY: an array of NSURL, which the picker takes as its items.
    let picker = unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
    picker.showRelativeToRect_ofView_preferredEdge(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)), &view, NSRectEdge::MaxX);
    PICKER.with(|p| *p.borrow_mut() = Some(picker));
}

/// The menu's point (window points from the top left) as a small rect on screen.
fn anchor_on_screen(at: [f32; 2]) -> NSRect {
    let Some(view) = crate::content_view() else { return NSRect::ZERO };
    let Some(window) = view.window() else { return NSRect::ZERO };
    let (x, y) = (f64::from(at[0]), f64::from(at[1]));
    let y = if view.isFlipped() { y } else { view.bounds().size.height - y };
    let in_window = view.convertRect_toView(NSRect::new(NSPoint::new(x, y), NSSize::new(1.0, 1.0)), None);
    window.convertRectToScreen(in_window)
}

#[cfg(test)]
mod tests {
    #[test]
    fn unpremultiply_restores_straight_alpha() {
        let mut px = vec![64, 32, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0];
        super::unpremultiply(&mut px);
        assert_eq!(px, [128, 64, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0]);
    }
}
