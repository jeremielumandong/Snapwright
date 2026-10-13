#!/usr/bin/env bash
# Renders the site's app screenshots (library and editor windows) in every theme the site
# offers, into site/img/themes/<id>/. Run under Xvfb from the repo root after a release build:
#
#   cargo build --release -p snapwright-app
#   xvfb-run -a -s "-screen 0 1600x1000x24" scripts/site-shots.sh target/release
#
# THEMES_ONLY="tokyo-night nord" renders just those (snapwright:light is the original look).
#
# Needs xdotool, ImageMagick and a Chromium (headless_shell from Playwright, or
# $SNAPWRIGHT_CHROME) to render the sample dashboard the captures come from.
set -euo pipefail

bin=${1:?usage: site-shots.sh <dir with snapwright-app>}
out=site/img/themes
# Site theme id and the app's settings key for it. "snapwright" is the original light look.
themes=(${THEMES_ONLY:-snapwright:light tokyo-night catppuccin catppuccin-latte ethereal everforest
  flexoki-light gruvbox hackerman kanagawa lumon matte-black nord osaka-jade ristretto
  rose-pine white})

chrome=${SNAPWRIGHT_CHROME:-$(ls /opt/pw-browsers/chromium_headless_shell-*/chrome-linux/headless_shell 2>/dev/null | head -1)}
work=$(mktemp -d)
trap 'pkill -f "$bin/snapwright-app" || true; rm -rf "$work"' EXIT

"$chrome" --no-sandbox --hide-scrollbars --window-size=1260,660 \
  --screenshot="$work/northwind.png" "file://$PWD/scripts/site-shots/northwind.html" >/dev/null 2>&1
cargo run -q --release -p snapwright-editor --example seed -- "$work/library" "$work/northwind.png"

# Waits for a window with this exact title and prints its id.
window() {
  for _ in $(seq 100); do
    if id=$(xdotool search --onlyvisible --name "^$1\$" 2>/dev/null | head -1) && [ -n "$id" ]; then
      echo "$id"
      return
    fi
    sleep 0.2
  done
  echo "no window named $1" >&2
  return 1
}

# Runs the app in a fresh home with `theme` saved, passing the remaining arguments.
run() {
  local theme=$1
  shift
  pkill -f "$bin/snapwright-app" || true
  sleep 0.5
  rm -rf "$work/home"
  mkdir -p "$work/home/.config/snapwright" "$work/home/.local/share/snapwright" "$work/run"
  chmod 700 "$work/run"
  echo "theme = \"$theme\"" > "$work/home/.config/snapwright/settings.toml"
  cp -r "$work/library" "$work/home/.local/share/snapwright/library"
  HOME=$work/home XDG_RUNTIME_DIR=$work/run DBUS_SESSION_BUS_ADDRESS=disabled: \
    "$bin/snapwright-app" "$@" >"$work/app.log" 2>&1 &
}

for entry in "${themes[@]}"; do
  id=${entry%%:*}
  key=${entry#*:}
  mkdir -p "$out/$id"

  run "$key" --library
  win=$(window "Snapwright Library")
  sleep 2
  # Select the annotated capture (the newest, first in the grid) so the inspector shows it.
  eval "$(xdotool getwindowgeometry --shell "$win")"
  xdotool mousemove $((X + 330)) $((Y + 150)) click 1
  xdotool mousemove $((X + 600)) $((Y + 740))
  sleep 1.5
  import -window "$win" "$out/$id/library.png"

  run "$key" --edit-latest
  win=$(window "Snapwright Editor")
  sleep 3
  import -window "$win" "$out/$id/editor.png"
  echo "$id"
done
