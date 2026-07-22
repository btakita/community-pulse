#!/usr/bin/env bash
set -euo pipefail

video_path="${1:-demo/community-pulse-demo.mp4}"
screenshot_path="${2:-demo/community-pulse.png}"
binary_path="${PULSE_BINARY:-target/debug/pulse}"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "Run through xvfb-run or from an X11 session." >&2
  exit 1
fi
if [[ ! -x "$binary_path" ]]; then
  echo "Build the app first: cargo build" >&2
  exit 1
fi

demo_dir="$(mktemp -d)"
app_pid=""
ffmpeg_pid=""
cleanup() {
  if [[ -n "$ffmpeg_pid" ]]; then
    kill -INT "$ffmpeg_pid" 2>/dev/null || true
    wait "$ffmpeg_pid" 2>/dev/null || true
  fi
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  rm -rf "$demo_dir"
}
trap cleanup EXIT

mkdir -p "$(dirname "$video_path")" "$(dirname "$screenshot_path")"

SLINT_BACKEND=winit-software SLINT_SCALE_FACTOR=1 "$binary_path" \
  --database "$demo_dir/pulse.db" --fixture --replay app &
app_pid=$!

window_id="$(xdotool search --sync --name '^Community Pulse$' | head -n 1)"
xdotool windowmove "$window_id" 0 0
xdotool windowsize "$window_id" 1480 900
sleep 1

ffmpeg -hide_banner -loglevel error -y \
  -f x11grab -framerate 15 -video_size 1480x900 -i "${DISPLAY}+0,0" \
  -c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p "$video_path" &
ffmpeg_pid=$!

# Show the real replay tool path, shared interest changes, inline evidence, and tracking.
xdotool mousemove --window "$window_id" 1315 788 click 1
sleep 3
xdotool mousemove --window "$window_id" 222 512 click 1
sleep 2
xdotool mousemove --window "$window_id" 974 211 click 1
sleep 3
xdotool windowfocus --sync "$window_id"
xdotool mousemove --window "$window_id" 1280 836 click 1
xdotool type --clearmodifiers --delay 35 "Track WASM runtimes for me"
xdotool key Return
sleep 4

import -window "$window_id" "$screenshot_path"

kill -INT "$ffmpeg_pid"
wait "$ffmpeg_pid" || true
ffmpeg_pid=""

echo "wrote $video_path"
echo "wrote $screenshot_path"
