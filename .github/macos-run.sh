#!/bin/bash
# Run one Loft dev smoke on the macOS runner and stop when it reports.
#
#   .github/macos-run.sh <success-marker> <timeout-seconds> <screenshot-tag>
#
# Expects LOFT_DEV_EVAL (and the other smoke variables) to be set by
# scripts/smoke.sh, which this script execs through. The success marker is
# a "[dev] ..." line. "[dev] FAILED:" fails the run.
#
# `npm run tauri dev` is started under `script` so Rust's stderr stays
# line-buffered and the marker shows up while the app is still running.
set -uo pipefail
# Background jobs get their own process group so stopping one smoke cannot
# kill this script.
set -m

marker=${1:?success marker}
timeout_s=${2:?timeout seconds}
shot_tag=${3:-}

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
log=/tmp/loft-dev-run.log
rm -f "$log"

# shellcheck disable=SC2094
script -q -F "$log" scripts/smoke.sh "${LOFT_SMOKE_SCRIPT:-smoke-ui.js}" &
pid=$!
sleep 0.5
pgid=$(ps -o pgid= -p "$pid" 2>/dev/null | tr -d ' ' || true)
my_pgid=$(ps -o pgid= -p $$ | tr -d ' ')

shot() {
  [ -n "$shot_tag" ] || return 0
  local name=$1
  local dest="/tmp/loft-${shot_tag}-${name}.png"
  [ -f "$dest" ] && return 0
  screencapture -x "$dest" || true
}

deadline=$((SECONDS + timeout_s))
while kill -0 "$pid" 2>/dev/null; do
  if [ -f "$log" ]; then
    if grep -a -q "probe expanded OK" "$log"; then
      shot expanded
    fi
    if grep -a -q "probe right OK" "$log"; then
      shot right
    fi
    if grep -a -q -e "$marker" -e "FAILED:" "$log"; then
      # The script asks the app to quit. Give it a moment, then kill the group
      # so a leftover vite server cannot hold the port for the next run.
      for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
      done
      break
    fi
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "timed out after ${timeout_s}s waiting for ${marker}"
    break
  fi
  sleep 1
done

if [ -n "${pgid:-}" ] && [ "$pgid" != "$my_pgid" ]; then
  kill -TERM -- "-$pgid" 2>/dev/null || true
  sleep 1
  kill -KILL -- "-$pgid" 2>/dev/null || true
else
  kill -TERM "$pid" 2>/dev/null || true
fi
wait "$pid" 2>/dev/null || true
pkill -f "src-tauri/target/debug/loft" 2>/dev/null || true
pkill -f "vite.js" 2>/dev/null || true
pkill -f "node_modules/.bin/vite" 2>/dev/null || true
sleep 2

echo "----- [dev] lines -----"
grep -a "\[dev\]" "$log" || true
echo "----- shortcut / tray -----"
grep -a -E "could not (create the tray icon|register global shortcut)" "$log" || echo "(no tray or shortcut error logged)"

if grep -a -q "FAILED:" "$log"; then
  echo "smoke reported FAILED"
  exit 1
fi
if ! grep -a -q "$marker" "$log"; then
  echo "smoke ended without ${marker}"
  exit 1
fi
exit 0
