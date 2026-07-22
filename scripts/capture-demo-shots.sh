#!/usr/bin/env bash
set -euo pipefail

shot_dir="${1:-demo/shots}"
binary_path="${PULSE_BINARY:-target/debug/pulse}"
capture_tmp_dir="$(mktemp -d)"
app_pid=""

cleanup() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  rm -rf -- "$capture_tmp_dir"
}
trap cleanup EXIT

for command_name in xdotool import; do
  if ! command -v "$command_name" >/dev/null; then
    echo "demo-shots requires $command_name" >&2
    exit 1
  fi
done
if [[ -z "${DISPLAY:-}" ]]; then
  echo "demo-shots requires an X11 display; run it through xvfb-run" >&2
  exit 1
fi
if [[ ! -x "$binary_path" ]]; then
  echo "Build the app first: cargo build --all-features" >&2
  exit 1
fi

mkdir -p "$shot_dir"

stop_app() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
    app_pid=""
  fi
}

wait_for_window() {
  local pid="$1"
  xdotool search --sync --onlyvisible --limit 1 --pid "$pid"
}

SLINT_BACKEND=winit-software SLINT_SCALE_FACTOR=1 "$binary_path" \
  --database "$capture_tmp_dir/desktop.db" --fixture --replay app --no-mcp &
app_pid=$!
desktop_window="$(wait_for_window "$app_pid")"
xdotool windowmove "$desktop_window" 0 0
xdotool windowsize "$desktop_window" 1480 900
sleep 1
import -window "$desktop_window" "$shot_dir/desktop-pulse.png"
import -window "$desktop_window" "$shot_dir/01-attention-budget.png"

# Ask for the ranked pulse through the replay agent.
xdotool mousemove --window "$desktop_window" 1138 788 click 1
sleep 1
import -window "$desktop_window" "$shot_dir/02-agent-pulse.png"

# Tune through chat to demonstrate that both controls share one state.
xdotool mousemove --window "$desktop_window" 1244 788 click 1
sleep 1
import -window "$desktop_window" "$shot_dir/03-shared-controls.png"

# Exercise the first digest card's evidence control and expanded treatment.
xdotool mousemove --window "$desktop_window" 892 216 click 1
sleep 1
import -window "$desktop_window" "$shot_dir/desktop-evidence.png"
import -window "$desktop_window" "$shot_dir/04-evidence-not-vibes.png"

# Drive the personal-bridge beat through the same deterministic input.
xdotool windowfocus --sync "$desktop_window"
xdotool mousemove --window "$desktop_window" 1250 834 click 1
xdotool key ctrl+a
xdotool type --delay 12 "Track WASM runtimes for me"
xdotool key Return
sleep 1
import -window "$desktop_window" "$shot_dir/05-personal-bridge.png"

# The collapsed rail and resized right pane are stable demo affordances too.
xdotool mousemove --window "$desktop_window" 231 143 click 1
sleep 0.25
import -window "$desktop_window" "$shot_dir/06-mix-collapsed.png"
xdotool mousemove --window "$desktop_window" 1056 450 click 1
sleep 0.25
import -window "$desktop_window" "$shot_dir/07-resizable-agent-pane.png"
xdotool mousemove --window "$desktop_window" 1125 151 click 1
sleep 0.25
import -window "$desktop_window" "$shot_dir/08-resizable-research-pane.png"

# Stress the minimum supported desktop viewport with both side panels open.
xdotool mousemove --window "$desktop_window" 1010 151 click 1
xdotool mousemove --window "$desktop_window" 946 450 click 1
xdotool mousemove --window "$desktop_window" 21 143 click 1
xdotool windowsize "$desktop_window" 1100 720
sleep 0.5
import -window "$desktop_window" "$shot_dir/09-minimum-viewport.png"
stop_app

SLINT_BACKEND=winit-software SLINT_SCALE_FACTOR=1 "$binary_path" \
  --database "$capture_tmp_dir/mobile.db" --fixture --replay app --mobile --no-mcp &
app_pid=$!
mobile_window="$(wait_for_window "$app_pid")"
xdotool windowmove "$mobile_window" 0 0
sleep 1
import -window "$mobile_window" "$shot_dir/mobile-pulse.png"

# Open the first evidence sheet, then rotate while the derived selection stays open.
xdotool mousemove --window "$mobile_window" 360 252 click 1
sleep 1
import -window "$mobile_window" "$shot_dir/mobile-evidence.png"
xdotool windowfocus --sync "$mobile_window"
xdotool key --window "$mobile_window" ctrl+r
sleep 1
xdotool windowsize "$mobile_window" 872 418
xdotool windowmove "$mobile_window" 0 0
sleep 0.25
import -window "$mobile_window" "$shot_dir/mobile-landscape-evidence.png"
stop_app

echo "wrote deterministic demo shots to $shot_dir"
