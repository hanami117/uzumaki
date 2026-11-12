#!/usr/bin/env bash
# Builds Uzumaki.app and a disk image for this Mac, signed ad hoc (free; an app built on the Mac
# it runs on isn't quarantined, so it opens without Gatekeeper warnings).
#
#   packaging/macos/build.sh      → target/macos.noindex/Uzumaki.app and Uzumaki-<version>.dmg there
#                                   (Uzumaki-<version>-<build>.dmg from build 2 on: see BUILD)
#
# The build copy is kept out of Launchpad, Spotlight and Open With (only the installed copy
# should show): the folder ends in .noindex, so Spotlight never indexes it (and so never hands the
# app to LaunchServices), and the bundle is unregistered from LaunchServices once built.
#
# Needs, once:
#   - Rust, and librsvg for the icon (`brew install librsvg`);
#   - the bridge's castLabs Electron: `cd bridge && npm ci && node node_modules/electron/install.js`;
#   - its Widevine VMP signature for streaming, from a free castLabs EVS account:
#     `python3 -m castlabs_evs.vmp sign-pkg bridge/node_modules/electron/dist` (see CONTRIBUTING.md).
#
# The bundle carries Electron.app unchanged (ditto), so that VMP signature stays valid, and the
# app's settings, caches and Apple Music login are the same ones `cargo run` uses.
set -euo pipefail

cd "$(dirname "$0")/../.."
say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf 'build.sh: %s\n' "$*" >&2; exit 1; }

[[ $(uname -s) == Darwin ]] || die "run this on a Mac"
command -v rsvg-convert >/dev/null || die "rsvg-convert is missing (brew install librsvg)"

# The one version: [workspace.package] in Cargo.toml (also About, `--version`, the update check).
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[[ -n $version ]] || die "no version in Cargo.toml's [workspace.package]"
grep -q "\"version\": \"$version\"" bridge/package.json || die "bridge/package.json's version isn't $version"
# The build number: BUILD at the top of the repo (also About, `--version`, the update check). It
# goes up when a package is rebuilt without code changes. Info.plist's CFBundleVersion is the build,
# CFBundleShortVersionString the version.
build=$(tr -d '[:space:]' <BUILD)
[[ $build =~ ^[1-9][0-9]*$ ]] || die "BUILD must hold a whole number above 0 (got '$build')"
# File names carry the build from build 2 on: Uzumaki-0.1.0.dmg, then Uzumaki-0.1.0-2.dmg.
if ((build == 1)); then release=$version; else release=$version-$build; fi
out=${CARGO_TARGET_DIR:-target}/macos.noindex
app=$out/Uzumaki.app
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
# Forget a bundle in LaunchServices (no error if it was never there).
unregister() { [[ -x $lsregister ]] && "$lsregister" -u "$1" >/dev/null 2>&1 || true; }
dist=bridge/node_modules/electron/dist
evs=${EVS_PYTHON:-$HOME/.venvs/castlabs-evs/bin/python}

vmp_ok() { "$evs" -m castlabs_evs.vmp verify-pkg "$1" 2>&1 | grep -q 'Signature is valid'; }

# --- Dock icons ---------------------------------------------------------------------------------
# The running app sets its Dock icon to Settings → Appearance → App icon (app/src/dock_icon.rs). It
# embeds these three PNGs, so they are redrawn from the SVGs before the build (the bundle icon's
# proportions, at 512 px).
say "Drawing the Dock icons"
for v in dark:uzumaki light:uzumaki-light coral:uzumaki-coral; do
    rsvg-convert -w 461 -h 461 --page-width 512 --page-height 512 --left 25 --top 25 \
        "assets/icons/${v#*:}.svg" -o "assets/icons/macos/dock-${v%%:*}.png"
done

# --- Build --------------------------------------------------------------------------------------
say "Building Uzumaki $version (build $build, release)"
cargo build --release --locked -p uzumaki -p uzumaki-ctl
bin=${CARGO_TARGET_DIR:-target}/release

[[ -x $dist/Electron.app/Contents/MacOS/Electron ]] ||
    die "castLabs Electron is missing: cd bridge && npm ci && node node_modules/electron/install.js"
if [[ -x $evs ]]; then
    vmp_ok "$dist" || die "Electron has no streaming VMP signature: $evs -m castlabs_evs.vmp sign-pkg $dist"
else
    say "No castLabs EVS at $evs (set EVS_PYTHON): not checking the VMP signature"
fi

