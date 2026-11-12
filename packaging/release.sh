#!/usr/bin/env bash
# Prepares a release of Uzumaki. It builds and writes files only; it never publishes anything
# (no `gh`, no `git push`). At the end it prints what to run and what to upload.
#
#   packaging/release.sh             checks, DMG (macOS), source tarball, checksums, notes, latest.json
#   packaging/release.sh --no-dmg    the same without the DMG (e.g. on Linux)
#
# Before: the version in Cargo.toml's [workspace.package] and bridge/package.json, the build
# number in BUILD, a CHANGELOG.md section "## [<version>]", everything committed, and an
# annotated tag on HEAD:
#   git tag -a v<version> -m "Uzumaki <version>"                         (build 1)
#   git tag -a v<version>-<build> -m "Uzumaki <version> (build <build>)"  (build 2 and up)
# The build number goes up when the packages are rebuilt without code changes (e.g. a new
# Electron signature): raise BUILD, commit, tag, run this again. A new version starts at build 1.
# Run it in the PUBLIC repo's checkout: the tarball is `git archive` of the tag, so it holds
# exactly what that repo has (and the PKGBUILD's checksum is the tarball's).
#
# <release> below is <version> for build 1 and <version>-<build> after it.
# Out: target/release-<release>/
#   Uzumaki-<release>.dmg (+ .sha256)       macOS disk image (packaging/macos/build.sh)
#   Uzumaki-macOS.dmg (+ .sha256)           the same file under a stable name (the site's button)
#   uzumaki-<release>.tar.gz (+ .sha256)    source tarball, top folder uzumaki-<version>/
#   SHA256SUMS                              all of them, in `shasum -c` form
#   notes.md                                release notes (CHANGELOG section + install notes)
#   latest.json                             for getuzumaki.app/latest.json (the update notice)
# The AppImage is built on Linux (packaging/appimage/build.sh), also under two names:
# Uzumaki-<release>-x86_64.AppImage and Uzumaki-x86_64.AppImage (+ .sha256, + .zsync).
set -euo pipefail

cd "$(dirname "$0")/.."
say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf 'release.sh: %s\n' "$*" >&2; exit 1; }

REPO=hanami117/uzumaki
# Stable file names (the same bytes as the versioned ones) so links can say releases/latest/download/.
DMG_LATEST=Uzumaki-macOS.dmg
APPIMAGE_LATEST=Uzumaki-x86_64.AppImage
dmg=1
case "${1:-}" in
    --no-dmg) dmg=0 ;;
    -h|--help) sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'; exit ;;
    "") ;;
    *) die "unknown option '$1' (try --help)" ;;
esac

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

# --- Checks -------------------------------------------------------------------------------------
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "no semver version in Cargo.toml's [workspace.package] (got '$version')"
# The build number (see BUILD): also in About, `--version`, Info.plist and latest.json.
build=$(tr -d '[:space:]' <BUILD)
[[ $build =~ ^[1-9][0-9]*$ ]] || die "BUILD must hold a whole number above 0 (got '$build')"
if ((build == 1)); then release=$version; else release=$version-$build; fi
tag=v$release
title="Uzumaki $version"
((build == 1)) || title="Uzumaki $version (build $build)"
grep -q "\"version\": \"$version\"" bridge/package.json || die "bridge/package.json's version isn't $version"
grep -q "^## \[$version\]" CHANGELOG.md || die "CHANGELOG.md has no '## [$version]' section"

[[ -z $(git status --porcelain) ]] || die "the working tree isn't clean (commit or stash first):
$(git status --short)"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
    die "no tag $tag. Create it on the release commit: git tag -a $tag -m \"$title\""
[[ $(git cat-file -t "$tag") == tag ]] || die "$tag is a lightweight tag; make it annotated: git tag -fa $tag -m \"$title\""
[[ $(git rev-parse "$tag^{commit}") == $(git rev-parse HEAD) ]] ||
    die "$tag isn't HEAD (it is $(git rev-parse --short "$tag^{commit}")); check out the tag or move it"
# The public repo has no internal notes; finding them means this is the private checkout.
for internal in docs/ROADMAP.md docs/ONBOARDING.md CLAUDE.md; do
    git cat-file -e "$tag:$internal" 2>/dev/null && warn "$internal is in $tag: is this the public repo? (fine for a trial run)"
done

out=${CARGO_TARGET_DIR:-target}/release-$release
rm -rf "$out"
mkdir -p "$out"

# --- macOS disk image ---------------------------------------------------------------------------
if ((dmg)); then
    [[ $(uname -s) == Darwin ]] || die "the DMG needs a Mac (use --no-dmg elsewhere)"
    say "Building the macOS disk image"
    packaging/macos/build.sh
    cp "${CARGO_TARGET_DIR:-target}/macos.noindex/Uzumaki-$release.dmg" "$out/"
    # A stable name for the newest DMG: .../releases/latest/download/Uzumaki-macOS.dmg.
    cp "$out/Uzumaki-$release.dmg" "$out/$DMG_LATEST"
    [[ -z $(git status --porcelain) ]] || die "build.sh changed tracked files (the Dock icons?): $(git status --short)"
fi

# --- Source tarball -----------------------------------------------------------------------------
say "Making the source tarball from $tag"
src=uzumaki-$release.tar.gz
git archive --format=tar.gz --prefix="uzumaki-$version/" -o "$out/$src" "$tag"

