#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_dir"
binary_path=${PULSE_BINARY:-./target/debug/pulse}
exec "$binary_path" app --companion --mcp-port 7432 --live --ingest-interval 300 "$@"
