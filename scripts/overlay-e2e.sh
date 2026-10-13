#!/usr/bin/env bash
# End-to-end overlay test under Xvfb: hotkey, overlay timing, Esc, drag-select, click,
# keyboard adjust, repeat last region, colour copy, clipboard, preview, library, editor and
# screen recording.
# Usage: xvfb-run -a -s "-screen 0 1600x1000x24" scripts/overlay-e2e.sh <target dir>
# Needs xdotool, xclip, xsetroot (x11-xserver-utils), ImageMagick, ffmpeg and a Vulkan driver.
set -euo pipefail
BIN=${1:-target/release}
OUT=${E2E_OUT:-$(mktemp -d)}
export HOME=$OUT/home
mkdir -p "$HOME"
SNAPWRIGHT_LOG=info "$BIN/snapwright-app" > "$OUT/app.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
# Paint after the app connects: Xvfb resets the root window when its last client leaves.
sleep 1
xsetroot -mod 16 16 -fg '#ff6600' -bg '#3366cc'
# GPU setup on a software renderer takes from 3 s to well over 10 s on hosted runners; the
# hotkeys are registered once it is done. Then give the hidden warm-up window a frame.
for _ in $(seq 120); do
  grep -q "hotkey registered" "$OUT/app.log" && break
  sleep 0.5
done
grep -q "hotkey registered" "$OUT/app.log" || { echo "FAIL: app not ready after 60 s"; cat "$OUT/app.log"; exit 1; }
sleep 1

pixel() { convert "$1" -format "%[pixel:p{$2,$3}]" info:; }
fail() { echo "FAIL: $*"; cat "$OUT/app.log"; exit 1; }

# 1. Hotkey shows the overlay (the frozen frame, dimmed); Esc closes it. The first overlay's
# first frame can take a few seconds on a software renderer, so poll for it.
xdotool key ctrl+shift+2; sleep 1
for _ in $(seq 10); do
  "$BIN/snapwright" capture -o "$OUT/overlay.png" 2>/dev/null
  [ "$(pixel "$OUT/overlay.png" 300 300)" = "srgba(30,56,109,1)" ] && break
  sleep 0.5
done
[ "$(pixel "$OUT/overlay.png" 300 300)" = "srgba(30,56,109,1)" ] || fail "overlay not shown dimmed"
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

# 3. A click on the bare desktop takes the whole display.
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 800 500 click 1; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/click.png"
[ "$(identify -format '%wx%h' "$OUT/click.png")" = "1600x1000" ] || fail "click did not take the display"

# 4. Shift on release keeps the region; arrows move it, Shift+arrows resize, Enter copies.
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 200 150 mousedown 1 mousemove 400 300 mousemove 600 450; sleep 0.3
xdotool keydown shift mouseup 1 keyup shift; sleep 0.5
xdotool key Right Right Right ctrl+Down shift+Right shift+Right shift+Up; sleep 0.3
xdotool key Return; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/adjusted.png"
[ "$(identify -format '%wx%h' "$OUT/adjusted.png")" = "402x299" ] || fail "adjusted size"
for xy in "0 0" "7 3" "40 21"; do
  set -- $xy
  [ "$(pixel "$OUT/adjusted.png" "$1" "$2")" = "$(pixel "$OUT/after.png" $((203 + $1)) $((160 + $2)))" ] \
    || fail "adjusted region moved wrong at $1,$2"
done

# 4b. Repeat last region grabs the same region again, with no overlay.
xdotool key ctrl+alt+shift+2; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/repeat.png"
[ "$(compare -metric AE "$OUT/repeat.png" "$OUT/adjusted.png" null: 2>&1)" = "0" ] \
  || fail "repeat did not capture the same region"

