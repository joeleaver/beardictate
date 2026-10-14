#!/usr/bin/env bash
# usage: run.sh label model.gguf
S=/tmp/claude-1000/-home-joe-dev/1ad64df7-bf58-4271-abc6-b11a80201b5d/scratchpad
/mnt/data/llama.cpp/build/bin/llama-server -m "$2" -ngl ${NGL:-99} -c 4096 --jinja --host 127.0.0.1 --port 8091 >"$S/server-$1.log" 2>&1 &
PID=$!
for i in $(seq 1 500); do curl -sf http://127.0.0.1:8091/health >/dev/null && break; sleep 1; done
python3 -I "$S/bench.py" 8091 "$1"
kill $PID; wait $PID 2>/dev/null
