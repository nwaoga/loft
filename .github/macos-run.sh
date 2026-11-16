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
rm -f "$log" /tmp/loft-promo-crops.txt

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

# Promo script logs "SHOT <name>". Each one is cropped to the menu bar and the
# notch. The script parks the window at the top-right for that capture only;
# "hold" still grabs the menu bar and tries the tray menu.
crop_promo() {
  local name=$1
  local line dpr sw sh
  line=$(grep -a "SHOT ${name} |" "$log" | tail -1 | tr -d '\r')
  dpr=$(grep -a -o 'dpr=[0-9.][0-9.]*' "$log" | tail -1 | sed 's/dpr=//')
  dpr=${dpr:-1}
  # screen=WxH in the promo log is CSS pixels, the unit screencapture -R uses.
  sw=$(grep -a -o 'screen=[0-9][0-9]*x[0-9][0-9]*' "$log" | tail -1 | sed 's/screen=//;s/x.*//')
  sh=$(grep -a -o 'screen=[0-9][0-9]*x[0-9][0-9]*' "$log" | tail -1 | sed 's/screen=[0-9]*x//')
  sw=${sw:-1024}
  sh=${sh:-768}
  python3 - "$name" "$line" "$dpr" "$sw" "$sh" <<'PY'
import re, subprocess, sys
name, line, dpr, sw, sh = sys.argv[1:]
dpr = float(dpr) or 1.0
sw, sh = int(sw), int(sh)
dest = f"/tmp/loft-macos-{name}.png"
m = re.search(r"window (\d+)x(\d+) at (-?\d+),(-?\d+)", line)
if not m:
    note = f"crop {name}: no window geometry; full screen"
    print(note)
    open("/tmp/loft-promo-crops.txt", "a").write(note + "\n")
    subprocess.run(["screencapture", "-x", dest], check=False)
    sys.exit(0)
w, h, x, y = [int(v) / dpr for v in m.groups()]
pad_left, pad_bottom = 220, 36
left = max(0, int(round(x - pad_left)))
top = 0
bottom = min(sh, int(round(y + h + pad_bottom)))
if bottom - top < 120:
    bottom = min(sh, top + 120)
width = max(1, sw - left)
height = max(1, bottom - top)
note = f"crop {name}: window {w:.0f}x{h:.0f} at {x:.0f},{y:.0f} -> {left},{top} {width}x{height} on {sw}x{sh} dpr {dpr:g}"
print(note)
open("/tmp/loft-promo-crops.txt", "a").write(note + "\n")
subprocess.run(["screencapture", "-x", "-R", f"{left},{top},{width},{height}", dest], check=False)
PY
}

promo_seen=" "
promo_shots() {
  [ -f "$log" ] || return 0
  local name
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    case " $promo_seen " in
      *" $name "*) continue ;;
    esac
    promo_seen="$promo_seen $name "
    if [ "$name" = "hold" ]; then
      local width
      width=$(system_profiler SPDisplaysDataType | sed -n 's/.*Resolution: \([0-9][0-9]*\) x [0-9][0-9]*.*/\1/p' | head -1)
      width=${width:-1440}
      screencapture -x -R "0,0,${width},90" /tmp/loft-macos-menubar.png || true
      # Opening the status menu needs Accessibility. A refusal is recorded;
      # the menu-bar strip above is the capture we still keep.
      osascript <<'EOF' > /tmp/loft-tray-click.txt 2>&1 || true
tell application "System Events"
  set procs to {"loft", "Loft"}
  repeat with procName in procs
    if exists process procName then
      tell process procName
        set frontmost to true
        try
          click (first menu bar item of menu bar 2 whose description contains "Loft" or name contains "Loft")
          return "clicked " & procName
        end try
      end tell
    end if
  end repeat
  return "no Loft status item"
end tell
EOF
      sleep 1
      screencapture -x /tmp/loft-macos-tray.png || true
      echo "tray click: $(tr '\n' ' ' < /tmp/loft-tray-click.txt)"
    else
      crop_promo "$name"
      echo "captured loft-macos-${name}.png"
    fi
  done < <(grep -a -o 'SHOT [a-z0-9-]*' "$log" | awk '{print $2}')
}

deadline=$((SECONDS + timeout_s))
while kill -0 "$pid" 2>/dev/null; do
  if [ -f "$log" ]; then
    promo_shots
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
pkill -f "binaries/mac-arm64-metal/llama-server" 2>/dev/null || true
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
