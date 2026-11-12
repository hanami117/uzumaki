//! Linux: makes the installed icon files match Settings → Appearance → App icon.
//!
//! Docks and taskbars take the icon from the desktop entry's `Icon=io.github.hanami117.uzumaki`,
//! looked up in the icon theme. The variants are `assets/icons/variants/` in the source tree,
//! installed as `<prefix>/share/uzumaki/icons/<variant>/<size>.png`. Three kinds of install:
//!
//! - `install.sh` (`<prefix>/bin/uzumaki`, `~/.local` by default, writable by the user): the
//!   chosen variant overwrites the PNGs in `<prefix>/share/icons/hicolor/<size>x<size>/apps/`
//!   ([`Mode::InPlace`]).
//! - A package (`<root>/lib/uzumaki/uzumaki`, like the AUR's `/usr/lib/uzumaki`) or any system
//!   prefix (`/usr`, `/opt`, or one the user can't write to): the system files are never touched.
//!   A non-default choice writes user overrides, the variant PNGs into
//!   `$XDG_DATA_HOME/icons/hicolor/...` and a copy of the system desktop entry (same desktop id,
//!   so it wins) into `$XDG_DATA_HOME/applications/`, marked with `X-Uzumaki-Override=true`.
//!   Choosing Dark (the default, which is the system set) removes the copies we wrote: icons
//!   whose bytes equal one of our variants, and the desktop entry only if it has the marker
//!   ([`Mode::UserOverride`]).
//! - A build run from the source tree leaves the user's icons alone, unless
//!   `UZUMAKI_APPLY_ICONS=1` asks for it (then: the source tree's variants into
//!   `$XDG_DATA_HOME/icons/hicolor`, in place).
//!
//! All file work runs on a background thread. At start-up the same call runs, so a reinstall
//! (which puts the dark set back) is undone by the next launch.
//!
//! The file logic compiles on every platform for the tests; the D-Bus signal to KDE and the
//! lookup of the running binary are Linux only.

use std::io;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::APP_ID;
use crate::settings::IconVariant;

/// The pixel sizes `install.sh` installs and `variants/<name>/` holds.
pub const SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// The line that marks a desktop entry we wrote (the only kind we ever remove).
const MARKER: &str = "X-Uzumaki-Override=true";

/// How the choice reaches the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Overwrite the installed icons (a prefix the user owns).
    InPlace,
    /// Leave the system files alone: icons and a desktop entry copy in the user's data folder.
    UserOverride {
        /// The system desktop entry to copy (None: it isn't there, only icons are written).
        system_desktop: Option<PathBuf>,
    },
}

/// Where the variants come from and where the icons go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Holds `<variant>/<size>.png`.
    pub src: PathBuf,
    /// The `hicolor` theme folder to write into.
    pub hicolor: PathBuf,
    /// The menu entry, touched after a change so desktops re-read it (if it exists); with
    /// `UserOverride`, where the copy goes.
    pub desktop: PathBuf,
    pub mode: Mode,
}

/// `$XDG_DATA_HOME` (absolute), else `~/.local/share`.
#[cfg(target_os = "linux")]
fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn icon_path(hicolor: &Path, size: u32) -> PathBuf {
    hicolor.join(format!("{size}x{size}/apps/{APP_ID}.png"))
}

/// The install prefix of a binary, and whether it sits in a package's libdir
/// (`<root>/lib/uzumaki/uzumaki`) rather than `<prefix>/bin`.
fn install_prefix(exe: &Path) -> Option<(&Path, bool)> {
    let dir = exe.parent()?;
    if dir.file_name().is_some_and(|n| n == "bin") {
        return Some((dir.parent()?, false));
    }
    let lib = dir.parent()?;
    if dir.file_name().is_some_and(|n| n == "uzumaki") && lib.file_name().is_some_and(|n| n == "lib" || n == "lib64") {
        return Some((lib.parent()?, true));
    }
    None
}

