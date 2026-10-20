#!/usr/bin/env bash
# Reads the testers' telemetry from the live server, for any agent or person
# debugging their problems or building training data:
#
#   telemetry/pull.sh stats
#       installs, most recent first: app version, OS, events, audio sent
#   telemetry/pull.sh export <out.jsonl> [--install <id>] [--since 2026-10-19]
#                            [--setting dialog=true]...
#       a line per dictated segment, with everything that happened to it
#       (what was heard, the commands, the cleanup's before and after, what
#       was typed, "scratch that"), its audio's key, and the settings it was
#       dictated with (`settings`, from 0.1.17); plus a line per session
#       (segment null) with its system, settings and log events (GPU
#       problems start there). --setting keeps only segments dictated with
#       those settings: dialog=true, tidy=false, destination=pad,
#       list_mode=true, numbers=style-guide, … (any key of the settings
#       file). Format: lib/export.js and docs/telemetry.md.
#   telemetry/pull.sh audio <audio key from the export> <out.flac>
#       one segment's recording (16 kHz mono FLAC)
#
# Uses the read key in ~/.config/beardictate-release/telemetry-read-key
# (BD_TELEMETRY_READ_KEY_FILE to change it), which only reads: the app's own
# key, built into every copy, can't. BD_TELEMETRY_URL points at another
# server (dev.sh's). The export holds testers' words: keep it out of the repo.
set -euo pipefail

base="${BD_TELEMETRY_URL:-https://beardictate-telemetry.jkbase.app}"
key_file="${BD_TELEMETRY_READ_KEY_FILE:-$HOME/.config/beardictate-release/telemetry-read-key}"
[ -f "$key_file" ] || { echo "no read key at $key_file (see telemetry/README.md)" >&2; exit 1; }

# The key goes to curl on stdin, never on a command line.
get() {
    printf 'header = "Authorization: Bearer %s"\n' "$(tr -d '[:space:]' < "$key_file")" |
        curl --config - --silent --show-error --fail-with-body --compressed "$@"
}

usage() { sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

case "${1:-}" in
    stats)
        get "$base/v1/stats"
        echo
        ;;
    export)
        out="${2:-}"; [ -n "$out" ] || usage
        shift 2
        query=""
        while [ $# -gt 0 ]; do
            case "$1" in
                --install) query="$query&install=${2:?--install needs an id}"; shift 2 ;;
                --since) query="$query&since=${2:?--since needs a date}"; shift 2 ;;
                --setting) query="$query&setting=$(printf %s "${2:?--setting needs key=value}" | sed 's/=/%3D/')"; shift 2 ;;
                *) usage ;;
            esac
        done
        get "$base/v1/export?${query#&}" -o "$out"
        echo "$(wc -l < "$out") lines → $out" >&2
        ;;
    audio)
        key="${2:-}" out="${3:-}"
        [[ "$key" =~ ^audio/[0-9a-f-]+/[0-9a-f-]+/[0-9]+\.flac$ ]] && [ -n "$out" ] || usage
        get "$base/v1/$key" -o "$out"
        echo "$out" >&2
        ;;
    *) usage ;;
esac
