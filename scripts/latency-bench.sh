#!/usr/bin/env bash
# Hotkey latency harness under Xvfb. Presses the real global hotkeys with xdotool and reads the
# app's own timings, measured from the moment the OS delivered the hotkey:
#   overlay  hotkey to the first frame of the selection overlay (target < 50 ms on hardware)
#   overlay+preview  the same, with the previous capture's preview still on screen
#   repeat   repeat-last-region hotkey to the image on the clipboard (target < 100 ms)
# Prints min/p50/p95/max, writes latency.json, appends a table to $GITHUB_STEP_SUMMARY when
# set, and fails when a p95 exceeds its budget. CI budgets are loose regression guards: hosted
# runners render with llvmpipe on shared CPUs, so hardware targets do not apply there.
# Usage: xvfb-run -a -s "-screen 0 1920x1080x24" scripts/latency-bench.sh <target dir>
# Env: RUNS (20), OVERLAY_P95_MAX_MS (400), REPEAT_P95_MAX_MS (60), E2E_OUT (temp dir).
set -euo pipefail
BIN=${1:-target/release}
RUNS=${RUNS:-20}
OVERLAY_MAX=${OVERLAY_P95_MAX_MS:-400}
REPEAT_MAX=${REPEAT_P95_MAX_MS:-60}
OUT=${E2E_OUT:-$(mktemp -d)}
export HOME=$OUT/home
mkdir -p "$HOME"
NO_COLOR=1 SNAPWRIGHT_LOG=info "$BIN/snapwright-app" > "$OUT/bench.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1
xsetroot -mod 16 16 -fg '#ff6600' -bg '#3366cc'
sleep 5 # GPU warm-up on a software renderer

for _ in $(seq "$RUNS"); do
  xdotool key ctrl+shift+2; sleep 0.6
  xdotool key Escape; sleep 0.4
done
mark0=$(wc -l < "$OUT/bench.log")
# A capture leaves its preview up; the next hotkey has to get it off screen first.
for _ in $(seq "$RUNS"); do
  xdotool key ctrl+shift+2; sleep 0.6
  xdotool mousemove 800 500 click 1; sleep 0.8
  xdotool key ctrl+shift+2; sleep 0.6
  xdotool key Escape; sleep 0.4
done
mark1=$(wc -l < "$OUT/bench.log")
# One drag sets the region; every repeat after it skips the overlay.
xdotool key ctrl+shift+2; sleep 1
xdotool mousemove 200 150 mousedown 1 mousemove 500 400 mouseup 1; sleep 1.5
mark=$(wc -l < "$OUT/bench.log")
for _ in $(seq "$RUNS"); do
  xdotool key ctrl+alt+shift+2; sleep 0.5
done
sleep 1

ms() { sed 's/\x1b\[[0-9;]*m//g' | grep -F "$1" | sed -E 's/.* ms=([0-9.]+).*/\1/'; }
overlay=$(head -n "$mark0" "$OUT/bench.log" | ms "overlay visible (from hotkey)")
# Every second overlay of that loop opened over a preview.
covered=$(sed -n "$((mark0 + 1)),${mark1}p" "$OUT/bench.log" | ms "overlay visible (from hotkey)" | awk 'NR % 2 == 0')
repeat=$(tail -n +"$((mark + 1))" "$OUT/bench.log" | ms "on the clipboard (from hotkey")

# "name max values..." -> "name n min p50 p95 max budget verdict"
stats() {
  local name=$1 budget=$2
  shift 2
  printf '%s\n' "$@" | sort -n | awk -v name="$name" -v budget="$budget" '
    { v[NR] = $1 }
    END {
      if (NR == 0) { print name, 0, "-", "-", "-", "-", budget, "MISSING"; exit }
      p50 = v[int((NR - 1) * 0.50) + 1]; p95 = v[int((NR - 1) * 0.95) + 1]
      printf "%s %d %.1f %.1f %.1f %.1f %d %s\n", name, NR, v[1], p50, p95, v[NR], budget,
        (p95 <= budget ? "ok" : "OVER")
    }'
}
# shellcheck disable=SC2086
rows=$(stats overlay "$OVERLAY_MAX" $overlay; stats overlay+preview "$OVERLAY_MAX" $covered; stats repeat "$REPEAT_MAX" $repeat)

echo "metric           n    min     p50     p95     max   budget(p95)"
echo "$rows" | awk '{ printf "%-16s %-4s %-7s %-7s %-7s %-7s %s ms  %s\n", $1, $2, $3, $4, $5, $6, $7, $8 }'
echo "$rows" | awk 'BEGIN { printf "{" } NR > 1 { printf "," }
  { printf "\"%s\":{\"n\":%s,\"min_ms\":\"%s\",\"p50_ms\":\"%s\",\"p95_ms\":\"%s\",\"max_ms\":\"%s\",\"budget_ms\":%s}", $1, $2, $3, $4, $5, $6, $7 }
  END { print "}" }' > "$OUT/latency.json"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Hotkey latency (Xvfb, software renderer, $RUNS runs)"
    echo
    echo "| | n | min | p50 | p95 | max | p95 budget | |"
    echo "|---|---|---|---|---|---|---|---|"
    echo "$rows" | awk '{ printf "| %s | %s | %s | %s | %s | %s | %s | %s |\n", $1, $2, $3, $4, $5, $6, $7, $8 }'
  } >> "$GITHUB_STEP_SUMMARY"
fi
if echo "$rows" | grep -qE "OVER|MISSING"; then
  echo "FAIL: latency over budget or missing; log in $OUT/bench.log"
  exit 1
fi
echo "PASS"