/// Whether the user can add files in `dir` (tried by creating and removing one).
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".uzumaki-probe-{}", std::process::id()));
    let ok = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The plan for a binary at `exe`. `dev` is the `UZUMAKI_APPLY_ICONS` switch, `data_home` the
/// user data folder it (and a package install) writes to. None: do nothing (a source-tree
/// build, an install without variants, or one whose icons aren't where the installer puts them).
pub fn plan_for(exe: &Path, dev: bool, data_home: Option<&Path>) -> Option<Plan> {
    if let Some((prefix, package)) = install_prefix(exe) {
        let src = prefix.join("share/uzumaki/icons");
        let hicolor = prefix.join("share/icons/hicolor");
        if src.is_dir() && icon_path(&hicolor, 128).is_file() {
            let desktop = prefix.join(format!("share/applications/{APP_ID}.desktop"));
            let system = package
                || prefix.starts_with("/usr")
                || prefix.starts_with("/opt")
                || !dir_writable(icon_path(&hicolor, 128).parent()?);
            if system {
                // The user's data folder being the prefix's own share/ would make "override" the
                // same files: treat that as an install the user owns.
                match data_home.filter(|h| *h != prefix.join("share")) {
                    Some(home) => {
                        return Some(Plan {
                            src,
                            hicolor: home.join("icons/hicolor"),
                            desktop: home.join(format!("applications/{APP_ID}.desktop")),
                            mode: Mode::UserOverride { system_desktop: desktop.is_file().then_some(desktop) },
                        });
                    }
                    None if data_home.is_none() => return None, // nowhere we may write
                    None => {}
                }
            }
            return Some(Plan { src, hicolor, desktop, mode: Mode::InPlace });
        }
    }
    if dev {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/icons/variants");
        let home = data_home?;
        if src.is_dir() {
            return Some(Plan {
                src,
                hicolor: home.join("icons/hicolor"),
                desktop: home.join(format!("applications/{APP_ID}.desktop")),
                mode: Mode::InPlace,
            });
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn current_plan() -> Option<Plan> {
    // An AppImage's files vanish when it quits, and a user copy of its desktop entry (Exec=uzumaki)
    // would start nothing: leave the menu entry and icons to the AppImage integration tool.
    if crate::bridge::running_from_appimage() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let dev = std::env::var_os("UZUMAKI_APPLY_ICONS").is_some_and(|v| !v.is_empty() && v != "0");
    plan_for(&exe, dev, data_home().as_deref())
}

/// Whether the file at `path` is one of our variant PNGs for `size` (so we may remove it).
fn is_ours(plan: &Plan, size: u32, path: &Path) -> bool {
    let Ok(have) = std::fs::read(path) else { return false };
    IconVariant::ALL
        .into_iter()
        .any(|v| std::fs::read(plan.src.join(v.dir()).join(format!("{size}.png"))).is_ok_and(|want| want == have))
}

/// The sizes `apply` would change for `icon`: with a user override and the default, the sizes
/// where one of our copies sits; otherwise the sizes whose installed file is missing or differs
/// from `icon`'s (sizes the variant has no file for are left out: there is nothing to apply).
pub fn stale_sizes(plan: &Plan, icon: IconVariant) -> Vec<u32> {
    if matches!(plan.mode, Mode::UserOverride { .. }) && icon == IconVariant::Dark {
        return SIZES.into_iter().filter(|&s| is_ours(plan, s, &icon_path(&plan.hicolor, s))).collect();
    }
    SIZES
        .into_iter()
        .filter(|&size| {
            let Ok(want) = std::fs::read(plan.src.join(icon.dir()).join(format!("{size}.png"))) else { return false };
            std::fs::read(icon_path(&plan.hicolor, size)).map_or(true, |have| have != want)
        })
        .collect()
}

/// Whether the installed icons are not `icon`'s.
#[cfg(test)]
pub fn differs(plan: &Plan, icon: IconVariant) -> bool {
    !stale_sizes(plan, icon).is_empty()
}

/// `contents` of a desktop entry with the marker added under `[Desktop Entry]` (None: no such
/// header, so it isn't a desktop entry we can mark).
fn marked(contents: &str) -> Option<String> {
    let mut out = String::with_capacity(contents.len() + 128);
    let mut done = false;
    for line in contents.lines() {
        out.push_str(line);
        out.push('\n');
        if !done && line.trim() == "[Desktop Entry]" {
            out.push_str("# Written by Uzumaki for Settings > App icon; removed again when the icon is Dark.\n");
            out.push_str(MARKER);
            out.push('\n');
            done = true;
        }
    }
    done.then_some(out)
}

fn has_marker(contents: &str) -> bool {
    contents.lines().any(|l| l.trim() == MARKER)
}

fn write_atomic(dst: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = dst.as_os_str().to_owned();
    tmp.push(".new");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, dst)
}

/// Writes `icon`'s files over the installed ones; returns how many.
fn write_icons(plan: &Plan, icon: IconVariant) -> io::Result<usize> {
    let stale = stale_sizes(plan, icon);
    for &size in &stale {
        let want = std::fs::read(plan.src.join(icon.dir()).join(format!("{size}.png")))?;
        write_atomic(&icon_path(&plan.hicolor, size), &want)?;
    }
    Ok(stale.len())
}

/// Puts the marked copy of the system desktop entry in the user's folder (0 or 1 files written).
/// A desktop entry there that we didn't write is left alone.
fn write_desktop(plan: &Plan, system: Option<&Path>) -> io::Result<usize> {
    let Some(system) = system else { return Ok(0) };
    let Some(want) = std::fs::read_to_string(system).ok().and_then(|c| marked(&c)) else { return Ok(0) };
    if let Ok(have) = std::fs::read_to_string(&plan.desktop)
        && (!has_marker(&have) || have == want)
    {
        return Ok(0);
    }
    write_atomic(&plan.desktop, want.as_bytes())?;
    Ok(1)
}

/// Removes the user copies we wrote; returns how many files went.
fn remove_ours(plan: &Plan) -> io::Result<usize> {
    let mut n = 0;
    for size in stale_sizes(plan, IconVariant::Dark) {
        std::fs::remove_file(icon_path(&plan.hicolor, size))?;
        n += 1;
    }
    if std::fs::read_to_string(&plan.desktop).is_ok_and(|c| has_marker(&c)) {
        std::fs::remove_file(&plan.desktop)?;
        n += 1;
    }
    Ok(n)
}

/// Makes the icons `icon`'s (see the module docs for where) and, if any file changed, nudges the
/// desktop. Files are written through a temp file and a rename, so a reader never sees half a
/// PNG. Returns how many files it wrote or removed.
pub fn apply(plan: &Plan, icon: IconVariant) -> io::Result<usize> {
    let n = match &plan.mode {
        Mode::InPlace => write_icons(plan, icon)?,
        Mode::UserOverride { .. } if icon == IconVariant::Dark => remove_ours(plan)?,
        Mode::UserOverride { system_desktop } => {
            write_icons(plan, icon)? + write_desktop(plan, system_desktop.as_deref())?
        }
    };
    if n > 0 {
        refresh(plan);
    }
    Ok(n)
}

/// What a desktop looks at to notice new icons, all cheap and best effort:
/// - the theme folder's mtime (Qt and KDE's icon loader compare it, GTK compares it with the cache);
/// - `gtk-update-icon-cache -t`, but only for a cache that already exists (a new one would go
///   stale when other apps add icons; the same rule as `install.sh`);
/// - the menu entry's mtime, which makes KDE's service cache (KSycoca) rebuild.
fn refresh(plan: &Plan) {
    let now = SystemTime::now();
    if let Ok(dir) = std::fs::File::open(&plan.hicolor) {
        let _ = dir.set_modified(now);
    }
    if plan.hicolor.join("icon-theme.cache").is_file() {
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .arg("-q")
            .arg("-t")
            .arg(&plan.hicolor)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&plan.desktop) {
        let _ = f.set_modified(now);
    }
}

/// The newest request; an older one still waiting for the lock gives up.
#[cfg(target_os = "linux")]
static LATEST: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "linux")]
static BUSY: Mutex<()> = Mutex::new(());

