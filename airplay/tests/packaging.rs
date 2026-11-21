//! The cliairplay helper every package ships: one pinned tag, the same in the build script, the AUR
//! source package and the licence note, and each package puts the helper where `helper::find` looks.

use std::fs;
use std::path::PathBuf;

fn repo(path: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(path);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The value of `name=` at the start of a line (shell assignment, no quotes).
fn assignment(text: &str, name: &str) -> String {
    let key = format!("{name}=");
    text.lines()
        .find_map(|l| l.strip_prefix(&key))
        .unwrap_or_else(|| panic!("no {key} line"))
        .trim()
        .to_string()
}

#[test]
fn the_helper_tag_is_the_same_everywhere() {
    let tag = assignment(&repo("scripts/airplay/build-cliairplay.sh"), "TAG");
    assert!(tag.starts_with('v'), "{tag}");
    assert_eq!(assignment(&repo("packaging/aur/PKGBUILD"), "_cliairplay"), tag, "packaging/aur/PKGBUILD _cliairplay");
    assert!(repo("packaging/CLIAIRPLAY_LICENSE.txt").contains(&format!("(tag {tag})")), "packaging/CLIAIRPLAY_LICENSE.txt names {tag}");
}

#[test]
fn every_package_installs_the_helper_where_the_app_looks() {
    // install.sh: bin/uzumaki → ../share/uzumaki/cliairplay.
    let install = repo("install.sh");
    assert!(install.contains(r#"BIN="$PREFIX/bin/uzumaki""#));
    assert!(install.contains(r#"SHARE="$PREFIX/share/uzumaki""#));
    assert!(install.contains(r#"install -Dm755 "$helper" "$SHARE/cliairplay""#));
    // AUR and AppImage (and uzumaki-bin, which copies the AppImage's usr/lib/uzumaki): beside the binary.
    let aur = repo("packaging/aur/PKGBUILD");
    assert!(aur.contains(r#"install -Dm755 target/release/uzumaki "$lib/uzumaki""#));
    assert!(aur.contains(r#"install -Dm755 target/cliairplay/cliairplay "$lib/cliairplay""#));
    let appimage = repo("packaging/appimage/build.sh");
    assert!(appimage.contains(r#"install -Dm755 "$target/release/uzumaki" "$lib/uzumaki""#));
    assert!(appimage.contains(r#"install -m755 target/cliairplay/cliairplay "$lib/cliairplay""#));
    assert!(repo("packaging/aur-bin/PKGBUILD").contains(r#"cp -a --no-preserve=ownership "usr/lib/$_name" "$pkgdir/usr/lib/""#));
    // And each ships the licence note.
    for (file, text) in [("install.sh", &install), ("packaging/aur/PKGBUILD", &aur), ("packaging/appimage/build.sh", &appimage)] {
        assert!(text.contains("packaging/CLIAIRPLAY_LICENSE.txt"), "{file} ships the licence note");
    }
    assert!(repo("packaging/macos/build.sh").contains("packaging/CLIAIRPLAY_LICENSE.txt"));
}

/// The release workflow rewrites the tarball's checksum line in place (`sed s/^sha256sums=(.*)/`):
/// the helper's sources must be added with `+=`, never in that line.
#[test]
fn the_aur_checksum_line_stays_the_tarballs_alone() {
    let aur = repo("packaging/aur/PKGBUILD");
    let lines: Vec<_> = aur.lines().filter(|l| l.starts_with("sha256sums=")).collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let sum = lines[0].strip_prefix("sha256sums=('").and_then(|s| s.strip_suffix("')")).expect("sha256sums=('<one sum>')");
    assert!(sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()), "{sum}");
}
