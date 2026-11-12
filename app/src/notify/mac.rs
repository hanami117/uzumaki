//! macOS backend: UNUserNotificationCenter through objc2 (Rune has no system notifications).
//!
//! Notifications need the app bundle (`io.github.hanami117.uzumaki`): a bare `cargo run` binary
//! has no bundle identifier, the framework would raise an Objective-C exception, so nothing is
//! sent then (one line in the log). Every song uses the same request identifier, so each one
//! replaces the previous notification. The permission prompt is shown when the setting is
//! switched on (`prepare`), and again, harmlessly, before a send (the system remembers the
//! answer). Called from worker threads only; the center is thread-safe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::msg_send;
use objc2_foundation::{NSArray, NSString};

use super::{Backend, Note};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

/// One identifier: a new request with it replaces the one on screen.
const REQUEST_ID: &str = "io.github.hanami117.uzumaki.now-playing";
/// `UNAuthorizationOptionAlert`.
const OPTION_ALERT: usize = 1 << 2;
/// How long to wait for an answer to the permission prompt.
const PROMPT_WAIT: Duration = Duration::from_secs(120);

pub struct Sys {
    warned: AtomicBool,
}

impl Sys {
    pub fn new() -> Self {
        Self { warned: AtomicBool::new(false) }
    }

    /// The center, or None (and one log line) when the process has no bundle identifier.
    fn center(&self) -> Option<Retained<AnyObject>> {
        let bundled = || -> Option<()> {
            let ns_bundle = AnyClass::get(c"NSBundle")?;
            // SAFETY: plain class and instance getters with no arguments.
            let bundle: Option<Retained<AnyObject>> = unsafe { msg_send![ns_bundle, mainBundle] };
            let id: Option<Retained<NSString>> = unsafe { msg_send![&*bundle?, bundleIdentifier] };
            id.map(|_| ())
        };
        if bundled().is_none() {
            if !self.warned.swap(true, Ordering::Relaxed) {
                eprintln!("uzumaki: notifications need the app bundle (not a bare binary); none shown");
            }
            return None;
        }
        let class = AnyClass::get(c"UNUserNotificationCenter")?;
        // SAFETY: a no-argument class getter; a bundle exists (checked above).
        unsafe { msg_send![class, currentNotificationCenter] }
    }

    /// Asks for permission; true when notifications are allowed. Blocks until the user answers.
    fn authorized(center: &AnyObject) -> bool {
        let (tx, rx) = mpsc::channel();
        let handler = RcBlock::new(move |granted: Bool, _error: *mut AnyObject| {
            let _ = tx.send(granted.as_bool());
        });
        // SAFETY: the selector takes (NSUInteger, void (^)(BOOL, NSError *)); the block is
        // copied by the framework.
        unsafe {
            let _: () = msg_send![center, requestAuthorizationWithOptions: OPTION_ALERT, completionHandler: &*handler];
        }
        rx.recv_timeout(PROMPT_WAIT).unwrap_or(false)
    }
}

impl Backend for Sys {
    fn prepare(&self) {
        if let Some(center) = self.center() {
            Self::authorized(&center);
        }
    }

    fn send(&self, note: &Note) {
        let Some(center) = self.center() else { return };
        if !Self::authorized(&center) {
            return;
        }
        let (Some(content_class), Some(request_class)) =
            (AnyClass::get(c"UNMutableNotificationContent"), AnyClass::get(c"UNNotificationRequest"))
        else {
            return;
        };
        // SAFETY: Objective-C calls on objects created here, with argument types matching the
        // UserNotifications selectors; every object stays alive across its call.
        unsafe {
            let content: Retained<AnyObject> = msg_send![content_class, new];
            let title = NSString::from_str(&note.title);
            let body = NSString::from_str(&note.body);
            let _: () = msg_send![&*content, setTitle: &*title];
            let _: () = msg_send![&*content, setBody: &*body];
            if let Some(att) = note.cover.as_deref().and_then(attachment) {
                let list = NSArray::from_slice(&[&*att]);
                let _: () = msg_send![&*content, setAttachments: &*list];
            }
            let id = NSString::from_str(REQUEST_ID);
            let request: Retained<AnyObject> =
                msg_send![request_class, requestWithIdentifier: &*id, content: &*content, trigger: Option::<&AnyObject>::None];
            let done = RcBlock::new(|error: *mut AnyObject| {
                if !error.is_null() {
                    eprintln!("uzumaki: could not show the song notification");
                }
            });
            let _: () = msg_send![&*center, addNotificationRequest: &*request, withCompletionHandler: &*done];
        }
    }
}

/// The cover as a `UNNotificationAttachment`. The art cache's files have no extension and the
/// system picks the image type from it, so the file is copied under a name with the right one.
fn attachment(cover: &std::path::Path) -> Option<Retained<AnyObject>> {
    let bytes = std::fs::read(cover).ok()?;
    let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else { "jpg" };
    let copy = std::env::temp_dir().join(format!("uzumaki-song-cover.{ext}"));
    std::fs::write(&copy, &bytes).ok()?;
    let url_class = AnyClass::get(c"NSURL")?;
    let att_class = AnyClass::get(c"UNNotificationAttachment")?;
    let path = NSString::from_str(&copy.display().to_string());
    let ident = NSString::from_str("cover");
    // SAFETY: a file URL from a string, then the attachment factory with no options and no
    // error out-parameter (null is allowed; a bad file just gives nil).
    unsafe {
        let url: Retained<AnyObject> = msg_send![url_class, fileURLWithPath: &*path];
        msg_send![
            att_class,
            attachmentWithIdentifier: &*ident,
            URL: &*url,
            options: Option::<&AnyObject>::None,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ]
    }
}
