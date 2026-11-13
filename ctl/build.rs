//! Records where a dev build comes from, for the version shown in About and `--version`.
//!
//! It emits `UZUMAKI_GIT` = `branch<TAB>short hash<TAB>dirty (0/1)<TAB>at release tag (0/1)`, or an
//! empty string when git facts are unavailable (a source tarball has no `.git`, git may be missing).
//! The rule that turns these into text is `git_suffix` in `src/lib.rs`. This script never fails the
//! build.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn facts() -> Option<String> {
    let hash = git(&["rev-parse", "--short=7", "HEAD"]).filter(|h| !h.is_empty())?;
    let branch = git(&["symbolic-ref", "--short", "-q", "HEAD"]).filter(|b| !b.is_empty()).unwrap_or_else(|| "detached".into());
    let dirty = git(&["status", "--porcelain"]).is_none_or(|s| !s.is_empty());

    // The release tag for this version and build: vX.Y.Z for build 1, vX.Y.Z-N after.
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let build: u32 = std::fs::read_to_string("../BUILD").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(1);
    let tag = if build == 1 { format!("v{version}") } else { format!("v{version}-{build}") };
    let at_tag = git(&["tag", "--points-at", "HEAD"]).is_some_and(|t| t.lines().any(|l| l == tag));

    Some(format!("{branch}\t{hash}\t{}\t{}", dirty as u8, at_tag as u8))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../BUILD");
    // HEAD moves on checkout and commit (a commit rewrites the branch ref); the index changes on `git add`.
    for p in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", p]).filter(|s| Path::new(s).exists()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]).and_then(|r| git(&["rev-parse", "--git-path", &r])) {
        println!("cargo:rerun-if-changed={r}");
    }
    println!("cargo:rerun-if-changed=../.git/refs/tags");
    println!("cargo:rustc-env=UZUMAKI_GIT={}", facts().unwrap_or_default());
}
