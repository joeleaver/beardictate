# eval

Seed of the cleanup-model eval set (DESIGN.md milestone 0).

- `run_eval.py PORT LABEL` — sends the cases to an OpenAI-compatible server on
  127.0.0.1:PORT and prints latency + outputs. Cases and the system prompt are
  inline for now; they move to `cases.jsonl` with expected outputs and a scorer.
- `run_llama_server.sh LABEL model.gguf` — starts llama-server on :8091
  (`NGL=0` for CPU-only), runs the eval, stops the server.

2026-10-14 baseline (machine heavily loaded — latencies unreliable): only
Gemma 4 12B handled unmarked dialog; Qwen3 1.7B/4B/8B did not.