/// How many times `apply_async` was asked to apply an icon (tests: offline picks ask for none).
#[cfg(all(test, target_os = "linux"))]
pub fn requests() -> u64 {
    LATEST.load(Ordering::SeqCst)
}

/// Makes the installed icons `icon`'s, on a background thread (the choice was just made, or the
/// app just started). A later call supersedes an earlier one that hasn't started.
#[cfg(target_os = "linux")]
pub fn apply_async(icon: IconVariant) {
    let ticket = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let _busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        if LATEST.load(Ordering::SeqCst) != ticket {
            return;
        }
        let Some(plan) = current_plan() else { return };
        match apply(&plan, icon) {
            Ok(0) => {}
            Ok(_) => tell_kde_icons_changed(),
            Err(e) => eprintln!("uzumaki: couldn't change the app icon files: {e}"),
        }
    });
}

/// KDE's icon loaders (Plasma's task manager and dock included) keep icons in memory and only
/// drop them on this broadcast, the one System Settings sends after an icon theme change. Without
/// it the menu and settings show the new icon but the dock keeps the old one until Plasma restarts.
/// Harmless elsewhere: nothing listens to it outside KDE.
#[cfg(target_os = "linux")]
fn tell_kde_icons_changed() {
    let sent = zbus::blocking::Connection::session().and_then(|c| {
        c.emit_signal(None::<&str>, "/KIconLoader", "org.kde.KIconLoader", "iconChanged", &(0i32,))
    });
    if let Err(e) = sent {
        eprintln!("uzumaki: couldn't tell KDE the icon changed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch prefix like `install.sh` makes: the variants in share/uzumaki/icons and the
    /// dark set installed in share/icons/hicolor. Variant `v` is filled with bytes `[v; 4]`.
    fn install(tag: &str) -> (PathBuf, Plan) {
        let prefix = std::env::temp_dir().join(format!("uzumaki-icons-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&prefix);
        for (dir, byte) in [("dark", 1u8), ("light", 2), ("coral", 3)] {
            for size in SIZES {
                let p = prefix.join(format!("share/uzumaki/icons/{dir}/{size}.png"));
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, [byte, size as u8, 0, 0]).unwrap();
                if byte == 1 {
                    let d = icon_path(&prefix.join("share/icons/hicolor"), size);
                    std::fs::create_dir_all(d.parent().unwrap()).unwrap();
                    std::fs::write(d, [byte, size as u8, 0, 0]).unwrap();
                }
            }
        }
        let exe = prefix.join("bin/uzumaki");
        let plan = plan_for(&exe, false, None).expect("an installed prefix");
        (prefix, plan)
    }

    #[test]
    fn an_installed_prefix_is_planned_and_a_source_tree_is_not() {
        let (prefix, plan) = install("plan");
        assert_eq!(plan.src, prefix.join("share/uzumaki/icons"));
        assert_eq!(plan.hicolor, prefix.join("share/icons/hicolor"));
        assert_eq!(plan.desktop, prefix.join(format!("share/applications/{APP_ID}.desktop")));
        // target/release/uzumaki in a checkout: no share/uzumaki/icons beside it.
        let tree = prefix.join("target/release/uzumaki");
        assert_eq!(plan_for(&tree, false, Some(&prefix)), None);
        // Even with the env switch off a bin/ that has no variants is left alone.
        assert_eq!(plan_for(&prefix.join("other/bin/uzumaki"), false, Some(&prefix)), None);
        // The binary isn't in a bin folder: not an install.
        assert_eq!(plan_for(&prefix.join("uzumaki"), false, None), None);
        std::fs::remove_dir_all(&prefix).unwrap();
    }

    #[test]
    fn variants_without_installed_icons_are_not_applied() {
        let (prefix, _) = install("noicons");
        std::fs::remove_dir_all(prefix.join("share/icons")).unwrap();
        assert_eq!(plan_for(&prefix.join("bin/uzumaki"), false, None), None, "install.sh did not put icons there");
        std::fs::remove_dir_all(&prefix).unwrap();
    }

    #[test]
    fn the_env_switch_targets_the_data_home_with_the_source_variants() {
        let home = std::env::temp_dir().join(format!("uzumaki-icons-dev-{}", std::process::id()));
        let tree = Path::new("/nonexistent/target/release/uzumaki");
        assert_eq!(plan_for(tree, false, Some(&home)), None);
        assert_eq!(plan_for(tree, true, None), None, "no data folder");
        let plan = plan_for(tree, true, Some(&home)).expect("dev switch");
        assert_eq!(plan.hicolor, home.join("icons/hicolor"));
        assert!(plan.src.join("coral/128.png").is_file(), "the source tree's variants");
    }

    #[test]
    fn differs_notices_a_changed_or_missing_file_and_apply_fixes_it() {
        let (prefix, plan) = install("apply");
        assert!(!differs(&plan, IconVariant::Dark), "install.sh's default set is the dark one");
        assert!(differs(&plan, IconVariant::Coral));
        assert_eq!(stale_sizes(&plan, IconVariant::Light).len(), SIZES.len());

        assert_eq!(apply(&plan, IconVariant::Coral).unwrap(), SIZES.len());
        assert!(!differs(&plan, IconVariant::Coral));
        assert_eq!(std::fs::read(icon_path(&plan.hicolor, 48)).unwrap(), [3, 48, 0, 0]);
        assert_eq!(apply(&plan, IconVariant::Coral).unwrap(), 0, "already there: nothing written");

        // One file changed behind our back (or a missing size): only that one is rewritten.
        std::fs::write(icon_path(&plan.hicolor, 24), b"x").unwrap();
        std::fs::remove_file(icon_path(&plan.hicolor, 512)).unwrap();
        assert_eq!(stale_sizes(&plan, IconVariant::Coral), vec![24, 512]);
        assert_eq!(apply(&plan, IconVariant::Coral).unwrap(), 2);
        assert!(!differs(&plan, IconVariant::Coral));

        // Back to the default, as a reinstall would leave it.
        assert_eq!(apply(&plan, IconVariant::Dark).unwrap(), SIZES.len());
        assert!(!differs(&plan, IconVariant::Dark));
        assert!(!icon_path(&plan.hicolor, 16).with_extension("png.new").exists(), "no temp files left");
        std::fs::remove_dir_all(&prefix).unwrap();
    }

    #[test]
    fn a_change_bumps_the_theme_folder_and_the_menu_entry() {
        let (prefix, plan) = install("touch");
        std::fs::create_dir_all(plan.desktop.parent().unwrap()).unwrap();
        std::fs::write(&plan.desktop, "[Desktop Entry]\n").unwrap();
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::open(&plan.hicolor).unwrap().set_modified(old).unwrap();
        std::fs::OpenOptions::new().write(true).open(&plan.desktop).unwrap().set_modified(old).unwrap();
        let mtime = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();

        apply(&plan, IconVariant::Dark).unwrap(); // nothing to change: nothing touched
        assert_eq!(mtime(&plan.hicolor), old);
        apply(&plan, IconVariant::Light).unwrap();
        assert!(mtime(&plan.hicolor) > old, "theme folder bumped");
        assert!(mtime(&plan.desktop) > old, "menu entry bumped");
        assert!(!plan.hicolor.join("icon-theme.cache").exists(), "never creates an icon cache");
        std::fs::remove_dir_all(&prefix).unwrap();
    }

    /// The bundled variants are what the code expects: every size for every variant, and the
    /// dark set is byte for byte the `hicolor/` set that `install.sh` installs by default.
    #[test]
    fn the_bundled_variants_are_complete_and_dark_is_the_default_set() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/icons");
        for icon in IconVariant::ALL {
            for size in SIZES {
                let p = assets.join("variants").join(icon.dir()).join(format!("{size}.png"));
                assert!(p.is_file(), "{} is missing", p.display());
            }
            assert!(assets.join("variants").join(icon.dir()).join("tray.png").is_file());
        }
        for size in SIZES {
            let dark = std::fs::read(assets.join(format!("variants/dark/{size}.png"))).unwrap();
            let default = std::fs::read(icon_path(&assets.join("hicolor"), size)).unwrap();
            assert_eq!(dark, default, "variants/dark/{size}.png vs hicolor");
        }
    }

    /// A scratch package install like the AUR's: `<root>/usr/lib/uzumaki/uzumaki`, the dark set
    /// in `<root>/usr/share/icons/hicolor`, the variants, the desktop entry; plus an empty user
    /// data folder. (The fake prefix is writable on purpose: the libdir layout alone must make
    /// it a system install.)
    fn package(tag: &str) -> (PathBuf, PathBuf, Plan) {
        let root = std::env::temp_dir().join(format!("uzumaki-pkg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let usr = root.join("usr");
        for (dir, byte) in [("dark", 1u8), ("light", 2), ("coral", 3)] {
            for size in SIZES {
                let p = usr.join(format!("share/uzumaki/icons/{dir}/{size}.png"));
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, [byte, size as u8, 0, 0]).unwrap();
                if byte == 1 {
                    let d = icon_path(&usr.join("share/icons/hicolor"), size);
                    std::fs::create_dir_all(d.parent().unwrap()).unwrap();
                    std::fs::write(d, [byte, size as u8, 0, 0]).unwrap();
                }
            }
        }
        let desk = usr.join(format!("share/applications/{APP_ID}.desktop"));
        std::fs::create_dir_all(desk.parent().unwrap()).unwrap();
        std::fs::write(&desk, "[Desktop Entry]\nName=Uzumaki\nExec=uzumaki\nIcon=io.github.hanami117.uzumaki\n").unwrap();
        let home = root.join("home/.local/share");
        std::fs::create_dir_all(&home).unwrap();
        let plan = plan_for(&usr.join("lib/uzumaki/uzumaki"), false, Some(&home)).expect("a package install");
        (root, home, plan)
    }

    /// Every file under `dir` with its bytes, to compare before and after.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    todo.push(p);
                } else {
                    out.push((p.clone(), std::fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_package_install_plans_user_overrides() {
        let (root, home, plan) = package("plan");
        let usr = root.join("usr");
        assert_eq!(plan.src, usr.join("share/uzumaki/icons"));
        assert_eq!(plan.hicolor, home.join("icons/hicolor"));
        assert_eq!(plan.desktop, home.join(format!("applications/{APP_ID}.desktop")));
        assert_eq!(
            plan.mode,
            Mode::UserOverride { system_desktop: Some(usr.join(format!("share/applications/{APP_ID}.desktop"))) }
        );
        // No data folder: nowhere to write, so no plan.
        assert_eq!(plan_for(&usr.join("lib/uzumaki/uzumaki"), false, None), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_choice_writes_user_files_and_never_the_system_ones() {
        let (root, home, plan) = package("write");
        let usr = root.join("usr");
        let before = snapshot(&usr);

        assert_eq!(apply(&plan, IconVariant::Coral).unwrap(), SIZES.len() + 1);
        assert_eq!(std::fs::read(icon_path(&plan.hicolor, 48)).unwrap(), [3, 48, 0, 0]);
        let desktop = std::fs::read_to_string(&plan.desktop).unwrap();
        assert!(desktop.starts_with("[Desktop Entry]\n"), "{desktop}");
        assert!(desktop.contains("\nExec=uzumaki\n") && desktop.contains("\nName=Uzumaki\n"), "{desktop}");
        assert!(has_marker(&desktop));
        assert!(plan.desktop.starts_with(&home) && plan.hicolor.starts_with(&home));
        assert_eq!(snapshot(&usr), before, "the system files are byte for byte the same");

        assert_eq!(apply(&plan, IconVariant::Coral).unwrap(), 0, "already there");
        assert_eq!(apply(&plan, IconVariant::Light).unwrap(), SIZES.len(), "icons change, desktop stays");
        assert_eq!(std::fs::read(icon_path(&plan.hicolor, 16)).unwrap(), [2, 16, 0, 0]);
        assert_eq!(snapshot(&usr), before);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn choosing_dark_removes_the_user_copies_we_wrote() {
        let (root, _, plan) = package("dark");
        assert_eq!(apply(&plan, IconVariant::Dark).unwrap(), 0, "nothing to remove yet");
        apply(&plan, IconVariant::Coral).unwrap();
        assert_eq!(apply(&plan, IconVariant::Dark).unwrap(), SIZES.len() + 1);
        for size in SIZES {
            assert!(!icon_path(&plan.hicolor, size).exists());
        }
        assert!(!plan.desktop.exists());
        assert!(root.join(format!("usr/share/applications/{APP_ID}.desktop")).is_file());
        assert!(!differs(&plan, IconVariant::Dark));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn user_made_files_without_our_marker_are_left_alone() {
        let (root, _, plan) = package("theirs");
        let mine = icon_path(&plan.hicolor, 64);
        std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
        std::fs::write(&mine, b"my own icon").unwrap();
        std::fs::create_dir_all(plan.desktop.parent().unwrap()).unwrap();
        std::fs::write(&plan.desktop, "[Desktop Entry]\nName=Mine\nExec=mine\n").unwrap();

        // Dark: neither is ours, so neither goes.
        assert_eq!(apply(&plan, IconVariant::Dark).unwrap(), 0);
        assert_eq!(std::fs::read(&mine).unwrap(), b"my own icon");
        assert!(std::fs::read_to_string(&plan.desktop).unwrap().contains("Name=Mine"));

        // A choice replaces the icon (that is what was asked for) but keeps their desktop entry.
        apply(&plan, IconVariant::Coral).unwrap();
        assert_eq!(std::fs::read(&mine).unwrap(), [3, 64, 0, 0]);
        assert!(std::fs::read_to_string(&plan.desktop).unwrap().contains("Name=Mine"));
        // Back to Dark: the icons are ours again and go; the desktop entry still isn't ours.
        apply(&plan, IconVariant::Dark).unwrap();
        assert!(!mine.exists());
        assert!(plan.desktop.is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_package_upgrade_refreshes_our_desktop_copy() {
        let (root, _, plan) = package("upgrade");
        apply(&plan, IconVariant::Coral).unwrap();
        let Mode::UserOverride { system_desktop: Some(sys) } = &plan.mode else { panic!() };
        std::fs::write(sys, "[Desktop Entry]\nName=Uzumaki\nExec=uzumaki --new\n").unwrap();
        assert_eq!(apply(&plan, IconVariant::Coral).unwrap(), 1);
        assert!(std::fs::read_to_string(&plan.desktop).unwrap().contains("Exec=uzumaki --new"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_install_sh_prefix_still_overwrites_in_place() {
        let (prefix, plan) = install("inplace");
        assert_eq!(plan.mode, Mode::InPlace);
        let home = prefix.join("elsewhere");
        let plan = plan_for(&prefix.join("bin/uzumaki"), false, Some(&home)).unwrap();
        assert_eq!(plan.mode, Mode::InPlace, "a data folder doesn't turn a user prefix into an override");
        assert_eq!(plan.hicolor, prefix.join("share/icons/hicolor"));
        assert_eq!(apply(&plan, IconVariant::Coral).unwrap(), SIZES.len());
        assert_eq!(std::fs::read(icon_path(&plan.hicolor, 32)).unwrap(), [3, 32, 0, 0]);
        assert!(!home.exists(), "nothing written elsewhere");
        std::fs::remove_dir_all(&prefix).unwrap();
    }

    #[test]
    fn a_source_tree_does_nothing_without_the_switch() {
        let home = std::env::temp_dir().join(format!("uzumaki-icons-tree-{}", std::process::id()));
        let tree = Path::new("/nonexistent/repo/target/release/uzumaki");
        assert_eq!(plan_for(tree, false, Some(&home)), None);
        assert_eq!(plan_for(Path::new("/nonexistent/repo/target/debug/deps/uzumaki"), false, Some(&home)), None);
    }
}
