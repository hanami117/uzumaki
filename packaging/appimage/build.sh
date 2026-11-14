#!/usr/bin/env bash
# Builds Uzumaki-<release>-x86_64.AppImage from this checkout. Runs on Linux x86_64.
#
#   packaging/appimage/build.sh
#
# Needs: cargo (stable), node + npm, git, pkg-config, the ALSA development files, patchelf, curl,
# file. It downloads appimagetool (https://github.com/AppImage/appimagetool, "continuous") unless
# $APPIMAGETOOL points at one; appimagetool in turn downloads the AppImage runtime
# (https://github.com/AppImage/type2-runtime, "continuous") unless $APPIMAGE_RUNTIME points at one.
# For a release, download both yourself and check them against the sha256 GitHub shows for the asset
# (gh api repos/AppImage/<repo>/releases/tags/continuous --jq '.assets[] | .name + " " + .digest').
# The update information (for AppImageUpdate) and the .zsync file need zsyncmake (package zsync):
# appimagetool -u fails without it, so without it the AppImage is built with no update information.
#
# Build it on an OLD distribution: the AppImage runs only where glibc is at least as new as on the
# build machine. Ubuntu 22.04 (glibc 2.35) covers Ubuntu 22.04+, Debian 12+, Fedora 36+, Arch.
# From any x86_64 Docker/Podman host (or an Apple silicon Mac with Rosetta, slower):
#   packaging/appimage/container.sh                       this checkout
#   packaging/appimage/container.sh uzumaki-0.1.0.tar.gz  a release's source tarball, with this
#                                                         checkout's packaging/appimage scripts
# (docker or podman, ubuntu:22.04; the result lands in target/appimage/, owned by you).
#
# <release> is <version> for build 1 and <version>-<build> after it (BUILD, as in release.sh).
# Out: target/appimage/
#   Uzumaki-<release>-x86_64.AppImage (+ .sha256)   the release file
#   Uzumaki-x86_64.AppImage (+ .sha256)             the same file under a stable name, for
#                                                    releases/latest/download/ links
#   Uzumaki-x86_64.AppImage.zsync                   for AppImageUpdate (the embedded update info)
#
# Layout inside (the same as the AUR package's /usr/lib/uzumaki, so bridge.rs finds the bridge
# beside the binary):
#   AppRun                                    starts usr/lib/uzumaki/uzumaki (or uzu, see AppRun)
#   io.github.hanami117.uzumaki.desktop, .png, .DirIcon
#   usr/lib/uzumaki/uzumaki, uzu              the binaries (RUNPATH $ORIGIN/lib)
#   usr/lib/uzumaki/lib/                      the few libraries we bundle (below)
#   usr/lib/uzumaki/bridge/                   the playback bridge with castLabs Electron
#   usr/share/applications, icons/hicolor, uzumaki/icons/<variant>, licenses/uzumaki
#
# Tool choice: appimagetool with a hand-picked library list, not linuxdeploy. linuxdeploy copies
# every library the binaries link (minus its exclude list) and is made for one binary; here most of
# the size is Electron, which brings its own libraries in dist/ and expects the desktop's GTK, NSS,
# etc. like every Electron AppImage. The Rust binary links only glibc, libgcc_s and ALSA (always
# present on a desktop, and ALSA must match the system's plugins). What is missing on some desktops
# is what winit loads with dlopen: libxkbcommon-x11.so.0 (X11 sessions; winit panics without it) and
# its dependency libxcb-xkb.so.1. Those two are bundled; the binary's RUNPATH points at them, so no
# LD_LIBRARY_PATH leaks into Electron or into the browser the app opens.
#
# The AppImage runtime (type2-runtime, static) mounts itself with FUSE (fusermount or fusermount3).
# Without FUSE: ./Uzumaki-x86_64.AppImage --appimage-extract-and-run (or set
# APPIMAGE_EXTRACT_AND_RUN=1).
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")/../.."
say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf 'appimage/build.sh: %s\n' "$*" >&2; exit 1; }

