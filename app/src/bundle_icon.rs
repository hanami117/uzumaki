//! macOS: makes the installed `Uzumaki.app` show Settings → Appearance → App icon in the Finder,
//! Launchpad and the Dock while the app is closed. (While it runs, `dock_icon.rs` sets the Dock
//! icon itself.)
//!
//! `packaging/macos/build.sh` puts the three icons in `Contents/Resources/icons/<variant>.icns`
//! and the dark one as the bundle icon, `Contents/Resources/Uzumaki.icns` (`CFBundleIconFile`).
//! Applying a choice copies the variant over `Uzumaki.icns` (temp file + rename), re-signs the
//! bundle ad hoc and asks LaunchServices to read it again. Dark puts the bundled dark icon back.
//!
//! Why not `NSWorkspace setIcon:forFile:`: it writes an `Icon\r` file and Finder info into the
//! bundle root, and `codesign` then refuses to sign the bundle at all ("resource fork, Finder
//! information, or similar detritus not allowed"), so the signature would stay broken. Swapping
//! the sealed `.icns` keeps the bundle valid.
//!
//! The re-sign is `codesign --force --sign -` on the bundle without `--deep`, the same as the
//! last step of `build.sh`: the bundled Electron keeps its own signature, which carries the
//! Widevine VMP signature (`--deep` would drop it and break playback). codesign writes a new file
//! for the executable, so the running process is not affected.
//!
//! The Dock keeps its picture of a pinned app's icon until it restarts: once the app quits, a
//! pinned icon goes back to the old one although the Finder shows the new one. So when this
//! session swapped the bundle icon, quitting runs `killall Dock` once (it comes back in about a
//! second). It runs from an `atexit` handler, which every way of quitting reaches (Ctrl/Cmd+Q,
//! the tray, the Dock's Quit, the OS ending the app), and only for a real `.app`.
//!
//! Only a real `.app` bundle with those files is touched: `cargo run` (no bundle) and headless
//! runs do nothing. Everything runs on a background thread; failures are logged, never fatal.
//! At start-up the same call runs, so a fresh install (dark again) gets the choice back.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, Once};

use crate::settings::IconVariant;

const LSREGISTER: &str =
    "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// The files of one bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The `.app` folder.
    pub bundle: PathBuf,
    /// The icon the bundle shows (`CFBundleIconFile`).
    pub icns: PathBuf,
    /// Holds `<variant>.icns`.
    pub variants: PathBuf,
}

impl Plan {
    fn variant(&self, icon: IconVariant) -> PathBuf {
        self.variants.join(format!("{}.icns", icon.dir()))
    }
}

/// The plan for the bundle at `bundle`, or None when it isn't a `.app` built by `build.sh` (a
/// `cargo run` binary's "bundle" is its own folder, `target/debug`).
pub fn plan_for(bundle: &Path) -> Option<Plan> {
    if bundle.extension().is_none_or(|e| e != "app") {
        return None;
    }
    let res = bundle.join("Contents/Resources");
    let plan = Plan { bundle: bundle.to_path_buf(), icns: res.join("Uzumaki.icns"), variants: res.join("icons") };
    (plan.icns.is_file() && IconVariant::ALL.iter().all(|&i| plan.variant(i).is_file())).then_some(plan)
}

/// Whether the bundle icon isn't `icon`'s (a missing or unreadable file counts as different).
pub fn differs(plan: &Plan, icon: IconVariant) -> bool {
    match (std::fs::read(plan.variant(icon)), std::fs::read(&plan.icns)) {
        (Ok(want), Ok(have)) => want != have,
        _ => true,
    }
}

/// Copies `icon`'s file over the bundle icon if it differs, then calls `after` (re-sign and
/// refresh). Returns whether anything changed.
pub fn apply(plan: &Plan, icon: IconVariant, after: impl FnOnce(&Plan) -> io::Result<()>) -> io::Result<bool> {
    if !differs(plan, icon) {
        return Ok(false);
    }
    let want = std::fs::read(plan.variant(icon))?;
    let tmp = plan.icns.with_extension("icns.new");
    std::fs::write(&tmp, want)?;
    if let Err(e) = std::fs::rename(&tmp, &plan.icns) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    after(plan)?;
    Ok(true)
}

