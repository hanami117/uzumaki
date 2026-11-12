#!/usr/bin/env bash
# Prepares a release of Uzumaki. It builds and writes files only; it never publishes anything
# (no `gh`, no `git push`). At the end it prints what to run and what to upload.
#
#   packaging/release.sh             checks, DMG (macOS), source tarball, checksums, notes, latest.json
#   packaging/release.sh --no-dmg    the same without the DMG (e.g. on Linux)
#
# Before: the version in Cargo.toml's [workspace.package] and bridge/package.json, a CHANGELOG.md
# section "## [<version>]", everything committed, and an annotated tag on HEAD:
#   git tag -a v<version> -m "Uzumaki <version>"
# Run it in the PUBLIC repo's checkout: the tarball is `git archive` of the tag, so it holds
# exactly what that repo has (and the PKGBUILD's checksum is the tarball's).
#
# Out: target/release-<version>/
#   Uzumaki-<version>.dmg (+ .sha256)       macOS disk image (packaging/macos/build.sh)
#   uzumaki-<version>.tar.gz (+ .sha256)    source tarball, top folder uzumaki-<version>/
#   SHA256SUMS                              both, in `shasum -c` form
#   notes.md                                release notes (CHANGELOG section + install notes)
#   latest.json                             for getuzumaki.app/latest.json (the update notice)
set -euo pipefail

cd "$(dirname "$0")/.."
say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf 'release.sh: %s\n' "$*" >&2; exit 1; }

REPO=hanami117/uzumaki
dmg=1
case "${1:-}" in
    --no-dmg) dmg=0 ;;
    -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit ;;
    "") ;;
    *) die "unknown option '$1' (try --help)" ;;
esac

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

# --- Checks -------------------------------------------------------------------------------------
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "no semver version in Cargo.toml's [workspace.package] (got '$version')"
tag=v$version
grep -q "\"version\": \"$version\"" bridge/package.json || die "bridge/package.json's version isn't $version"
grep -q "^## \[$version\]" CHANGELOG.md || die "CHANGELOG.md has no '## [$version]' section"

[[ -z $(git status --porcelain) ]] || die "the working tree isn't clean (commit or stash first):
$(git status --short)"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
    die "no tag $tag. Create it on the release commit: git tag -a $tag -m \"Uzumaki $version\""
[[ $(git cat-file -t "$tag") == tag ]] || die "$tag is a lightweight tag; make it annotated: git tag -fa $tag -m \"Uzumaki $version\""
[[ $(git rev-parse "$tag^{commit}") == $(git rev-parse HEAD) ]] ||
    die "$tag isn't HEAD (it is $(git rev-parse --short "$tag^{commit}")); check out the tag or move it"
# The public repo has no internal notes; finding them means this is the private checkout.
for internal in docs/ROADMAP.md docs/ONBOARDING.md CLAUDE.md; do
    git cat-file -e "$tag:$internal" 2>/dev/null && warn "$internal is in $tag: is this the public repo? (fine for a trial run)"
done

out=${CARGO_TARGET_DIR:-target}/release-$version
rm -rf "$out"
mkdir -p "$out"

# --- macOS disk image ---------------------------------------------------------------------------
if ((dmg)); then
    [[ $(uname -s) == Darwin ]] || die "the DMG needs a Mac (use --no-dmg elsewhere)"
    say "Building the macOS disk image"
    packaging/macos/build.sh
    cp "${CARGO_TARGET_DIR:-target}/macos/Uzumaki-$version.dmg" "$out/"
    [[ -z $(git status --porcelain) ]] || die "build.sh changed tracked files (the Dock icons?): $(git status --short)"
fi

# --- Source tarball -----------------------------------------------------------------------------
say "Making the source tarball from $tag"
src=uzumaki-$version.tar.gz
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
dmg_sha=$([[ -e $out/Uzumaki-$version.dmg ]] && sha256 "$out/Uzumaki-$version.dmg" || echo "(no DMG built)")

# --- Release notes ------------------------------------------------------------------------------
say "Writing release notes from CHANGELOG.md"
{
    # The section's body: after its heading, up to the next "## [" heading or the link list.
    awk -v v="$version" '
        $0 ~ "^## \\[" v "\\]" { on = 1; next }
        on && (/^## \[/ || /^\[[^]]+\]: /) { exit }
        on { print }
    ' CHANGELOG.md | sed -e '/./,$!d'
    cat <<NOTES

### Install
- **macOS** (Apple silicon, macOS 13 or later): download \`Uzumaki-$version.dmg\`, drag Uzumaki to Applications. The
  app is not notarized, so on first launch macOS refuses it: open **System Settings → Privacy & Security** and click
  **Open Anyway** (or run \`xattr -dr com.apple.quarantine /Applications/Uzumaki.app\`).
  With Homebrew: \`brew install --cask hanami117/uzumaki/uzumaki\`.
- **Arch Linux**: \`yay -S uzumaki\` (AUR, builds from source).
- **Other Linux**: download \`$src\`, unpack it and run \`./install.sh\` (needs Rust, Node.js and npm).

Checksums are in \`SHA256SUMS\`.
NOTES
} >"$out/notes.md"

# --- latest.json --------------------------------------------------------------------------------
dl=https://github.com/$REPO/releases/download/$tag
cat >"$out/latest.json" <<JSON
{
  "version": "$version",
  "date": "$(git log -1 --format=%cs "$tag")",
  "notes_url": "https://github.com/$REPO/releases/tag/$tag",
  "downloads": {
    "macos_dmg": "$dl/Uzumaki-$version.dmg",
    "linux_tarball": "$dl/$src",
    "appimage": "$dl/Uzumaki-$version-x86_64.AppImage",
    "aur": "https://aur.archlinux.org/packages/uzumaki",
    "brew": "brew install --cask hanami117/uzumaki/uzumaki"
  }
}
JSON
python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$out/latest.json" 2>/dev/null || warn "latest.json didn't parse (no python3 to check?)"

# --- What to do next (nothing below has been run) -----------------------------------------------
shopt -s nullglob
files=$(printf '%s ' "$out"/*.dmg "$out"/*.tar.gz "$out"/*.sha256 "$out/SHA256SUMS")
cat <<DONE

$(say "Ready in $out. Nothing was published.")

1. Push the tag to the public repo, then create the release:

   git push origin $tag
   gh release create $tag --repo $REPO --title "Uzumaki $version" --notes-file $out/notes.md --verify-tag \\
       $files

   The AppImage (Uzumaki-$version-x86_64.AppImage, linked from latest.json) can't be built here:
   build it on Linux (x86_64) and upload it to the same release:
   gh release upload $tag --repo $REPO Uzumaki-$version-x86_64.AppImage

2. Publish latest.json on the site (site/latest.json on main), after the release is up:

$(sed 's/^/   /' "$out/latest.json")

3. Fill in the packages:
   packaging/aur/PKGBUILD          pkgver=$version  sha256sums=('$src_sha')
                                   then: makepkg --printsrcinfo > .SRCINFO, push to aur.archlinux.org/uzumaki.git
   packaging/homebrew/uzumaki.rb   version "$version"  sha256 "$dmg_sha"
                                   then: copy to the tap repo hanami117/homebrew-uzumaki as Casks/uzumaki.rb
DONE
