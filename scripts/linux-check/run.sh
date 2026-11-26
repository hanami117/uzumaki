#!/bin/sh
# Usage: scripts/linux-check/run.sh [SOURCE_DIR] [STEP...]
#   SOURCE_DIR  a checkout of the repo (default: the one this script is in)
#   STEP        build test clippy node snap bus  (default: all)
#               bus: the D-Bus tests (tray) on a private session bus of their own (dbus-run-session).
# The source is mounted read-only and copied (no .git, target, node_modules) into the container. Cargo's
# registry and the target dir live in docker volumes (uzu-linux-cargo, uzu-linux-target; another prefix with
# LINUX_CHECK_VOLUMES=<prefix>, e.g. for a parallel session), never on the Mac.
# Logs: /tmp/linux-check/<label>/<step>.log, label = the folder's name.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
src=$here/../..
if [ $# -gt 0 ] && [ -d "$1" ]; then src=$1; shift; fi
src=$(cd "$src" && pwd)
steps=${*:-build test clippy node snap bus}
vol=${LINUX_CHECK_VOLUMES:-uzu-linux}
label=$(basename "$src")
out=/tmp/linux-check/$label
mkdir -p "$out"
docker build -q -t uzu-linux-check "$here" >/dev/null
docker run --rm -v "$src":/src:ro -v "$out":/out \
  -v "$vol-cargo":/cargo -v "$vol-target":/target \
  -e STEPS="$steps" -e WGPU_BACKEND="${WGPU_BACKEND:-gl}" uzu-linux-check sh -c '
set -u
fail=0
mkdir -p /work "$HOME" && rsync -a --delete --exclude .git --exclude target --exclude node_modules /src/ /work/
cd /work
export UZUMAKI_PROFILE_DIR=/tmp/scratch-profile
for s in $STEPS; do
  case $s in
    build)  cmd="cargo build --workspace" ;;
    test)   cmd="cargo test --workspace" ;;
    clippy) cmd="cargo clippy --workspace --all-targets" ;;
    node)   cmd="node --test bridge/*.test.js" ;;
    snap)   cmd="sh -c \"for sc in album-motion up-next-autoplay; do WGPU_BACKEND=${WGPU_BACKEND:-gl} UZUMAKI_MOTION=ffmpeg cargo run -q -p uzumaki -- --snap /out/\\\$sc.png --scene \\\$sc --size 1360x860 --scale 1 || exit 1; done\"" ;;
    bus)    cmd="dbus-run-session -- env UZUMAKI_PRIVATE_BUS=1 cargo test -p uzumaki tray:: -- --nocapture" ;;
    *) echo "unknown step $s"; continue ;;
  esac
  echo "== $s: $cmd"
  eval "$cmd" >/out/$s.log 2>&1; rc=$?; echo "== $s exit $rc"
  [ $rc -eq 0 ] || fail=1
  if [ $s = clippy ]; then echo "clippy warnings: $(grep -c "^warning" /out/clippy.log)"; fi
  if [ $s = test ] || [ $s = bus ]; then grep -E "^test result|FAILED|failed" /out/$s.log | sort | uniq -c | head -20; fi
done
exit ${fail:-0}'