# --- Assemble -----------------------------------------------------------------------------------
say "Assembling $app"
# Builds before this one went to target/macos, where Spotlight found the app: unregister and remove it.
legacy=${CARGO_TARGET_DIR:-target}/macos/Uzumaki.app
if [[ -d $legacy ]]; then
    unregister "$legacy"
    rm -rf "$legacy"
fi
mkdir -p "$out"
# .noindex already keeps Spotlight out; this says so for tools that only look for the file.
touch "$out/.metadata_never_index"
unregister "$app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/bridge/node_modules/electron/dist"
cp "$bin/uzumaki" "$bin/uzu" "$app/Contents/MacOS/"
# The bridge needs only its own files and Electron (main.js loads nothing from npm). bridge.rs
# finds it in Contents/Resources/bridge.
cp bridge/main.js bridge/preload.js bridge/signin.js bridge/package.json "$app/Contents/Resources/bridge/"
# The licence and the notices travel with the app (GPL-3.0-or-later; Rune, its fonts, Electron).
cp LICENSE THIRD_PARTY_NOTICES.md "$app/Contents/Resources/"
# ditto keeps the framework symlinks and Electron's own signatures as they are.
ditto "$dist/Electron.app" "$app/Contents/Resources/bridge/node_modules/electron/dist/Electron.app"

# The icons: the swirl tile drawn at Apple's icon size (an 824 px body on a 1024 px canvas; the
# SVG's tile is 458 of its 512 units, so the whole SVG is drawn 921 px wide), then every size.
# One per App icon choice in Resources/icons; the bundle icon is the dark one. The app copies the
# chosen one over Uzumaki.icns and re-signs (app/src/bundle_icon.rs), so the Finder shows it too.
work=$(mktemp -d)
# The disk image's staging copy is an app too: forget it with the folder.
trap 'unregister "$work/dmg/Uzumaki.app"; rm -rf "$work"' EXIT
mkdir "$app/Contents/Resources/icons"
for v in dark:uzumaki light:uzumaki-light coral:uzumaki-coral; do
    name=${v%%:*}
    rsvg-convert -w 921 -h 921 --page-width 1024 --page-height 1024 --left 51 --top 51 \
        "assets/icons/${v#*:}.svg" -o "$work/$name-1024.png"
    iconset=$work/$name.iconset
    mkdir "$iconset"
    for px in 16 32 128 256 512; do
        sips -z "$px" "$px" "$work/$name-1024.png" --out "$iconset/icon_${px}x${px}.png" >/dev/null
        sips -z "$((px * 2))" "$((px * 2))" "$work/$name-1024.png" --out "$iconset/icon_${px}x${px}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/icons/$name.icns"
done
cp "$app/Contents/Resources/icons/dark.icns" "$app/Contents/Resources/Uzumaki.icns"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>Uzumaki</string>
    <key>CFBundleExecutable</key><string>uzumaki</string>
    <key>CFBundleIconFile</key><string>Uzumaki</string>
    <key>CFBundleIdentifier</key><string>io.github.hanami117.uzumaki</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>Uzumaki</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$build</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>© 2026 Thomas Lonigro. Free software under the GNU GPL, version 3 or later.</string>
</dict>
</plist>
PLIST
printf 'APPL????' >"$app/Contents/PkgInfo"

# --- Sign ---------------------------------------------------------------------------------------
# Ad hoc, inside out, no --deep. Electron.app keeps the signature it came with (re-signing it would
# change its identity, and macOS would ask again for the keychain item that holds the login).
say "Signing (ad hoc)"
codesign --force --sign - "$app/Contents/MacOS/uzu"
codesign --force --sign - "$app"
codesign --verify --strict "$app" || die "the bundle's signature doesn't verify"
if [[ -x $evs ]]; then
    vmp_ok "$app/Contents/Resources/bridge/node_modules/electron/dist" || die "the bundled Electron lost its VMP signature"
fi

# --- Disk image ---------------------------------------------------------------------------------
dmg=$out/Uzumaki-$release.dmg
say "Making $dmg"
stage=$work/dmg
mkdir "$stage"
ditto "$app" "$stage/Uzumaki.app"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname Uzumaki -srcfolder "$stage" -format UDZO -ov "$dmg"

# Signing and copying can register the bundle; the build copy must not be a second Uzumaki.
unregister "$app"

say "Done: $(du -sh "$dmg" | cut -f1) $dmg"
say "Open it and drag Uzumaki to Applications. Quit any running Uzumaki first."