/// Re-signs the bundle ad hoc (no `--deep`, see the module docs) and makes LaunchServices, and so
/// the Finder and the Dock, pick up the new icon: the bundle's mtime, then `lsregister -f`.
fn resign_and_refresh(plan: &Plan) -> io::Result<()> {
    let out = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&plan.bundle)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("codesign failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    if let Ok(dir) = std::fs::File::open(&plan.bundle) {
        let _ = dir.set_modified(std::time::SystemTime::now());
    }
    let _ = Command::new(LSREGISTER)
        .arg("-f")
        .arg(&plan.bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// Set when this session replaced the bundle's icon file (even if the re-sign then failed).
static SWAPPED: AtomicBool = AtomicBool::new(false);

/// Whether quitting should restart the Dock: the bundle icon changed during this session, in a
/// real windowed run from a built `.app` (never headless, never `cargo run`).
pub fn dock_restart_due(swapped: bool, headless: bool, bundle: Option<&Path>) -> bool {
    swapped && !headless && bundle.and_then(plan_for).is_some()
}

/// Notes that the icon file changed and makes sure the quit handler is installed (once).
fn note_swapped() {
    static INSTALL: Once = Once::new();
    SWAPPED.store(true, Ordering::SeqCst);
    INSTALL.call_once(|| {
        // SAFETY: `at_exit` is a plain `extern "C" fn()` that lives for the whole process.
        if unsafe { libc::atexit(at_exit) } != 0 {
            eprintln!("uzumaki: couldn't arrange the Dock restart at quit");
        }
    });
}

extern "C" fn at_exit() {
    restart_dock_if_due();
}

/// Runs `killall Dock` once if the decision says so; failures are logged.
fn restart_dock_if_due() {
    let swapped = SWAPPED.swap(false, Ordering::SeqCst);
    if !dock_restart_due(swapped, rune::is_headless(), main_bundle().as_deref()) {
        return;
    }
    match Command::new("/usr/bin/killall").arg("Dock").stdin(Stdio::null()).output() {
        Ok(out) if out.status.success() => eprintln!("uzumaki: restarted the Dock to show the new icon"),
        Ok(out) => eprintln!("uzumaki: killall Dock failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
        Err(e) => eprintln!("uzumaki: couldn't run killall Dock: {e}"),
    }
}

/// The running app's bundle folder (`NSBundle.mainBundle.bundlePath`).
fn main_bundle() -> Option<PathBuf> {
    let path = objc2_foundation::NSBundle::mainBundle().bundlePath();
    Some(PathBuf::from(path.to_string())).filter(|p| p.is_absolute())
}

static LATEST: AtomicU64 = AtomicU64::new(0);
static BUSY: Mutex<()> = Mutex::new(());

/// How many times `apply_async` was asked to apply an icon (tests: offline picks ask for none).
#[cfg(test)]
pub fn requests() -> u64 {
    LATEST.load(Ordering::SeqCst)
}

/// Makes the installed bundle show `icon`, on a background thread (the choice was just made, or
/// the app just started). A later call supersedes an earlier one that hasn't started. Nothing in
/// headless runs.
pub fn apply_async(icon: IconVariant) {
    if rune::is_headless() {
        return;
    }
    let ticket = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let _busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        if LATEST.load(Ordering::SeqCst) != ticket {
            return;
        }
        let Some(plan) = main_bundle().as_deref().and_then(plan_for) else { return };
        // `after` runs only once the file was replaced: from then on the Dock's copy is stale.
        let after = |p: &Plan| {
            note_swapped();
            resign_and_refresh(p)
        };
        match apply(&plan, icon, after) {
            Ok(true) => eprintln!("uzumaki: the app bundle's icon is now {}", icon.dir()),
            Ok(false) => {}
            Err(e) => eprintln!("uzumaki: couldn't change the app bundle's icon: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch bundle like `build.sh` makes; variant `v`'s file holds `[v]`, the bundle icon is dark.
    fn bundle(tag: &str) -> (PathBuf, Plan) {
        let dir = std::env::temp_dir().join(format!("uzumaki-bundle-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = dir.join("Uzumaki.app");
        let res = app.join("Contents/Resources");
        std::fs::create_dir_all(res.join("icons")).unwrap();
        for (i, icon) in IconVariant::ALL.iter().enumerate() {
            std::fs::write(res.join(format!("icons/{}.icns", icon.dir())), [i as u8]).unwrap();
        }
        std::fs::write(res.join("Uzumaki.icns"), [0u8]).unwrap();
        let plan = plan_for(&app).expect("a built bundle");
        (dir, plan)
    }

    #[test]
    fn only_a_built_app_bundle_is_planned() {
        let (dir, plan) = bundle("plan");
        assert_eq!(plan.icns, dir.join("Uzumaki.app/Contents/Resources/Uzumaki.icns"));
        assert_eq!(plan_for(&dir), None, "cargo run: the binary's folder, not a .app");
        assert_eq!(plan_for(Path::new("/nonexistent/Uzumaki.app")), None);
        std::fs::remove_file(plan.variant(IconVariant::Coral)).unwrap();
        assert_eq!(plan_for(&plan.bundle), None, "an older build without the variants");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apply_swaps_the_icon_only_when_it_differs_and_dark_puts_the_bundled_one_back() {
        let (dir, plan) = bundle("apply");
        let calls = std::cell::Cell::new(0);
        let after = |_: &Plan| {
            calls.set(calls.get() + 1);
            Ok(())
        };
        assert!(!differs(&plan, IconVariant::Dark), "a fresh build shows the dark icon");
        assert!(!apply(&plan, IconVariant::Dark, after).unwrap(), "nothing to do: no re-sign");
        assert_eq!(calls.get(), 0);

        assert!(apply(&plan, IconVariant::Coral, after).unwrap());
        assert_eq!(std::fs::read(&plan.icns).unwrap(), [2]);
        assert_eq!(calls.get(), 1);
        assert!(!apply(&plan, IconVariant::Coral, after).unwrap(), "already coral");
        assert_eq!(calls.get(), 1);

        assert!(apply(&plan, IconVariant::Dark, after).unwrap());
        assert_eq!(std::fs::read(&plan.icns).unwrap(), [0]);
        assert!(!plan.icns.with_extension("icns.new").exists(), "no temp file left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_dock_restarts_only_for_a_changed_icon_in_a_real_windowed_app() {
        let (dir, plan) = bundle("dock");
        let app = Some(plan.bundle.as_path());
        assert!(dock_restart_due(true, false, app), "the icon changed this session");
        assert!(!dock_restart_due(false, false, app), "nothing changed: leave the Dock alone");
        assert!(!dock_restart_due(true, true, app), "never headless");
        assert!(!dock_restart_due(true, false, Some(dir.as_path())), "cargo run: not a .app");
        assert!(!dock_restart_due(true, false, None), "no bundle at all");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_re_sign_is_an_error_not_a_panic() {
        let (dir, plan) = bundle("fail");
        let r = apply(&plan, IconVariant::Light, |_| Err(io::Error::other("codesign failed")));
        assert!(r.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
