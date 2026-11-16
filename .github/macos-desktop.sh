#!/bin/bash
# Desktop for the promo screenshots on the macOS runner.
#
# The display stays at whatever mode it already has unless a larger one is
# listed; we then pick that. A light wallpaper is set so the notch is not
# sitting on black. The dock is hidden. Nothing here moves Loft's window.
set -euo pipefail

note=/tmp/loft-desktop.txt
: > "$note"
say() {
  echo "$*" | tee -a "$note"
}

say "===== display modes ====="
# Best effort: a runner that lists only one mode, or a Swift failure, keeps it.
if ! swift - <<'SWIFT' | tee -a "$note"
import CoreGraphics
import Foundation

let display = CGMainDisplayID()
guard let current = CGDisplayCopyDisplayMode(display) else {
    print("no current display mode")
    exit(0)
}
print("current points \(current.width)x\(current.height) pixels \(current.pixelWidth)x\(current.pixelHeight)")

guard let modes = CGDisplayCopyAllDisplayModes(display, nil) as? [CGDisplayMode], !modes.isEmpty else {
    print("no modes listed; keeping the current one")
    exit(0)
}

var best = current
var bestPixels = current.pixelWidth * current.pixelHeight
for mode in modes {
    let pixels = mode.pixelWidth * mode.pixelHeight
    print("mode points \(mode.width)x\(mode.height) pixels \(mode.pixelWidth)x\(mode.pixelHeight) refresh \(mode.refreshRate)")
    if pixels > bestPixels {
        bestPixels = pixels
        best = mode
    }
}

if best.pixelWidth == current.pixelWidth && best.pixelHeight == current.pixelHeight && best.width == current.width {
    print("already at the highest listed mode")
    exit(0)
}

print("switching to points \(best.width)x\(best.height) pixels \(best.pixelWidth)x\(best.pixelHeight)")
var config: CGDisplayConfigRef?
let began = CGBeginDisplayConfiguration(&config)
guard began == .success, let config else {
    print("could not begin a display configuration (\(began.rawValue)); keeping the current mode")
    exit(0)
}
let set = CGConfigureDisplayWithDisplayMode(config, display, best, nil)
guard set == .success else {
    print("could not select that mode (\(set.rawValue)); keeping the current mode")
    CGCancelDisplayConfiguration(config)
    exit(0)
}
let done = CGCompleteDisplayConfiguration(config, .forSession)
print("display configuration \(done == .success ? "applied" : "failed (\(done.rawValue))")")
SWIFT
then
  say "could not adjust the display mode; keeping the current one"
fi
sleep 1

say "===== wallpaper ====="
python3 - <<'PY'
import struct, zlib

w, h = 1600, 1000

def pixel(x, y):
    nx = x / (w - 1)
    ny = y / (h - 1)
    # Light paper at the top, a cool gray-blue toward the bottom.
    r = 246 - 22 * ny - 8 * abs(nx - 0.45)
    g = 244 - 16 * ny - 4 * abs(nx - 0.45)
    b = 238 - 2 * ny + 10 * ny
    return bytes(max(0, min(255, int(round(v)))) for v in (r, g, b))

raw = bytearray()
for y in range(h):
    raw.append(0)
    for x in range(w):
        raw.extend(pixel(x, y))

def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")
open("/tmp/loft-desktop.png", "wb").write(png)
print(f"wrote /tmp/loft-desktop.png {w}x{h}")
PY

set_wallpaper() {
  WALLPAPER=/tmp/loft-desktop.png swift - <<'SWIFT'
import AppKit
import Foundation

_ = NSApplication.shared
let path = ProcessInfo.processInfo.environment["WALLPAPER"] ?? ""
let url = URL(fileURLWithPath: path)
let screens = NSScreen.screens
if screens.isEmpty {
    fputs("no screens\n", stderr)
    exit(1)
}
for screen in screens {
    try NSWorkspace.shared.setDesktopImageURL(url, for: screen, options: [:])
    print("wallpaper set \(screen.frame)")
}
SWIFT
}

if ! set_wallpaper | tee -a "$note"; then
  say "AppKit wallpaper failed; trying Finder"
  osascript -e 'tell application "Finder" to set desktop picture to POSIX file "/tmp/loft-desktop.png"' | tee -a "$note" || true
fi

# The dock slides away so it is not in the picture. Restarting it also
# picks up a new desktop picture.
defaults write com.apple.dock autohide -bool true
killall Dock || true
sleep 2

sample_luma() {
  screencapture -x -R 40,140,80,80 /tmp/loft-desktop-sample.png
  swift - <<'SWIFT'
import AppKit
let url = URL(fileURLWithPath: "/tmp/loft-desktop-sample.png")
guard let img = NSImage(contentsOf: url),
      let tiff = img.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff) else {
    fputs("could not read the desktop sample\n", stderr)
    exit(1)
}
let x = min(8, rep.pixelsWide - 1)
let y = min(8, rep.pixelsHigh - 1)
guard let color = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else {
    fputs("no pixel\n", stderr)
    exit(1)
}
var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
color.getRed(&r, green: &g, blue: &b, alpha: &a)
let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b
print(String(format: "desktop luma %.3f rgb %.3f %.3f %.3f", luma, r, g, b))
if luma < 0.45 { exit(2) }
SWIFT
}

if ! sample_luma | tee -a "$note"; then
  say "desktop still dark; setting the picture again"
  set_wallpaper | tee -a "$note" || true
  killall Dock || true
  sleep 3
  sample_luma | tee -a "$note"
fi

say "===== display now ====="
system_profiler SPDisplaysDataType | sed -n '1,40p' | tee -a "$note"
