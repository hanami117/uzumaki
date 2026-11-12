//! `menu_bar_mac::tidy` on winit's app menu as Rune leaves it with the app's own items on top:
//! one About, the Mac order, "Uzumaki" titles, the Window and Help menus registered. Main
//! thread, accessory, no window, no event loop; nothing is clicked or dispatched.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/menu_bar_mac.rs"]
mod menu_bar_mac;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem};
    use objc2_foundation::NSString;

    let mtm = MainThreadMarker::new().expect("harness = false runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let item = |title: &str, action: Option<objc2::runtime::Sel>| unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), action, &NSString::new())
    };
    let top = |root: &NSMenu, title: &str| {
        let t = item(title, None);
        let m = NSMenu::new(mtm);
        m.setTitle(&NSString::from_str(title));
        t.setSubmenu(Some(&m));
        root.addItem(&t);
        m
    };
    // winit 0.30's app menu (process name "uzumaki"), then Rune's Edit and Window.
    let root = NSMenu::new(mtm);
    let menu = top(&root, "");
    let services = item("Services", None);
    services.setSubmenu(Some(&NSMenu::new(mtm)));
    for i in [
        item("About uzumaki", Some(sel!(orderFrontStandardAboutPanel:))),
        NSMenuItem::separatorItem(mtm),
        services,
        item("Hide uzumaki", Some(sel!(hide:))),
        item("Hide Others", Some(sel!(hideOtherApplications:))),
        item("Show All", Some(sel!(unhideAllApplications:))),
        NSMenuItem::separatorItem(mtm),
        item("Quit uzumaki", Some(sel!(terminate:))),
    ] {
        menu.addItem(&i);
    }
    top(&root, "Edit");
    let window = top(&root, "Window");
    app.setMainMenu(Some(&root));
    // Rune's custom app-menu items go first (`Location::App` inserts from index 0).
    for (n, i) in [
        item("About Uzumaki", Some(sel!(runeMenuAction:))),
        NSMenuItem::separatorItem(mtm),
        item("Settings\u{2026}", Some(sel!(runeMenuAction:))),
        NSMenuItem::separatorItem(mtm),
    ]
    .into_iter()
    .enumerate()
    {
        menu.insertItem_atIndex(&i, n as isize);
    }
    let titles = |m: &NSMenu| -> Vec<String> {
        (0..m.numberOfItems())
            .filter_map(|i| m.itemAtIndex(i))
            .map(|i| if i.isSeparatorItem() { "-".into() } else { i.title().to_string() })
            .collect()
    };
    let want = [
        "About Uzumaki", "-", "Settings\u{2026}", "-", "Services", "-", "Hide Uzumaki", "Hide Others", "Show All", "-",
        "Quit Uzumaki",
    ];
    assert!(!menu_bar_mac::tidy(), "no Help menu yet: not done");
    assert_eq!(titles(&menu), want);
    assert!(app.windowsMenu().is_some_and(|m| std::ptr::eq(&*m, &*window)));
    // Rune's first snapshot adds Help; a second tidy changes nothing else.
    let help = top(&root, "Help");
    assert!(menu_bar_mac::tidy(), "done once Help exists");
    assert_eq!(titles(&menu), want);
    assert!(app.helpMenu().is_some_and(|m| std::ptr::eq(&*m, &*help)));
    let about = menu.itemAtIndex(0).unwrap();
    assert_eq!(about.action(), Some(sel!(runeMenuAction:)), "the app's own About stays");
    // A later Rune snapshot re-inserts the app's items at the top: still one About.
    menu.removeItemAtIndex(0);
    menu.insertItem_atIndex(&item("About Uzumaki", Some(sel!(runeMenuAction:))), 0);
    menu_bar_mac::tidy();
    assert_eq!(titles(&menu), want);
    app.setMainMenu(None);
    println!("PASS: one About, Mac order, Uzumaki titles, Window and Help menus registered, idempotent");
}
