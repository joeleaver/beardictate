#!/usr/bin/env bash
# Runs the telemetry server locally, with its own RhypeDB and a folder for
# audio, under telemetry/.data. Point an alpha build at it with
#   BD_TELEMETRY_URL=http://127.0.0.1:8787 BD_TELEMETRY_KEY=dev-key
set -euo pipefail
cd "$(dirname "$0")"

RHYPEDB_BIN=${RHYPEDB_BIN:-$HOME/dev/rhypedb/target/release/rhypedb-server}
PORT=${PORT:-8787}
DB_PORT=${DB_PORT:-4290}
mkdir -p .data

"$RHYPEDB_BIN" --schema schema.rhype --data-dir .data/db \
  --listen "127.0.0.1:$DB_PORT" --tcp-listen "127.0.0.1:$((DB_PORT + 1))" &
db=$!
trap 'kill $db 2>/dev/null' EXIT

PORT=$PORT ALPHA_KEY=${ALPHA_KEY:-dev-key} READ_KEY=${READ_KEY:-dev-read-key} RHYPEDB_URL="http://127.0.0.1:$DB_PORT" \
  AUDIO_DIR=.data/audio AUDIO_BUDGET_GB=${AUDIO_BUDGET_GB:-1} node server.js
