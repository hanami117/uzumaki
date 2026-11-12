#!/usr/bin/env bash
# Builds the release AppImage inside an ubuntu:22.04 container (glibc 2.35), so it runs on older
# distributions too (see the top of build.sh). Uses podman or docker, whichever is there
# ($CONTAINER picks one).
#
#   packaging/appimage/container.sh                        this checkout's HEAD (git archive)
#   packaging/appimage/container.sh uzumaki-0.1.0.tar.gz   a release's source tarball, built with
#                                                          this checkout's packaging/appimage scripts
#
# $APPIMAGETOOL and $APPIMAGE_RUNTIME (files checked against GitHub's digests, see build.sh) are
# copied in when set. The result goes to $OUT (default target/appimage/ in this checkout), owned
# by you. The container only sees a copy of the source, read-only, and is removed afterwards.
set -euo pipefail

here=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
repo=$(cd "$here/../.." && pwd)
die() { printf 'appimage/container.sh: %s\n' "$*" >&2; exit 1; }

engine=${CONTAINER:-}
if [[ -z $engine ]]; then
    engine=$(command -v podman || command -v docker || true)
fi
[[ -n $engine ]] || die "needs podman or docker"
out=${OUT:-$repo/target/appimage}
mkdir -p "$out"
out=$(cd "$out" && pwd)

work=$(mktemp -d "${TMPDIR:-/tmp}/uzumaki-appimage.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir "$work/src" "$work/tools"
if [[ $# -ge 1 ]]; then
    [[ -f $1 ]] || die "no such tarball: $1"
    tar -xzf "$1" -C "$work/src" --strip-components=1
else
    [[ -z $(git -C "$repo" status --porcelain) ]] || echo "note: uncommitted changes are not included (git archive HEAD)" >&2
    git -C "$repo" archive HEAD | tar -x -C "$work/src"
fi
install -d "$work/src/packaging/appimage"   # older tarballs (v0.1.0) have none
install -m755 "$here/build.sh" "$here/deps-ubuntu.sh" "$work/src/packaging/appimage/"

env=()
if [[ -n ${APPIMAGETOOL:-} ]]; then
    install -m755 "$APPIMAGETOOL" "$work/tools/appimagetool"
    env+=(-e APPIMAGETOOL=/tools/appimagetool)
fi
if [[ -n ${APPIMAGE_RUNTIME:-} ]]; then
    install -m644 "$APPIMAGE_RUNTIME" "$work/tools/runtime"
    env+=(-e APPIMAGE_RUNTIME=/tools/runtime)
fi
# Rootless podman maps the container's root to you; docker's root is the real root.
owner=
[[ $(basename "$engine") == docker ]] && owner="$(id -u):$(id -g)"

"$engine" run --rm --platform linux/amd64 \
    -v "$work/src:/src:ro" -v "$work/tools:/tools:ro" -v "$out:/out" "${env[@]}" \
    -e OWNER="$owner" ubuntu:22.04 bash -euo pipefail -c '
        /src/packaging/appimage/deps-ubuntu.sh
        . ~/.cargo/env
        cp -a /src /build
        cd /build
        packaging/appimage/build.sh
        cp target/appimage/Uzumaki-*.AppImage* /out/
        if [ -n "$OWNER" ]; then chown "$OWNER" /out/Uzumaki-*; fi
    '
ls -l "$out"/Uzumaki-*
