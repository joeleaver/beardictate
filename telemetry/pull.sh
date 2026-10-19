#!/usr/bin/env bash
# Exports the alpha telemetry as JSONL (export.js: a line per dictated
# segment, plus one per session), for any agent or person debugging testers'
# problems:
#
#   telemetry/pull.sh <out.jsonl> [--install <id>] [--since 2026-10-18]
#
# Reads a managed-DB access key from ~/.config/beardictate/telemetry-db.env
# (BD_TELEMETRY_DB_ENV overrides the path), which must hold
#   JKBASE_DB_ACCESS_KEY_ID=...
#   JKBASE_DB_SECRET=...
# and be readable only by you. Joe makes it once:
#   jkbase db key create --project beardictate-telemetry   # the secret shows once
# The key never goes on a command line: jkbase reads it from the environment.
# Runs `jkbase db proxy` on a free local port for the export, then stops it.
# The export holds testers' words and audio keys: keep it out of the repo.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:?usage: pull.sh <out.jsonl> [--install <id>] [--since <date>]}"
shift
env_file="${BD_TELEMETRY_DB_ENV:-$HOME/.config/beardictate/telemetry-db.env}"
[ -f "$env_file" ] || { echo "no $env_file: see the top of $0" >&2; exit 1; }
if [ -n "$(find "$env_file" -perm /077)" ]; then
    echo "$env_file is readable by others: chmod 600 it" >&2
    exit 1
fi
set -a
# shellcheck source=/dev/null
. "$env_file"
set +a
: "${JKBASE_DB_ACCESS_KEY_ID:?missing in $env_file}" "${JKBASE_DB_SECRET:?missing in $env_file}"

port="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
log="$(mktemp)"
jkbase db proxy --project beardictate-telemetry --listen "127.0.0.1:$port" >"$log" 2>&1 &
proxy=$!
trap 'kill "$proxy" 2>/dev/null; rm -f "$log"' EXIT
for _ in $(seq 50); do
    if ! kill -0 "$proxy" 2>/dev/null; then
        echo "jkbase db proxy exited:" >&2
        cat "$log" >&2
        exit 1
    fi
    (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break
    sleep 0.2
done
RHYPEDB_URL="http://127.0.0.1:$port" node "$here/export.js" "$out" "$@"
