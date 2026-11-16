#!/bin/bash
# Run one Loft dev smoke on the Linux display and stop when it reports.
#
#   .github/linux-run.sh <success-marker> <timeout-seconds> <screenshot-tag>
#
# The display comes from `.github/linux-display.sh` (DISPLAY and the GTK
# scale). The success marker is a "[dev] ..." line. "[dev] FAILED:" fails
# the run. Screenshots are cropped to the window geometry dev_log prints,
# not the whole (mostly empty) framebuffer.
set -uo pipefail
set -m

marker=${1:?success marker}
timeout_s=${2:?timeout seconds}
shot_tag=${3:-}

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
log=/tmp/loft-dev-run.log

# Indexing the seven sample files finishes before a screenshot can land.
# A copy with the same notes repeated keeps "Indexing…" on screen.
if [ "${LOFT_SMOKE_SCRIPT:-}" = "linux-promo.js" ]; then
  promo=/tmp/loft-promo-docs/sample-docs
  rm -rf /tmp/loft-promo-docs
  mkdir -p "$promo/notes"
  cp -a "$root/sample-docs/." "$promo/"
  for i in $(seq 1 30); do
    cp "$root/sample-docs/project-plan.md" "$promo/notes/plan-$(printf '%02d' "$i").md"
  done
  export LOFT_SEED_FOLDER="$promo"
fi
rm -f "$log"
mkdir -p /tmp/loft-shots

# A pty, so Rust's stderr stays line-buffered and the marker shows up
# while the app is still running.
script -q -f -c "scripts/smoke.sh ${LOFT_SMOKE_SCRIPT:-smoke-ui.js}" "$log" &
pid=$!
sleep 0.5
pgid=$(ps -o pgid= -p "$pid" 2>/dev/null | tr -d ' ' || true)
my_pgid=$(ps -o pgid= -p $$ | tr -d ' ')

hotkey_sent=0
press_hotkey() {
  [ "$hotkey_sent" = 0 ] || return 0
  grep -a -q "HOTKEY-WAIT" "$log" 2>/dev/null || return 0
  hotkey_sent=1
  # XTEST, so the grab registered by the global-shortcut plugin sees it.
  DISPLAY=${DISPLAY:-:99} xdotool key ctrl+shift+space >/tmp/loft-hotkey.txt 2>&1 || true
  echo "sent Ctrl+Shift+Space"
}

# "SHOT <name>" plus the geometry dev_log appends: "window WxH at X,Y".
crop_window() {
  local name=$1 line w h x y dest
  line=$(grep -a "SHOT ${name} " "$log" | tail -1 || true)
  [ -n "$line" ] || return 0
  if [[ "$line" =~ window\ ([0-9]+)x([0-9]+)\ at\ (-?[0-9]+),(-?[0-9]+) ]]; then
    w=${BASH_REMATCH[1]}
    h=${BASH_REMATCH[2]}
    x=${BASH_REMATCH[3]}
    y=${BASH_REMATCH[4]}
  else
    echo "no geometry for SHOT ${name}"
    return 0
  fi
  dest="/tmp/loft-${shot_tag}-${name}.png"
  [ -f "$dest" ] && return 0
  # Keep the shadow margin; drop the rest of the framebuffer.
  import -window root -crop "${w}x${h}+${x}+${y}" +repage "$dest" 2>/dev/null \
    || import -window root -crop "${w}x${h}+${x}+${y}" +repage "$dest" || true
  echo "captured ${dest} (${w}x${h} at ${x},${y})"
}

crop_tray() {
  local dest=/tmp/loft-${shot_tag}-tray.png
  [ -f "$dest" ] && return 0
  # The indicator is drawn inside the panel, not as its own window. The
  # StatusNotifier plugin is the first item, at the left of the bottom bar.
  local geom
  geom=$(DISPLAY=${DISPLAY:-:99} xwininfo -root -tree 2>/dev/null | awk '
    /"xfce4-panel"/ {
      w = h = x = y = ""
      # Skip the "0xNNNN" window id: the size is the first " WxH+X+Y".
      if (match($0, / [0-9]+x[0-9]+\+[0-9]+\+[0-9]+/)) {
        split(substr($0, RSTART + 1, RLENGTH - 1), a, /[x+]/)
        w = a[1]; h = a[2]
      }
      # Absolute origin is the last +X+Y on the line.
      s = $0
      while (match(s, /\+[0-9]+\+[0-9]+/)) {
        pair = substr(s, RSTART, RLENGTH)
        s = substr(s, RSTART + RLENGTH)
        split(substr(pair, 2), b, /\+/)
        x = b[1]; y = b[2]
      }
      if (w+0 > 200 && (best == "" || y+0 > besty)) {
        best = "yes"; bestw = w; besth = h; bestx = x; besty = y
      }
    }
    END { if (best != "") print bestw, besth, bestx, besty }
  ')
  if [ -z "$geom" ]; then
    echo "no panel to crop the tray from"
    return 0
  fi
  local w h x y
  read -r w h x y <<<"$geom"
  # The icon sits at the left end of the bar. Keep a little of the bar
  # around it so it reads as an app indicator, not a loose glyph.
  local cw=120
  local ch=$h
  local cx=$x
  local cy=$y
  import -window root -crop "${cw}x${ch}+${cx}+${cy}" +repage "$dest" 2>/dev/null || true
  echo "captured ${dest} from panel ${w}x${h} at ${x},${y}"
}

promo_seen=" "
promo_shots() {
  [ -n "$shot_tag" ] || return 0
  [ -f "$log" ] || return 0
  local name
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    case " $promo_seen " in
      *" $name "*) continue ;;
    esac
    promo_seen="$promo_seen $name "
    if [ "$name" = "tray" ]; then
      crop_tray
    else
      crop_window "$name"
    fi
  done < <(grep -a -o 'SHOT [a-z0-9-]*' "$log" | awk '{print $2}')
}

deadline=$((SECONDS + timeout_s))
while kill -0 "$pid" 2>/dev/null; do
  if [ -f "$log" ]; then
    press_hotkey
    promo_shots
    if grep -a -q -e "$marker" -e "FAILED:" "$log"; then
      promo_shots
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
  sleep 0.3
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
pkill -f "binaries/linux-x64-.*/llama-server" 2>/dev/null || true
pkill -f "vite.js" 2>/dev/null || true
pkill -f "node_modules/.bin/vite" 2>/dev/null || true
sleep 1

echo "----- [dev] lines -----"
grep -a "\[dev\]" "$log" || true
echo "----- shortcut / tray -----"
grep -a -E "could not (create the tray icon|register global shortcut)" "$log" || echo "(no tray or shortcut error logged)"
if [ -f /tmp/loft-hotkey.txt ]; then
  echo "----- xdotool -----"
  cat /tmp/loft-hotkey.txt
fi

if grep -a -q "FAILED:" "$log"; then
  echo "smoke reported FAILED"
  exit 1
fi
if ! grep -a -q "$marker" "$log"; then
  echo "smoke ended without ${marker}"
  exit 1
fi
if grep -a -q -E "could not (create the tray icon|register global shortcut)" "$log"; then
  echo "tray or hotkey registration failed"
  exit 1
fi
if grep -a -q "probe hotkey did not toggle" "$log"; then
  echo "Ctrl+Shift+Space was sent and the panel did not open"
  exit 1
fi
exit 0