[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || die "runs on Linux x86_64 only (see the docker line at the top)"
for tool in cargo npm node git pkg-config patchelf curl file; do
    command -v "$tool" >/dev/null || die "missing $tool"
done

APP_ID=io.github.hanami117.uzumaki
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[[ -n $version ]] || die "no version in Cargo.toml's [workspace.package]"
# Sources from before the build number (v0.1.0) have no BUILD file: that is build 1.
build=1
[[ -f BUILD ]] && build=$(tr -d '[:space:]' <BUILD)
[[ $build =~ ^[1-9][0-9]*$ ]] || die "BUILD must hold a whole number above 0 (got '$build')"
if ((build == 1)); then release=$version; else release=$version-$build; fi
target=${CARGO_TARGET_DIR:-target}
out=$target/appimage
appdir=$target/appimage/Uzumaki.AppDir
# AppImageUpdate reads this from the file and fetches the .zsync from the latest GitHub release.
update_info="gh-releases-zsync|hanami117|uzumaki|latest|Uzumaki-x86_64.AppImage.zsync"

# --- Build --------------------------------------------------------------------------------------
say "Building Uzumaki $version (build $build, release profile)"
# No compile-time channel: the app finds out at run time that it is an AppImage (APPIMAGE/APPDIR,
# update.rs this_install). The AUR's uzumaki-bin installs this same binary under /usr/lib/uzumaki,
# where it must report a package install ("system"), so "appimage" can't be baked in.
unset UZUMAKI_CHANNEL
cargo build --release --locked -p uzumaki -p uzumaki-ctl

say "Fetching the playback bridge (castLabs Electron)"
if [[ ! -x bridge/node_modules/electron/dist/electron ]]; then
    (cd bridge && npm ci --no-audit --no-fund)
    # The download (GitHub, ~100 MB) sometimes fails on a flaky connection: try three times.
    for try in 1 2 3; do
        (cd bridge && node node_modules/electron/install.js) && break
        ((try < 3)) || die "the Electron download failed three times"
        say "Electron download failed, trying again in 10 s"
        sleep 10
    done
fi
[[ -x bridge/node_modules/electron/dist/electron ]] || die "the Electron download failed"

# --- AppDir -------------------------------------------------------------------------------------
say "Assembling $appdir"
rm -rf "$appdir"
lib=$appdir/usr/lib/uzumaki
install -Dm755 "$target/release/uzumaki" "$lib/uzumaki"
install -Dm755 "$target/release/uzu" "$lib/uzu"
strip --strip-unneeded "$lib/uzumaki" "$lib/uzu" 2>/dev/null || true

install -dm755 "$lib/bridge"
install -m644 bridge/main.js bridge/preload.js bridge/signin.js bridge/airplay.js bridge/package.json \
    bridge/package-lock.json bridge/.npmrc "$lib/bridge/"
cp -a bridge/node_modules "$lib/bridge/"
# Since 0.1.1 the app itself passes --no-sandbox to Electron inside an AppImage (bridge.rs,
# running_from_appimage; see AppRun below for why). The 0.1.0 source predates that, so when this
# builds it, Electron's own binary gets a wrapper that adds the flag. The wrapper exists only in the
# AppDir: nothing outside the AppImage is affected, and packaging/aur-bin puts the real binary back
# (electron.bin -> electron) so a package install keeps the sandbox.
if ! grep -q 'fn running_from_appimage' app/src/bridge.rs; then
    say "This source doesn't turn the sandbox off itself: wrapping Electron with --no-sandbox"
    dist=$lib/bridge/node_modules/electron/dist
    mv "$dist/electron" "$dist/electron.bin"
    cat >"$dist/electron" <<'WRAP'
#!/bin/sh
# Uzumaki AppImage (built from a source before 0.1.1): Chromium's setuid sandbox helper can't work
# inside an AppImage (see AppRun), so the bridge's Electron runs with --no-sandbox.
exec "$(dirname "$(readlink -f "$0")")/electron.bin" --no-sandbox "$@"
WRAP
    chmod 755 "$dist/electron"
fi

# Libraries winit dlopens that some desktops lack (see the top). Copied from this build machine.
install -dm755 "$lib/lib"
for so in libxkbcommon-x11.so.0 libxcb-xkb.so.1; do
    # Not a pipe: awk exits at the first match, and with pipefail ldconfig's SIGPIPE kills the script.
    path=$(awk -v n="$so" '$1 == n && /x86-64/ { print $NF; exit }' <(ldconfig -p))
    [[ -n $path ]] || die "$so not found on this machine (install libxkbcommon-x11 / libxkbcommon-x11-0)"
    install -m644 "$(readlink -f "$path")" "$lib/lib/$so"
done
patchelf --set-rpath '$ORIGIN' "$lib/lib/libxkbcommon-x11.so.0"
patchelf --set-rpath '$ORIGIN/lib' "$lib/uzumaki"

# Anything else the binaries link must be on every desktop.
unexpected=$(for bin in "$lib/uzumaki" "$lib/uzu"; do patchelf --print-needed "$bin"; done | sort -u |
    grep -v -E '^(libc|libm|libdl|libpthread|librt|libgcc_s|libasound|ld-linux-x86-64)\.so' || true)
[[ -z $unexpected ]] || die "the binaries link libraries that aren't bundled or guaranteed: $unexpected"

install -Dm644 "assets/$APP_ID.desktop" "$appdir/usr/share/applications/$APP_ID.desktop"
cp "assets/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
for icon in assets/icons/hicolor/*/apps/"$APP_ID".png; do
    size=$(basename "$(dirname "$(dirname "$icon")")")
    install -Dm644 "$icon" "$appdir/usr/share/icons/hicolor/$size/apps/$APP_ID.png"
done
cp "assets/icons/hicolor/256x256/apps/$APP_ID.png" "$appdir/$APP_ID.png"
ln -s "$APP_ID.png" "$appdir/.DirIcon"
for variant in assets/icons/variants/*/; do
    install -dm755 "$appdir/usr/share/uzumaki/icons/$(basename "$variant")"
    install -m644 "$variant"*.png "$appdir/usr/share/uzumaki/icons/$(basename "$variant")/"
done

lic=$appdir/usr/share/licenses/uzumaki
install -Dm644 LICENSE "$lic/LICENSE"
install -Dm644 THIRD_PARTY_NOTICES.md "$lic/THIRD_PARTY_NOTICES.md"
# Electron's and Chromium's notices are in usr/lib/uzumaki/bridge/node_modules/electron/dist/.
for f in "$lib"/lib/*.so.*; do
    printf '%s: copied from the build machine (%s); MIT licence (libxkbcommon, libxcb)\n' \
        "$(basename "$f")" "$(. /etc/os-release && echo "$PRETTY_NAME")" >>"$lic/BUNDLED_LIBRARIES"
done

cat >"$appdir/AppRun" <<'APPRUN'
#!/bin/sh
# Uzumaki's AppImage entry point.
#   ./Uzumaki-x86_64.AppImage [options]      the app (--version, --snap ..., as the plain binary)
#   ./Uzumaki-x86_64.AppImage uzu [args]     the uzu command line
#   a link named uzu to the AppImage          uzu as well (the runtime sets ARGV0 to the link)
# Sandbox: the app adds --no-sandbox to the bridge's Electron when it runs from an AppImage
# (APPIMAGE and APPDIR set, binary under APPDIR; app/src/bridge.rs, running_from_appimage).
# Chromium's chrome-sandbox helper needs to be root-owned and setuid, which a file inside an
# AppImage can never be (the image is mounted nosuid by the user), and on Ubuntu 24.04+ AppArmor
# blocks the user-namespace sandbox for unprivileged programs, so Electron would refuse to start.
HERE=$(dirname "$(readlink -f "$0")")
LIB="$HERE/usr/lib/uzumaki"
case "${ARGV0##*/}" in
    uzu|uzu.*) exec "$LIB/uzu" "$@" ;;
