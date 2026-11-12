//! The AppKit side of the menu bar (`ui/menu_bar.rs` builds it through Rune): the About panel
//! and a tidy of the app menu winit made. Main thread only, never headless.
//!
//! Rune puts the app's own app-menu items (About, Settings…) above winit's (About, Services,
//! Hide, Hide Others, Show All, Quit), so winit's About goes, leaving one, in the Mac order.
//! winit names Hide and Quit after the process ("uzumaki", the bundle's executable): they get
//! the app's name. The Window menu becomes `NSApp.windowsMenu` (AppKit lists the windows and
//! adds Bring All to Front) and Help `NSApp.helpMenu` (the search field).

use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{NSDictionary, NSString};

/// The standard About panel (name and icon from the bundle). The version and build are the
/// binary's own, so a `cargo run` outside the bundle shows them too: AppKit writes
/// "Version 0.1.0 (1)". Plus the licence. (Spelled out instead of `crate::VERSION` and
/// `crate::BUILD`: the menu bar test builds this file on its own.)
pub fn about() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let keys = [NSString::from_str("ApplicationVersion"), NSString::from_str("Version"), NSString::from_str("Copyright")];
    let values = [
        NSString::from_str(env!("CARGO_PKG_VERSION")),
        NSString::from_str(&uzumaki_ctl::BUILD.to_string()),
        NSString::from_str("© 2026 Thomas Lonigro\nFree software under the GNU GPL, version 3 or later."),
    ];
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&NSString> = values.iter().map(|v| &**v).collect();
    let options = NSDictionary::from_slices(&keys, &values);
    // SAFETY: main thread; the keys are AppKit's NSAboutPanelOption* strings, the values NSStrings.
    unsafe {
        let _: () = objc2::msg_send![&*app, orderFrontStandardAboutPanelWithOptions: &*options];
    }
}

/// Tidies the menu bar; true once nothing is left to do. Cheap, and idempotent.
pub fn tidy() -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return true };
    let app = NSApplication::sharedApplication(mtm);
    let Some(root) = app.mainMenu() else { return false };
    let Some(menu) = root.itemAtIndex(0).and_then(|i| i.submenu()) else { return false };
    let ours = sel!(runeMenuAction:);
    let mut i = 0;
    while i < menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(i) else { break };
        let action = item.action();
        if action == Some(sel!(orderFrontStandardAboutPanel:)) && action != Some(ours) {
            // winit's About and the line under it.
            menu.removeItemAtIndex(i);
            if menu.itemAtIndex(i).is_some_and(|n| n.isSeparatorItem()) {
                menu.removeItemAtIndex(i);
            }
            continue;
        }
        let title = if action == Some(sel!(hide:)) {
            Some("Hide Uzumaki")
        } else if action == Some(sel!(terminate:)) {
            Some("Quit Uzumaki")
        } else {
            None
        };
        if let Some(t) = title.filter(|t| item.title().to_string() != *t) {
            item.setTitle(&NSString::from_str(t));
        }
        // A line between Services and Hide, as every Mac app has.
        if item.submenu().is_some() && item.title().to_string() == "Services" {
            if menu.itemAtIndex(i + 1).is_some_and(|n| !n.isSeparatorItem()) {
                menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), i + 1);
            }
        }
        i += 1;
    }
    let top = |title: &str| -> Option<objc2::rc::Retained<NSMenu>> {
        (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).find(|i| i.title().to_string() == title).and_then(|i| i.submenu())
    };
    let mut done = true;
    if app.windowsMenu().is_none() {
        match top("Window") {
            Some(m) => app.setWindowsMenu(Some(&m)),
            None => done = false,
        }
    }
    if app.helpMenu().is_none() {
        match top("Help") {
            Some(m) => app.setHelpMenu(Some(&m)),
            None => done = false, // Rune adds it with the first menu snapshot
        }
    }
    done
}
