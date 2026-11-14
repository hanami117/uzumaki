#!/bin/sh
# Usage: scripts/linux-check/run.sh [SOURCE_DIR] [STEP...]
#   SOURCE_DIR  a checkout of the repo (default: the one this script is in)
#   STEP        build test clippy node snap  (default: all)
# The source is mounted read-only and copied (no .git, target, node_modules) into the container. Cargo's
# registry and the target dir live in docker volumes (uzu-linux-cargo, uzu-linux-target), never on the Mac.
# Logs: /tmp/linux-check/<label>/<step>.log, label = the folder's name.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
src=$here/../..
if [ $# -gt 0 ] && [ -d "$1" ]; then src=$1; shift; fi
src=$(cd "$src" && pwd)
steps=${*:-build test clippy node snap}
label=$(basename "$src")
out=/tmp/linux-check/$label
mkdir -p "$out"
docker build -q -t uzu-linux-check "$here" >/dev/null
docker run --rm -v "$src":/src:ro -v "$out":/out \
  -v uzu-linux-cargo:/cargo -v uzu-linux-target:/target \
  -e STEPS="$steps" uzu-linux-check sh -c '
set -u
mkdir -p /work "$HOME" && rsync -a --delete --exclude .git --exclude target --exclude node_modules /src/ /work/
cd /work
export UZUMAKI_PROFILE_DIR=/tmp/scratch-profile
for s in $STEPS; do
  case $s in
    build)  cmd="cargo build --workspace" ;;
    test)   cmd="cargo test --workspace" ;;
    clippy) cmd="cargo clippy --workspace --all-targets" ;;
    node)   cmd="node --test bridge/*.test.js" ;;
    snap)   cmd="sh -c \"for sc in album-motion up-next-autoplay; do UZUMAKI_MOTION=ffmpeg cargo run -q -p uzumaki -- --snap /out/\\\$sc.png --scene \\\$sc --size 1360x860 --scale 1 || exit 1; done\"" ;;
    *) echo "unknown step $s"; continue ;;
  esac
  echo "== $s: $cmd"
  eval "$cmd" >/out/$s.log 2>&1; echo "== $s exit $?"
  [ $s = clippy ] && echo "clippy warnings: $(grep -c "^warning" /out/clippy.log)"
  [ $s = test ] && grep -E "^test result|FAILED|failed" /out/test.log | sort | uniq -c | head -20
done'