esac
if [ "${1:-}" = uzu ]; then
    shift
    exec "$LIB/uzu" "$@"
fi
exec "$LIB/uzumaki" "$@"
APPRUN
chmod 755 "$appdir/AppRun"

# --- AppImage -----------------------------------------------------------------------------------
tool=${APPIMAGETOOL:-}
if [[ -z $tool ]]; then
    tool=$out/appimagetool-x86_64.AppImage
    if [[ ! -x $tool ]]; then
        say "Downloading appimagetool"
        curl -fL --retry 3 -o "$tool" \
            https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
        chmod +x "$tool"
    fi
fi
say "appimagetool: $(sha256sum "$tool" | cut -d' ' -f1)"

name=Uzumaki-$release-x86_64.AppImage
stable=Uzumaki-x86_64.AppImage
rm -f "$out/$name" "$out/$stable" "$out"/*.sha256 "$out"/*.zsync
opts=(--no-appstream)
[[ -n ${APPIMAGE_RUNTIME:-} ]] && opts+=(--runtime-file "$(readlink -f "$APPIMAGE_RUNTIME")")
if command -v zsyncmake >/dev/null; then
    opts+=(-u "$update_info")
else
    say "Warning: no zsyncmake (package zsync): building without update information or .zsync"
fi
# Extract-and-run: works without FUSE (containers, CI).
# Absolute paths only: run this way, appimagetool resolves relative ones in its own temporary
# folder (the AppDir isn't found, or the AppImage lands there and is deleted with it). It writes
# the .zsync into the current folder, hence the cd.
tool=$(readlink -f "$tool")
appdir=$(readlink -f "$appdir")
(cd "$out" && ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" "${opts[@]}" "$appdir" "$PWD/$name")
# The .zsync is named after the output; AppImageUpdate looks for the stable name.
if [[ -f $out/$name.zsync ]]; then
    mv "$out/$name.zsync" "$out/$stable.zsync"
else
    say "No .zsync written: AppImageUpdate can't update this file (fine for a test build, not a release)"
fi
cp "$out/$name" "$out/$stable"
for f in "$name" "$stable"; do
    (cd "$out" && sha256sum "$f" >"$f.sha256")
done

say "Done: $out/$name ($(du -h "$out/$name" | cut -f1))"
cat "$out/$name.sha256"
