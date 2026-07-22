#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_dir"
./target/debug/pulse app --companion --mcp-port 7432 --live --ingest-interval 300
