#!/usr/bin/env bash
# Builds cliairplay (Music Assistant's AirPlay sender, GPL-3.0, github.com/music-assistant/airplay-cli)
# at a pinned tag into target/cliairplay/cliairplay. The macOS bundle, install.sh, the AppImage and the
# AUR packages all ship this file; the app finds it beside its binary (airplay/src/helper.rs, find).
#
#   scripts/airplay/build-cliairplay.sh            clone (or update) target/cliairplay/src and build
#   scripts/airplay/build-cliairplay.sh --reuse    keep a target/cliairplay/cliairplay that already
#                                                  reports the pinned tag (no network, no build)
#   CLIAIRPLAY_SRC=<dir> scripts/airplay/...       build an existing checkout of the tag with its
#                                                  submodules (the AUR package: makepkg fetches them)
#
# macOS (arm64 or x86_64): needs the Xcode command line tools (clang, make, git).
# Linux (x86_64 or aarch64): needs gcc, g++, make, git (Arch: `pacman -S base-devel git`;
#   Debian/Ubuntu: `apt install build-essential git`). The same `make STATIC=1`; the binary is
#   bin/cliairplay-linux-<arch>. libstdc++ and libatomic are linked statically, so the helper needs only
#   glibc (libatomic is not on every desktop; the AppImage runs where it was not built). AirPlay 2 PTP
#   timing binds UDP 319/320, which Linux allows only with privilege:
#   `sudo setcap cap_net_bind_service=+ep <path>/cliairplay`, or the helper falls back to NTP timing
#   on its own (fine for one speaker and for multi-room).
#
# The build links libraop's prebuilt static codec / mDNS archives (from its git submodule), as
# upstream's CI does. Its prebuilt OpenSSL is already 3.x (3.5.4 at v0.5.10). On macOS, when Homebrew's
# openssl@3 is installed, the helper links that one statically instead (newer patch level, no runtime
# dependency on /opt/homebrew: only libc++ and libSystem remain). Otherwise the prebuilt one is used.
#
# The tag is pinned here AND in packaging/aur/PKGBUILD (_cliairplay); a test keeps them equal
# (airplay/tests/packaging.rs).
set -euo pipefail

TAG=v0.5.10
root=$(cd "$(dirname "$0")/../.." && pwd)
out=$root/target/cliairplay
src=${CLIAIRPLAY_SRC:-$out/src}

reports_tag() { [[ -x $1 ]] && "$1" --check 2>/dev/null | grep -qF "cliairplay v${TAG#v} "; }

if [[ ${1:-} == --reuse ]] && reports_tag "$out/cliairplay"; then
  echo "reusing $out/cliairplay ($TAG)"
  exit 0
fi

mkdir -p "$out"
if [[ -n ${CLIAIRPLAY_SRC:-} ]]; then
  [[ -f $src/Makefile && -d $src/libraop/src ]] || { echo "CLIAIRPLAY_SRC=$src: not a cliairplay checkout with its submodules" >&2; exit 1; }
else
  if [ ! -d "$src/.git" ]; then
    git clone --quiet https://github.com/music-assistant/airplay-cli.git "$src"
  fi
  git -C "$src" fetch --quiet --tags
  git -C "$src" checkout --quiet "$TAG"
  git -C "$src" submodule update --init --recursive --quiet
fi

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
if [ "$host" = linux ]; then
  # The Makefile links -lstdc++ -latomic. A folder holding only their static archives, searched
  # first (ld applies every -L to every -l), makes those two static without patching the Makefile.
  static=$out/static-libs
  rm -rf "$static"
  mkdir -p "$static"
  for a in libstdc++.a libatomic.a; do
    p=$(${CC:-gcc} -print-file-name="$a")
    [[ $p == /* && -f $p ]] || { echo "no $a for ${CC:-gcc} (Debian/Ubuntu: libstdc++-<n>-dev; it comes with g++)" >&2; exit 1; }
    ln -s "$p" "$static/$a"
  done
  extra+=(EXTRA_LDFLAGS="-L$static -static-libgcc")
  # A build from before (dynamically linked) would otherwise be kept by make.
  rm -f "$src/bin/cliairplay-$host-$arch"
fi

make -C "$src" STATIC=1 ${extra[@]+"${extra[@]}"} -j"$jobs" > "$out/build.log" 2>&1 || { tail -30 "$out/build.log"; echo "build failed (full log: $out/build.log)" >&2; exit 1; }
cp "$src/bin/cliairplay-$host-$arch" "$out/cliairplay.new"
mv -f "$out/cliairplay.new" "$out/cliairplay"
reports_tag "$out/cliairplay" || { "$out/cliairplay" --check >&2 || true; echo "the helper does not report $TAG" >&2; exit 1; }
if [ "$host" = linux ]; then
  # Only glibc's libraries (and the loader) may remain: anything else is not on every desktop.
  odd=$(objdump -p "$out/cliairplay" | awk '$1 == "NEEDED" { print $2 }' |
    grep -v -E '^(libc|libm|libdl|libpthread|librt|libgcc_s|ld-linux-[a-z0-9_-]+)\.so' || true)
  [[ -z $odd ]] || { echo "cliairplay links libraries that are not on every desktop: $odd" >&2; exit 1; }
fi
echo "built $out/cliairplay ($TAG)"