# --- Checksums ----------------------------------------------------------------------------------
say "Writing checksums"
: >"$out/SHA256SUMS"
for f in "$out"/*.dmg "$out"/*.tar.gz; do
    [[ -e $f ]] || continue
    name=$(basename "$f")
    sum=$(sha256 "$f")
    printf '%s  %s\n' "$sum" "$name" >"$f.sha256"
    printf '%s  %s\n' "$sum" "$name" >>"$out/SHA256SUMS"
done
src_sha=$(sha256 "$out/$src")
dmg_sha=$([[ -e $out/Uzumaki-$release.dmg ]] && sha256 "$out/Uzumaki-$release.dmg" || echo "(no DMG built)")

# --- Release notes ------------------------------------------------------------------------------
say "Writing release notes from CHANGELOG.md"
{
    # The section's body: after its heading, up to the next "## [" heading or the link list.
    awk -v v="$version" '
        $0 ~ "^## \\[" v "\\]" { on = 1; next }
        on && (/^## \[/ || /^\[[^]]+\]: /) { exit }
        on { print }
    ' CHANGELOG.md | sed -e '/./,$!d'
    if ((build > 1)); then
        printf '\nThis is build %s of %s: the same code, packaged again.\n' "$build" "$version"
    fi
    cat <<NOTES

### Install
- **macOS** (Apple silicon, macOS 13 or later): download \`Uzumaki-$release.dmg\`, drag Uzumaki to Applications. The
  app is not notarized, so on first launch macOS refuses it: open **System Settings → Privacy & Security** and click
  **Open Anyway** (or run \`xattr -dr com.apple.quarantine /Applications/Uzumaki.app\`).
  With Homebrew: \`brew install --cask hanami117/uzumaki/uzumaki\`.
- **Arch Linux**: \`yay -S uzumaki\` (AUR, builds from source).
- **Other Linux** (x86_64): download \`Uzumaki-$release-x86_64.AppImage\`, \`chmod +x\` it and run it. It needs FUSE
  (\`fuse2\` or \`fuse3\`); without it, run it with \`--appimage-extract-and-run\`. Or build from source: download
  \`$src\`, unpack it and run \`./install.sh\` (needs Rust, Node.js and npm).

Checksums are in \`SHA256SUMS\`.
NOTES
} >"$out/notes.md"

# --- latest.json --------------------------------------------------------------------------------
dl=https://github.com/$REPO/releases/download/$tag
# Always the newest release's file: the site's buttons use these too.
latest=https://github.com/$REPO/releases/latest/download
cat >"$out/latest.json" <<JSON
{
  "version": "$version",
  "build": $build,
  "date": "$(git log -1 --format=%cs "$tag")",
  "notes_url": "https://github.com/$REPO/releases/tag/$tag",
  "downloads": {
    "macos_dmg": "$latest/$DMG_LATEST",
    "linux_tarball": "$dl/$src",
    "appimage": "$latest/$APPIMAGE_LATEST",
    "aur": "https://aur.archlinux.org/packages/uzumaki",
    "brew": "brew install --cask hanami117/uzumaki/uzumaki"
  }
}
JSON
python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$out/latest.json" 2>/dev/null || warn "latest.json didn't parse (no python3 to check?)"

# --- What to do next (nothing below has been run) -----------------------------------------------
# Homebrew orders "0.1.0,2" after "0.1.0", so `brew upgrade` sees a rebuild.
if ((build == 1)); then cask_version=$version; else cask_version=$version,$build; fi
shopt -s nullglob
files=$(printf '%s ' "$out"/*.dmg "$out"/*.tar.gz "$out"/*.sha256 "$out/SHA256SUMS")
cat <<DONE

$(say "Ready in $out. Nothing was published.")

1. Push the tag to the public repo, then create the release:

   git push origin $tag
   gh release create $tag --repo $REPO --title "$title" --notes-file $out/notes.md --verify-tag \\
       $files

   The AppImage can't be built here: build it on Linux x86_64 with build $build
   (packaging/appimage/container.sh in a checkout of $tag, or container.sh $src; it builds in an
   ubuntu:22.04 container, see build.sh) and upload all
   its files to the same release:
   gh release upload $tag --repo $REPO target/appimage/Uzumaki-$release-x86_64.AppImage{,.sha256} \\
       target/appimage/$APPIMAGE_LATEST{,.sha256,.zsync}

2. Publish latest.json on the site (site/latest.json on main), after the release is up.
   The site's download buttons should use the same stable links (they follow each new release):
     $latest/$DMG_LATEST
     $latest/$APPIMAGE_LATEST
   (the source tarball keeps its versioned name: $dl/$src)

$(sed 's/^/   /' "$out/latest.json")

3. Fill in the packages:
   packaging/aur/PKGBUILD          pkgver=$version  _build=$build  sha256sums=('$src_sha')
                                   pkgrel: 1 for a new version, else one more than the AUR's
                                   then: makepkg --printsrcinfo > .SRCINFO, push to aur.archlinux.org/uzumaki.git
   packaging/homebrew/uzumaki.rb   version "$cask_version"  sha256 "$dmg_sha"
                                   then: copy to the tap repo hanami117/homebrew-uzumaki as Casks/uzumaki.rb
DONE
