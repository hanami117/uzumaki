#!/usr/bin/env bash
# Installs what packaging/appimage/build.sh needs on a fresh Ubuntu 22.04 (as root; container.sh
# runs it): the build tools, the ALSA headers, the two libraries the AppImage bundles, zsync
# (zsyncmake: update information and the .zsync file), Rust (rustup, stable) and Node.js 22 (the
# official tarball; Ubuntu's is too old). libxkbcommon-dev: a crate asks the linker for
# -lxkbcommon; with --as-needed the binary doesn't end up depending on it.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -yq --no-install-recommends ca-certificates curl git xz-utils file patchelf \
    build-essential pkg-config libasound2-dev libxkbcommon-dev libxkbcommon-x11-0 libxcb-xkb1 zsync
if ! command -v cargo >/dev/null; then
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
if ! command -v node >/dev/null; then
    node=v22.21.1
    curl -fsSL "https://nodejs.org/dist/$node/node-$node-linux-x64.tar.xz" | tar -xJ -C /usr/local --strip-components=1
fi