# 5. C copies the colour under the pointer as hex.
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 301 301; sleep 0.3
xdotool key c; sleep 1
want=$(pixel "$OUT/after.png" 301 301 | sed -E 's/srgba?\(([0-9]+),([0-9]+),([0-9]+).*/\1 \2 \3/' | xargs printf '#%02X%02X%02X')
got=$(timeout 5 xclip -selection clipboard -o || true)
[ "$got" = "$want" ] || fail "colour copy: got '$got', want $want"

# 6. After a capture, a preview shows bottom right; clicking it opens the library.
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 100 100 mousedown 1 mousemove 300 200 mousemove 500 300; sleep 0.3
xdotool mouseup 1; sleep 1.5
"$BIN/snapwright" capture -o "$OUT/toast.png" 2>/dev/null
[ "$(pixel "$OUT/toast.png" 1470 975)" != "$(pixel "$OUT/after.png" 1470 975)" ] || fail "no preview after capture"
xdotool mousemove 1470 900 click 1; sleep 3
xdotool search --name "Snapwright Library" >/dev/null || fail "clicking the preview did not open the library"

grep -E "overlay visible|on the clipboard" "$OUT/app.log" | sed 's/\x1b\[[0-9;]*m//g'
# 7. The preview's Edit button opens the editor on that capture (400x200, from step 6).
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 100 100 mousedown 1 mousemove 300 200 mousemove 500 300; sleep 0.3
xdotool mouseup 1; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/plain.png"
xdotool mousemove 1527 966 click 1; sleep 3
eval "$(xdotool search --name "Snapwright Editor" getwindowgeometry --shell)" \
  || fail "the preview's Edit did not open the editor"
# The 400x200 image sits centred in the canvas, right of the tool rail, at 1:1.
IX=$((X + 278)); IY=$((Y + 209))
at() { echo $((IX + $1)) $((IY + $2)); }
xdotool mousemove $(at 100 100); sleep 0.3
xdotool key r mousemove $(at 48 28) mousedown 1 mousemove $(at 100 60) mousemove $(at 158 108) mouseup 1
sleep 0.3
xdotool key b mousemove $(at 300 20) mousedown 1 mousemove $(at 350 100) mousemove $(at 390 180) mouseup 1
sleep 0.3
xdotool key t mousemove $(at 200 140) click 1; sleep 0.3; xdotool type "Note"; xdotool key Return
xdotool key n mousemove $(at 30 170) click 1; sleep 1
xdotool key ctrl+c; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/annotated.png"
[ "$(identify -format '%wx%h' "$OUT/annotated.png")" = "400x200" ] || fail "annotated size"
[ "$(pixel "$OUT/annotated.png" 48 70)" = "srgba(229,72,77,1)" ] || fail "rectangle not drawn"
[ "$(compare -metric AE -extract 80x150+305+25 "$OUT/annotated.png" "$OUT/plain.png" null: 2>&1 || true)" != "0" ] \
  || fail "blur did not change the region"
[ "$(pixel "$OUT/annotated.png" 200 5)" = "$(pixel "$OUT/plain.png" 200 5)" ] || fail "untouched pixels changed"
# Undo removes the step marker, redo brings it back.
xdotool key ctrl+z; sleep 0.5; xdotool key ctrl+c; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/undone.png"
[ "$(pixel "$OUT/undone.png" 30 170)" = "$(pixel "$OUT/plain.png" 30 170)" ] || fail "undo"
xdotool key ctrl+shift+z; sleep 0.5
# Crop, then Ctrl+S saves the edit into the library; Ctrl+C copies the cropped image.
xdotool key c mousemove $(at 20 10) mousedown 1 mousemove $(at 200 100) mousemove $(at 320 190) mouseup 1
xdotool key v; sleep 0.5; xdotool key ctrl+s; sleep 1.5; xdotool key ctrl+c; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/cropped.png"
[ "$(identify -format '%wx%h' "$OUT/cropped.png")" = "300x180" ] || fail "crop size"
grep -q "edit saved" "$OUT/app.log" || fail "edit not saved"

