#!/bin/bash
# Start the virtual display the Linux checks run on, then print the
# environment a caller should export.
#
#   eval "$(.github/linux-display.sh)"
#
# Xvfb is 1920x1080 at 1x. GDK_SCALE=2 was tried on a 3840x2160 screen:
# GTK grew the window, but WebKit kept devicePixelRatio at 1, so the
# notch sat in the top of a window twice as tall as the panel. A bottom
# panel hosts the Ayatana app indicator. picom composites the
# transparent notch, and xfwm4 honours the resize (Xvfb alone does not).
# Idempotent: a second call leaves the running display alone.
set -euo pipefail

# A private runtime, so this display does not reuse another session's bus.
runtime=/tmp/loft-runtime
mkdir -p "$runtime"
chmod 700 "$runtime"

if [ ! -S /tmp/.X11-unix/X99 ]; then
  Xvfb :99 -screen 0 1920x1080x24 -ac +extension GLX +extension RENDER +extension RANDR >/tmp/loft-xvfb.log 2>&1 &
  echo $! >/tmp/loft-xvfb.pid
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -S /tmp/.X11-unix/X99 ] && break
    sleep 0.2
  done
fi

export DISPLAY=:99
export XDG_RUNTIME_DIR="$runtime"
export GDK_SCALE=1
export GDK_BACKEND=x11
# WebKit's dmabuf renderer paints a blank view on Xvfb. Compositing mode is
# left on so devicePixelRatio can follow GDK_SCALE.
export WEBKIT_DISABLE_DMABUF_RENDERER=1

if [ ! -S "$runtime/bus" ]; then
  dbus-daemon --session --address="unix:path=$runtime/bus" --fork --print-address >/tmp/loft-dbus.addr
fi
export DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus"

# A dark desktop, so a transparent margin is not a black hole.
if [ ! -f /tmp/loft-wallpaper.png ]; then
  convert -size 1920x1080 gradient:'#2c3548-#1b2030' /tmp/loft-wallpaper.png
fi
if ! pgrep -f 'feh --bg-fill /tmp/loft-wallpaper.png' >/dev/null 2>&1; then
  if command -v feh >/dev/null 2>&1; then
    feh --bg-fill /tmp/loft-wallpaper.png >/dev/null 2>&1 || true
  else
    xsetroot -solid '#243044' || true
  fi
fi

if ! pgrep -x picom >/dev/null 2>&1; then
  picom --backend xrender --vsync >/tmp/loft-picom.log 2>&1 &
fi

# A window manager, or GTK keeps the notch at its first size.
wm_here=0
for pid in $(pgrep -x xfwm4 || true); do
  if tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -qx "DISPLAY=$DISPLAY"; then
    wm_here=1
  fi
done
if [ "$wm_here" = 0 ]; then
  xfwm4 --compositor=off >/tmp/loft-xfwm.log 2>&1 &
  sleep 0.5
fi

# A private config, so this panel does not read or write the desktop's.
# Bottom edge: the notch docks to the top and must stay visible.
export XDG_CONFIG_HOME=/tmp/loft-config
panel_config=$XDG_CONFIG_HOME/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml
if [ ! -f "$panel_config" ]; then
  mkdir -p "$(dirname "$panel_config")"
  cat >"$panel_config" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfce4-panel" version="1.0">
  <property name="configver" type="int" value="2"/>
  <property name="panels" type="array">
    <value type="int" value="1"/>
    <property name="panel-1" type="empty">
      <property name="position" type="string" value="p=10;x=0;y=0"/>
      <property name="length" type="uint" value="100"/>
      <property name="position-locked" type="bool" value="true"/>
      <property name="size" type="uint" value="40"/>
      <property name="background-style" type="uint" value="1"/>
      <property name="background-rgba" type="array">
        <value type="double" value="0.12"/>
        <value type="double" value="0.14"/>
        <value type="double" value="0.18"/>
        <value type="double" value="1"/>
      </property>
      <property name="plugin-ids" type="array">
        <value type="int" value="1"/>
      </property>
    </property>
  </property>
  <property name="plugins" type="empty">
    <property name="plugin-1" type="string" value="sntray">
      <property name="square-icons" type="bool" value="true"/>
      <property name="icon-size" type="uint" value="24"/>
    </property>
  </property>
</channel>
EOF
fi
# xfconfd is dbus-activated and would otherwise miss this config directory.
dbus-update-activation-environment DISPLAY XDG_CONFIG_HOME XDG_RUNTIME_DIR GDK_SCALE GDK_BACKEND >/dev/null
if ! pgrep -x xfconfd >/dev/null 2>&1 || ! tr '\0' '\n' </proc/$(pgrep -x xfconfd | tail -1)/environ 2>/dev/null | grep -q 'XDG_CONFIG_HOME=/tmp/loft-config'; then
  xfconfd >/tmp/loft-xfconf.log 2>&1 &
  sleep 0.3
fi

panel_here=0
for pid in $(pgrep -x xfce4-panel || true); do
  if tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -qx "DISPLAY=$DISPLAY"; then
    panel_here=1
  fi
done
if [ "$panel_here" = 0 ]; then
  xfce4-panel >/tmp/loft-panel.log 2>&1 &
  sleep 1
fi

# shellcheck disable=SC2016
printf 'export DISPLAY=%q\nexport XDG_RUNTIME_DIR=%q\nexport XDG_CONFIG_HOME=%q\nexport DBUS_SESSION_BUS_ADDRESS=%q\nexport GDK_SCALE=%q\nexport GDK_BACKEND=%q\nexport WEBKIT_DISABLE_DMABUF_RENDERER=%q\n' \
  "$DISPLAY" "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$DBUS_SESSION_BUS_ADDRESS" "$GDK_SCALE" "$GDK_BACKEND" \
  "$WEBKIT_DISABLE_DMABUF_RENDERER"
