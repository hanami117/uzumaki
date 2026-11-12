#!/usr/bin/env bash
# Builds Uzumaki from this checkout and installs it for the current user.
#
#   ./install.sh               build and install (or update, after a `git pull`)
#   ./install.sh --uninstall   remove it again (keeps your login and settings)
#
# Installs into $PREFIX (default ~/.local):
#   bin/uzumaki                                    the app (release build)
#   bin/uzu                                        the mpc-style command line
#   share/uzumaki/bridge/                          the castLabs Electron playback bridge
#   share/applications/io.github.hanami117.uzumaki.desktop
#   share/icons/hicolor/<size>/apps/io.github.hanami117.uzumaki.png   (the default, dark icon)
#   share/uzumaki/icons/{dark,light,coral}/<size>.png                   (the app swaps hicolor to
#                                                                        the one chosen in Settings)
# Your login, settings and caches live in ~/.config/uzumaki and ~/.cache/uzumaki.
set -euo pipefail

APP_ID=io.github.hanami117.uzumaki
PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/uzumaki"
UZU="$PREFIX/bin/uzu"
SHARE="$PREFIX/share/uzumaki"
DESKTOP="$PREFIX/share/applications/$APP_ID.desktop"
ICONS="$PREFIX/share/icons/hicolor"
EXEC_LINE="Exec=$BIN"
[[ $BIN == *" "* ]] && EXEC_LINE="Exec=\"$BIN\""

cd "$(dirname "$(readlink -f "$0")")"

say() { printf '\033[1;38;2;255;122;138m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

refresh_menus() {
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
    # Only refresh an icon cache that exists: a new one goes stale when other apps add icons.
    [[ -f $ICONS/icon-theme.cache ]] && gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true
}

# The installed copy is in use if its Electron (or the app) is running.
running() { pgrep -f "^($BIN|$SHARE/bridge/)" >/dev/null 2>&1; }

uninstall() {
    running && die "Uzumaki is running. Quit it first (Ctrl+Q), then run this again."
    say "Removing Uzumaki from $PREFIX"
    rm -f "$BIN" "$UZU"
    # The program only: playlist pictures (share/uzumaki/playlist-art) stay, as settings do.
    rm -rf "$SHARE/bridge" "$SHARE/icons"
    rmdir "$SHARE" 2>/dev/null || true
    rm -f "$ICONS"/*/apps/"$APP_ID".png
    # Only remove the menu entry if it is ours (it may point at a build in a source checkout).
    if [[ -f $DESKTOP ]] && grep -qxF "$EXEC_LINE" "$DESKTOP"; then
        rm -f "$DESKTOP"
    elif [[ -f $DESKTOP ]]; then
        say "Kept $DESKTOP: it starts $(grep -m1 '^Exec=' "$DESKTOP" | cut -d= -f2-), not this install"
    fi
    refresh_menus
    say "Done. Your login, settings and caches are still in ~/.config/uzumaki and ~/.cache/uzumaki;"
    say "delete those two folders to forget everything."
}

case "${1:-}" in
    --uninstall) uninstall; exit ;;
    -h|--help) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit ;;
    "") ;;
    *) die "unknown option '$1' (try --help)" ;;
esac

# --- Requirements -------------------------------------------------------------------------------
missing=()
for tool in cargo node npm git pkg-config; do
    command -v "$tool" >/dev/null || missing+=("$tool")
done
if command -v pkg-config >/dev/null && ! pkg-config --exists alsa; then
    missing+=("alsa (development files)")
fi
if ((${#missing[@]})); then
    printf 'Missing: %s\n' "${missing[*]}" >&2
    if command -v pacman >/dev/null; then
        echo "On Arch: sudo pacman -S --needed rust nodejs npm git pkgconf alsa-lib" >&2
    fi
    die "install the tools above, then run this again"
fi
if ! grep -q 'libvulkan\.so\.1' <(ldconfig -p 2>/dev/null); then  # not a pipe: grep -q + pipefail = SIGPIPE
    say "Warning: no Vulkan loader found. Uzumaki draws with Vulkan; install vulkan-icd-loader and"
    say "your GPU's driver (vulkan-radeon, vulkan-intel or nvidia-utils)."
fi
running && die "Uzumaki is running from $PREFIX. Quit it first (Ctrl+Q), then run this again."

# --- Build --------------------------------------------------------------------------------------
say "Building Uzumaki (release; the first build takes several minutes)"
cargo build --release --locked -p uzumaki -p uzumaki-ctl
target="${CARGO_TARGET_DIR:-target}/release/uzumaki"
[[ -x $target ]] || die "build finished but $target is missing"
[[ -x ${target%uzumaki}uzu ]] || die "build finished but ${target%uzumaki}uzu is missing"

say "Fetching the playback bridge (castLabs Electron with Widevine)"
if [[ ! -x bridge/node_modules/electron/dist/electron ]]; then
    # bridge/.npmrc allows the castLabs git dependency, which npm 12 blocks by default.
    (cd bridge && npm ci --no-audit --no-fund)
    # Downloads the Electron binary (npm may skip the package's own install script).
    (cd bridge && node node_modules/electron/install.js)
fi
[[ -x bridge/node_modules/electron/dist/electron ]] || die "the Electron download failed (see above)"

# --- Install ------------------------------------------------------------------------------------
say "Installing into $PREFIX"
install -Dm755 "$target" "$BIN"
install -Dm755 "${target%uzumaki}uzu" "$UZU"

stage="$SHARE/bridge.new"
rm -rf "$stage"
mkdir -p "$stage"
cp bridge/main.js bridge/preload.js bridge/signin.js bridge/package.json bridge/package-lock.json bridge/.npmrc "$stage/"
cp -a --reflink=auto bridge/node_modules/. "$stage/node_modules/"
rm -rf "$SHARE/bridge"
mv "$stage" "$SHARE/bridge"

for icon in assets/icons/hicolor/*/apps/"$APP_ID".png; do
    size=$(basename "$(dirname "$(dirname "$icon")")")
    install -Dm644 "$icon" "$ICONS/$size/apps/$APP_ID.png"
done

# The three app icons the user can pick in Settings → Appearance. hicolor/ (above) holds the
# default; the app copies the chosen set over it at start-up and when the choice changes.
rm -rf "$SHARE/icons"
for variant in assets/icons/variants/*/; do
    name=$(basename "$variant")
    mkdir -p "$SHARE/icons/$name"
    install -m644 "$variant"*.png "$SHARE/icons/$name/"
done

if [[ -f $DESKTOP ]] && ! grep -qxF "$EXEC_LINE" "$DESKTOP"; then
    say "Replacing the existing menu entry (it started $(grep -m1 '^Exec=' "$DESKTOP" | cut -d= -f2-))"
fi
mkdir -p "$(dirname "$DESKTOP")"
sed "s|^Exec=.*|$EXEC_LINE|" "assets/$APP_ID.desktop" > "$DESKTOP"
chmod 644 "$DESKTOP"
refresh_menus

say "Installed. Start Uzumaki from your app menu, or run: $BIN"
say "From a terminal: uzu (status), uzu playlists, uzu play <name>, uzu help"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) say "Note: $PREFIX/bin is not on your PATH, so plain \`uzumaki\` won't work in a terminal." ;;
esac
say "First start: sign in to Apple Music from the banner (the Widevine module downloads by itself)."
say "To update later: git pull && ./install.sh"
