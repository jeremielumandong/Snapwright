#!/usr/bin/env bash
# End-to-end overlay test under Xvfb: hotkey, overlay timing, Esc, drag-select, clipboard.
# Usage: xvfb-run -a -s "-screen 0 1600x1000x24" scripts/overlay-e2e.sh <target dir>
# Needs xdotool, xclip, xsetroot (x11-xserver-utils), ImageMagick and a Vulkan driver.
set -euo pipefail
BIN=${1:-target/release}
OUT=$(mktemp -d)
export HOME=$OUT/home
mkdir -p "$HOME"
SNAPWRIGHT_LOG=info "$BIN/snapwright-app" > "$OUT/app.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
# Paint after the app connects: Xvfb resets the root window when its last client leaves.
sleep 1
xsetroot -mod 16 16 -fg '#ff6600' -bg '#3366cc'
sleep 5 # GPU warm-up on a software renderer

pixel() { convert "$1" -format "%[pixel:p{$2,$3}]" info:; }
fail() { echo "FAIL: $*"; cat "$OUT/app.log"; exit 1; }

# 1. Hotkey shows the overlay (the frozen frame, dimmed); Esc closes it.
xdotool key ctrl+shift+2; sleep 1
"$BIN/snapwright" capture -o "$OUT/overlay.png" 2>/dev/null
[ "$(pixel "$OUT/overlay.png" 300 300)" = "srgba(28,56,112,1)" ] || fail "overlay not shown dimmed"
xdotool key Escape; sleep 1
"$BIN/snapwright" capture -o "$OUT/after.png" 2>/dev/null
[ "$(pixel "$OUT/after.png" 300 300)" = "srgba(51,102,204,1)" ] || fail "Esc did not close the overlay"

# 2. Drag selects 400x300; release copies it to the clipboard as PNG and saves it.
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 200 150 mousedown 1 mousemove 400 300 mousemove 600 450; sleep 0.3
xdotool mouseup 1; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/clip.png"
[ "$(identify -format '%wx%h' "$OUT/clip.png")" = "400x300" ] || fail "clipboard image size"
[ "$(pixel "$OUT/clip.png" 10 10)" = "srgba(255,102,0,1)" ] || fail "clipboard pixels"
[ -n "$(find "$HOME" -path "*Snapwright/*.png" -print -quit)" ] || fail "not saved"

grep -E "overlay visible|on the clipboard" "$OUT/app.log" | sed 's/\x1b\[[0-9;]*m//g'
echo "PASS"