# 8. Repeating a region the preview now covers waits for the preview to go: same pixels.
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 1300 760 mousedown 1 mousemove 1450 900 mousemove 1599 999; sleep 0.3
xdotool mouseup 1; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/corner.png"
xdotool key ctrl+alt+shift+2; sleep 1.5
xclip -selection clipboard -t image/png -o > "$OUT/corner-again.png"
[ "$(compare -metric AE "$OUT/corner.png" "$OUT/corner-again.png" null: 2>&1)" = "0" ] \
  || fail "the preview got into a repeat capture"

# 9. Ctrl+Shift+6 records a dragged region, with a bar under it showing the time; the hotkey
# again stops. The MP4 has the region's size and lands in the recordings folder.
xdotool key ctrl+shift+6; sleep 1
xdotool mousemove 200 150 mousedown 1 mousemove 400 300 mousemove 600 450; sleep 0.3
xdotool mouseup 1; sleep 1.5
"$BIN/snapwright" capture -o "$OUT/recording.png" 2>/dev/null
xdotool mousemove 300 250; sleep 0.5; xdotool mousemove 500 350; sleep 1
xdotool key ctrl+shift+6; sleep 3
"$BIN/snapwright" capture -o "$OUT/recorded.png" 2>/dev/null
[ "$(pixel "$OUT/recording.png" 400 484)" != "$(pixel "$OUT/recorded.png" 400 484)" ] \
  || fail "no recording bar under the region"
grep -q "recording saved" "$OUT/app.log" || fail "recording not saved"
mp4=$(find "$HOME" -path "*Snapwright/*.mp4" -print -quit)
[ -n "$mp4" ] || fail "no MP4 in the recordings folder"
probe=$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width,height \
  -show_entries format=duration -of default=noprint_wrappers=1 "$mp4")
echo "$probe" | grep -q "codec_name=h264" || fail "recording is not H.264: $probe"
echo "$probe" | grep -q "width=400" && echo "$probe" | grep -q "height=300" \
  || fail "recording size: $probe"
grep -E "recording saved" "$OUT/app.log" | sed 's/\x1b\[[0-9;]*m//g'

# 10. T in the overlay copies the text in the selection; captures get their text indexed.
convert -size 640x140 xc:white -font DejaVu-Sans -pointsize 30 -fill black \
  -annotate +24+56 'Snapwright reads text' -annotate +24+108 'Invoice 4821 overdue' "$OUT/text.png"
display -geometry +200+300 -title text-sample "$OUT/text.png" &
VIEWER=$!
trap 'kill $PID $VIEWER 2>/dev/null || true' EXIT
sleep 2
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 210 310 mousedown 1 mousemove 500 380 mousemove 830 430; sleep 0.3
xdotool keydown shift mouseup 1 keyup shift; sleep 0.5
xdotool key t
got=""
for _ in $(seq 40); do
  got=$(timeout 5 xclip -selection clipboard -o 2>/dev/null || true)
  echo "$got" | grep -q "overdue" && break
  sleep 0.5
done
echo "$got" | grep -qi "snapwright reads text" || fail "copy text: got '$got'"
echo "$got" | grep -q "4821" || fail "copy text: got '$got'"
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 210 310 mousedown 1 mousemove 500 380 mousemove 830 430; sleep 0.3
xdotool mouseup 1; sleep 1.5
kill $VIEWER 2>/dev/null || true
DB=$(find "$HOME" -name library.sqlite3 -print -quit)
indexed() {
  python3 -c 'import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
q = "SELECT count(*) FROM captures_fts WHERE captures_fts MATCH ?"
sys.exit(0 if db.execute(q, ("invoice* AND overdue*",)).fetchone()[0] else 1)' "$DB"
}
for _ in $(seq 60); do indexed && break; sleep 1; done
indexed || fail "capture text not indexed"
grep -E "text copied|capture text indexed" "$OUT/app.log" | sed 's/\x1b\[[0-9;]*m//g'

echo "PASS"
