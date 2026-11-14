#!/usr/bin/env bash
# Builds cliairplay (Music Assistant's AirPlay sender, GPL-3.0, github.com/music-assistant/airplay-cli)
# at a pinned tag into target/cliairplay/cliairplay, where the AirPlay spike finds it.
#
#   scripts/airplay/build-cliairplay.sh
#
# macOS (arm64 or x86_64): needs the Xcode command line tools (clang, make, git).
# Linux (x86_64 or aarch64): needs gcc, g++, make, git (Arch: `pacman -S base-devel git`;
#   Debian/Ubuntu: `apt install build-essential git`). The same `make STATIC=1`; the binary is
#   bin/cliairplay-linux-<arch>. AirPlay 2 PTP timing binds UDP 319/320, which Linux allows only
#   with privilege: `sudo setcap cap_net_bind_service=+ep target/cliairplay/cliairplay`, or the
#   helper falls back to NTP timing on its own. Open UDP 319/320 and the helper's ports in ufw/firewalld.
#
# The build links libraop's prebuilt static codec / mDNS archives (from its git submodule), as
# upstream's CI does. Its prebuilt OpenSSL is already 3.x (3.5.4 at v0.5.10). On macOS, when Homebrew's
# openssl@3 is installed, the helper links that one statically instead (newer patch level, no runtime
# dependency on /opt/homebrew: only libc++ and libSystem remain). Otherwise the prebuilt one is used.
set -euo pipefail

TAG=v0.5.10
root=$(cd "$(dirname "$0")/../.." && pwd)
out=$root/target/cliairplay
src=$out/src

mkdir -p "$out"
if [ ! -d "$src/.git" ]; then
  git clone --quiet https://github.com/music-assistant/airplay-cli.git "$src"
fi
git -C "$src" fetch --quiet --tags
git -C "$src" checkout --quiet "$TAG"
git -C "$src" submodule update --init --recursive --quiet

case "$(uname -s)" in
  Darwin) host=macos ;;
  Linux) host=linux ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
arch=$(uname -m)
[ "$host" = macos ] && [ "$arch" != arm64 ] && arch=x86_64
jobs=$( (sysctl -n hw.ncpu 2>/dev/null || nproc) | head -1)

extra=()
if [ "$host" = macos ] && command -v brew >/dev/null 2>&1 && ossl=$(brew --prefix openssl@3 2>/dev/null) && [ -f "$ossl/lib/libcrypto.a" ]; then
  # cliairplay's Makefile wants one libopenssl.a and an include/ next to it.
  mkdir -p "$out/ossl3"
  ln -sfn "$ossl/include" "$out/ossl3/include"
  libtool -static -o "$out/ossl3/libopenssl.a" "$ossl/lib/libssl.a" "$ossl/lib/libcrypto.a" 2>/dev/null
  extra=(OPENSSL="$out/ossl3")
  make -C "$src" clean >/dev/null 2>&1 || true
fi

make -C "$src" STATIC=1 ${extra[@]+"${extra[@]}"} -j"$jobs" > "$out/build.log" 2>&1 || { tail -30 "$out/build.log"; echo "build failed (full log: $out/build.log)" >&2; exit 1; }
cp "$src/bin/cliairplay-$host-$arch" "$out/cliairplay"
"$out/cliairplay" --check
echo "built $out/cliairplay ($TAG)"
