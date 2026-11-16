//! macOS start at login: the app bundle as a login item through `SMAppService.mainApp`
//! (ServiceManagement, macOS 13+, the app's minimum). It is listed in System Settings → General
//! → Login Items → "Open at Login". Works for the ad-hoc signed bundle (checked 2026-11-16 on a
//! probe app: register → Enabled, unregister → NotRegistered). Only a built `Uzumaki.app` can
//! be one: `cargo run` has no bundle, and a copy on the DMG or in App Translocation (opened from
//! Downloads with the quarantine flag) would point at a path that goes away.
//!
//! A login item gets no arguments, so `--hidden` can't be passed. `watch_launch` tells the app
//! whether this launch is the login one: the `kAEOpenApplication` Apple event that starts a login
//! item carries `keyAEPropData = keyAELaunchedAsLogInItem`, readable while AppKit posts
//! `NSApplicationDidFinishLaunchingNotification`.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::{NSError, NSString};

use super::Login;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// `SMAppServiceStatus`.
const NOT_REGISTERED: isize = 0;
const ENABLED: isize = 1;
const REQUIRES_APPROVAL: isize = 2;

const fn fourcc(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

/// Why this copy can't be a login item, from its bundle path.
fn unavailable_for(bundle: &str) -> Option<&'static str> {
    if !bundle.trim_end_matches('/').ends_with(".app") {
        Some("Only in the app (Uzumaki.app). This copy runs from a build folder.")
    } else if bundle.starts_with("/Volumes/") || bundle.contains("/AppTranslocation/") {
        Some("Move Uzumaki to the Applications folder first.")
    } else {
        None
    }
}

fn bundle_path() -> String {
    objc2_foundation::NSBundle::mainBundle().bundlePath().to_string()
}

fn service() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"SMAppService")?;
    // SAFETY: `+[SMAppService mainAppService]` returns the shared instance (or nil).
    unsafe { msg_send![cls, mainAppService] }
}

fn status(svc: &AnyObject) -> isize {
    // SAFETY: `status` is an NSInteger property.
    unsafe { msg_send![svc, status] }
}

/// The login item's state.
pub fn read() -> Login {
    if let Some(why) = unavailable_for(&bundle_path()) {
        return Login::unavailable(why);
    }
    let Some(svc) = service() else { return Login::unavailable("Needs macOS 13 or later.") };
    from_status(status(&svc))
}

fn from_status(st: isize) -> Login {
    Login { on: matches!(st, ENABLED | REQUIRES_APPROVAL), needs_approval: st == REQUIRES_APPROVAL, ..Login::default() }
}

/// Registers or unregisters the login item. `_hidden` is kept by the app's settings: a login
/// item has no arguments.
pub fn set(on: bool, _hidden: bool) -> Result<Login, String> {
    if let Some(why) = unavailable_for(&bundle_path()) {
        return Err(why.to_string());
    }
    let svc = service().ok_or("needs macOS 13 or later")?;
    let st = status(&svc);
    let result: Result<(), Retained<NSError>> = if on {
        if matches!(st, ENABLED | REQUIRES_APPROVAL) {
            Ok(())
        } else {
            // SAFETY: `-registerAndReturnError:` returns BOOL with an NSError out-parameter.
            unsafe { msg_send![&*svc, registerAndReturnError: _] }
        }
    } else if st == NOT_REGISTERED {
        Ok(())
    } else {
        // SAFETY: as above.
        unsafe { msg_send![&*svc, unregisterAndReturnError: _] }
    };
    match result {
        Ok(()) => Ok(from_status(status(&svc))),
        Err(e) => Err(e.localizedDescription().to_string()),
    }
}

/// Nothing to repair: the login item follows the bundle (Launch Services finds it if it moves).
pub fn repair() {}

/// Opens System Settings → General → Login Items (to approve a waiting item).
pub fn open_login_items() {
    if let Some(cls) = AnyClass::get(c"SMAppService") {
        // SAFETY: a class method with no arguments and no result.
        unsafe {
            let _: () = msg_send![cls, openSystemSettingsLoginItems];
        }
    }
}

/// Calls `f` once, as AppKit finishes launching, with true when macOS started the app as a
/// login item. Register before the app runs (from `main`).
pub fn watch_launch(f: impl FnOnce(bool) + 'static) {
    let f = Cell::new(Some(f));
    let block = block2::RcBlock::new(move |_note: *mut AnyObject| {
        if let Some(f) = f.take() {
            f(launched_at_login());
        }
    });
    let name = NSString::from_str("NSApplicationDidFinishLaunchingNotification");
    // SAFETY: NSNotificationCenter's block observer; the centre keeps the block, and the token is
    // kept for the process's life (the notification comes once).
    unsafe {
        let center: Retained<AnyObject> = msg_send![AnyClass::get(c"NSNotificationCenter").expect("Foundation"), defaultCenter];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let token: Option<Retained<AnyObject>> =
            msg_send![&*center, addObserverForName: &*name, object: nil, queue: nil, usingBlock: &*block];
        std::mem::forget(token);
    }
}

/// The Apple event being handled is the login item's "open application" one.
fn launched_at_login() -> bool {
    // SAFETY: plain getters on NSAppleEventManager / NSAppleEventDescriptor; every object is
    // checked for nil before use.
    unsafe {
        let Some(cls) = AnyClass::get(c"NSAppleEventManager") else { return false };
        let manager: *mut AnyObject = msg_send![cls, sharedAppleEventManager];
        if manager.is_null() {
            return false;
        }
        let event: *mut AnyObject = msg_send![manager, currentAppleEvent];
        if event.is_null() {
            return false;
        }
        let id: u32 = msg_send![event, eventID];
        if id != fourcc(b"oapp") {
            return false;
        }
        let prop: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: fourcc(b"prdt")];
        if prop.is_null() {
            return false;
        }
        let code: u32 = msg_send![prop, enumCodeValue];
        code == fourcc(b"lgit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_installed_app_bundle_can_start_at_login() {
        assert_eq!(unavailable_for("/Applications/Uzumaki.app"), None);
        assert_eq!(unavailable_for("/Users/u/work/uzumaki/target/macos/Uzumaki.app"), None);
        assert!(unavailable_for("/Users/u/work/uzumaki/target/debug").is_some(), "cargo run");
        assert!(unavailable_for("/Volumes/Uzumaki/Uzumaki.app").is_some(), "the DMG");
        assert!(unavailable_for("/private/var/folders/x/T/AppTranslocation/ABC/d/Uzumaki.app").is_some());
    }

    #[test]
    fn approval_pending_counts_as_on() {
        assert_eq!(from_status(ENABLED), Login { on: true, ..Login::default() });
        assert_eq!(from_status(REQUIRES_APPROVAL), Login { on: true, needs_approval: true, ..Login::default() });
        assert_eq!(from_status(NOT_REGISTERED), Login::default());
        assert_eq!(from_status(3), Login::default(), "not found: never registered");
    }

    #[test]
    fn four_char_codes() {
        assert_eq!(fourcc(b"oapp"), 0x6f61_7070);
        assert_eq!(fourcc(b"lgit"), 0x6c67_6974);
    }
}
