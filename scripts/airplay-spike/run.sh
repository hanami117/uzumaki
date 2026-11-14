#!/usr/bin/env bash
# Runs Uzumaki with the AirPlay spike on: the Apple Music audio goes to ONE AirPlay speaker
# (default "Bedroom") instead of this computer's output. Quit the normal Uzumaki first.
#
#   scripts/airplay-spike/run.sh             the test app (target/macos.noindex/Uzumaki.app), else target/release or target/debug
#   scripts/airplay-spike/run.sh --dry-run   no speaker: a stand-in helper prints the level it receives
#   scripts/airplay-spike/run.sh <binary>    another uzumaki binary
#
# Settings (environment):
#   UZUMAKI_AIRPLAY_TARGET   speaker name as in Apple's AirPlay picker (default Bedroom)
#   UZUMAKI_AIRPLAY_VOLUME   speaker volume at start, 0-100 (default 30)
#   UZUMAKI_AIRPLAY_DEBUG    cliairplay --debug level (default 3; 5 for more)
#   UZUMAKI_AIRPLAY_EXTRA_ARGS  more cliairplay arguments, e.g. "--timing ntp"
#   UZUMAKI_AIRPLAY_LOCAL=1  keep playing on this computer too while casting (A/B checks)
#
# Everything the spike logs (app, helper and the bridge's capture lines) ends up in one file, printed
# at the end. It holds levels and status lines only, never audio.
set -uo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
dry=0
bin=""
for a in "$@"; do
  case "$a" in
    --dry-run) dry=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) bin=$a ;;
  esac
done
if [ -z "$bin" ]; then
  for c in "$root/target/macos.noindex/Uzumaki.app/Contents/MacOS/uzumaki" "$root/target/release/uzumaki" "$root/target/debug/uzumaki"; do
    [ -x "$c" ] && { bin=$c; break; }
  done
fi
[ -d "$bin" ] && bin="$bin/Contents/MacOS/uzumaki" # an .app was given
[ -x "$bin" ] || { echo "no uzumaki binary found (build one, or pass its path)" >&2; exit 1; }

if [ "$dry" = 1 ]; then
  export UZUMAKI_CLIAIRPLAY="$root/scripts/airplay-spike/fake-cliairplay.py"
  export UZUMAKI_AIRPLAY_HOST=127.0.0.1
else
  export UZUMAKI_CLIAIRPLAY="${UZUMAKI_CLIAIRPLAY:-$root/target/cliairplay/cliairplay}"
  [ -x "$UZUMAKI_CLIAIRPLAY" ] || { echo "cliairplay missing: run scripts/airplay-spike/build-cliairplay.sh first" >&2; exit 1; }
fi
export UZUMAKI_AIRPLAY_SPIKE=1
export UZUMAKI_AIRPLAY_TARGET="${UZUMAKI_AIRPLAY_TARGET:-Bedroom}"

if [ "$(uname -s)" = Darwin ]; then logdir="$HOME/Library/Logs/Uzumaki"; else logdir="${XDG_STATE_HOME:-$HOME/.local/state}/uzumaki"; fi
mkdir -p "$logdir"
log="$logdir/airplay-spike.log"
bridge_log="${TMPDIR:-/tmp}"
bridge_log="${bridge_log%/}/uzumaki-bridge.log"

{
  echo "=== airplay spike $(date '+%Y-%m-%d %H:%M:%S'), $( [ $dry = 1 ] && echo DRY RUN || echo "target \"$UZUMAKI_AIRPLAY_TARGET\"" )"
  echo "=== app: $bin"
  echo "=== helper: $UZUMAKI_CLIAIRPLAY"
  echo "=== $(cd "$root" && git log --oneline -1 2>/dev/null)"
} | tee "$log"
echo "Starting Uzumaki. Play a song; it should come out of \"$UZUMAKI_AIRPLAY_TARGET\" after a second or two."
echo "Quit Uzumaki (Cmd+Q / Ctrl+Q) to end the test."

# stderr carries the app's and the helper's lines; the window keeps running in the foreground.
"$bin" 2>&1 | tee -a "$log"

{
  echo "=== bridge capture lines ($bridge_log)"
  grep -E '\] airplay|song start|playback error|renderer gone|widevine' "$bridge_log" 2>/dev/null | grep -v 'ctrl-airplay' | tail -400
} >> "$log"
echo
echo "Log for Claude: $log"
