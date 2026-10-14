#!/usr/bin/env python3
"""Run the cleanup eval against any OpenAI-compatible chat endpoint, then score.

    # against a server that is already running
    run_eval.py --base-url http://127.0.0.1:8092/v1 --model qwen3-4b --out results/qwen3-4b

    # launch llama-server for the run and stop it afterwards
    run_eval.py --launch /mnt/data/models/Qwen3-4B/Qwen3-4B-Q4_K_M.gguf --no-think --out results/qwen3-4b

Each case's raw text first goes through the command pass (commands.py, a
mirror of crates/bd-commands), as in the runtime, so the model sees quote
marks and breaks rather than the words "open quote". --no-command-pass sends
raw verbatim.

Writes <out>/outputs.jsonl ({id, output, latency_ms}), <out>/report.md and
<out>/scores.jsonl.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

import commands
import score
from prompt import Prompt
from textnorm import ends_in_open_quote

HERE = Path(__file__).resolve().parent
LLAMA_SERVER = "/mnt/data/llama.cpp/build/bin/llama-server"
MARKS = {"us": ("“", "”"), "uk": ("‘", "’")}
THINK_RE = re.compile(r"<think>.*?</think>", re.S)


def post(url: str, body: dict[str, object], timeout: float) -> dict[str, object]:
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.load(resp)  # type: ignore[no-any-return]


def model_input(case: dict[str, str], command_pass: bool) -> str:
    if not command_pass:
        return case["raw"]
    style = case.get("style", "us")
    return commands.apply(case["raw"], ends_in_open_quote(case.get("context", "")), MARKS[style]).text


def tidy(output: str, model_in: str) -> str:
    """Strip stray thinking blocks and edge whitespace, but keep a leading
    paragraph break (meaningful) and a trailing one that came from a command."""
    out = THINK_RE.sub("", output)
    lead = "\n\n" if re.match(r"^[ \t]*\n[ \t]*\n", out) else ""
    trail = "\n\n" if model_in.endswith("\n\n") else ("\n" if model_in.endswith("\n") else "")
    return lead + out.strip() + trail


def launch(model: str, port: int, ngl: int, ctx: int, log: Path) -> subprocess.Popen[bytes]:
    cmd = [LLAMA_SERVER, "-m", model, "-ngl", str(ngl), "-c", str(ctx), "--jinja",
           "--host", "127.0.0.1", "--port", str(port)]
    proc = subprocess.Popen(cmd, stdout=log.open("wb"), stderr=subprocess.STDOUT, start_new_session=True)
    for _ in range(900):
        if proc.poll() is not None:
            raise RuntimeError(f"llama-server exited early; see {log}")
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as r:
                if r.status == 200:
                    return proc
        except (urllib.error.URLError, OSError):
            pass
        time.sleep(1)
    stop(proc)
    raise RuntimeError("llama-server did not become healthy")


def stop(proc: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=30)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        os.killpg(proc.pid, signal.SIGKILL)


def run(args: argparse.Namespace, base_url: str) -> Path:
    prompt = Prompt.load(args.prompt)
    cases = [c for p in args.cases for c in score.load_jsonl(p)]
    if args.limit:
        cases = cases[: args.limit]
    args.out.mkdir(parents=True, exist_ok=True)
    out_path = args.out / "outputs.jsonl"
    url = base_url.rstrip("/") + "/chat/completions"

    def call(case: dict[str, str]) -> tuple[str, float, str]:
        model_in = model_input(case, not args.no_command_pass)
        body: dict[str, object] = {
            "model": args.model,
            "messages": prompt.messages(case.get("context", ""), model_in, case.get("style", "us")),
            "temperature": args.temperature,
            "max_tokens": args.max_tokens,
        }
        if args.no_think:
            body["chat_template_kwargs"] = {"enable_thinking": False}
        t0 = time.perf_counter()
        r = post(url, body, args.timeout)
        ms = (time.perf_counter() - t0) * 1000
        content = r["choices"][0]["message"].get("content") or ""  # type: ignore[index]
        return tidy(str(content), model_in), ms, model_in

    call({"context": "", "raw": "Um, warm up the the model."})
    with out_path.open("w", encoding="utf-8") as f:
        for k, case in enumerate(cases, 1):
            out, ms, model_in = call(case)  # type: ignore[arg-type]
            f.write(json.dumps({"id": case["id"], "output": out, "latency_ms": round(ms, 1),
                                "model_input": model_in}, ensure_ascii=False) + "\n")
            f.flush()
            if args.verbose:
                print(f"[{k}/{len(cases)}] {case['id']} {ms:.0f}ms: {out!r}")
            elif k % 20 == 0:
                print(f"  {k}/{len(cases)}", flush=True)
    return out_path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base-url", default="http://127.0.0.1:8092/v1")
    ap.add_argument("--model", default="local", help="model name sent in the request")
    ap.add_argument("--cases", type=Path, nargs="+", default=[HERE / "cases.jsonl", HERE / "cases_uk.jsonl"])
    ap.add_argument("--prompt", type=Path, default=None,
                    help="prompt file (default: crates/bd-cleanup/prompts/generic-v1.txt if present, else eval/prompts/generic-v1.txt)")
    ap.add_argument("--out", type=Path, required=True, help="results directory")
    ap.add_argument("--title", default=None)
    ap.add_argument("--temperature", type=float, default=0.0)
    ap.add_argument("--max-tokens", type=int, default=600)
    ap.add_argument("--timeout", type=float, default=600)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--no-think", action="store_true", help="send chat_template_kwargs.enable_thinking=false (Qwen3)")
    ap.add_argument("--no-command-pass", action="store_true", help="send raw text with spoken commands verbatim")
    ap.add_argument("--launch", metavar="GGUF", help="start llama-server with this model for the run, stop it after")
    ap.add_argument("--port", type=int, default=8092)
    ap.add_argument("--ngl", type=int, default=99)
    ap.add_argument("--ctx", type=int, default=4096)
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    proc = None
    base_url = args.base_url
    if args.launch:
        args.out.mkdir(parents=True, exist_ok=True)
        print(f"starting llama-server on :{args.port} with {args.launch}")
        proc = launch(args.launch, args.port, args.ngl, args.ctx, args.out / "server.log")
        base_url = f"http://127.0.0.1:{args.port}/v1"
    try:
        out_path = run(args, base_url)
    finally:
        if proc is not None:
            stop(proc)

    cases, scores, outputs = score.score_files(args.cases, out_path)
    title = args.title or f"Cleanup eval: {args.launch and Path(args.launch).name or args.model}"
    prompt_path = Prompt.load(args.prompt).path
    try:
        prompt_name = str(prompt_path.relative_to(HERE.parent))
    except ValueError:
        prompt_name = str(prompt_path)
    md = score.report(title, cases, scores, outputs)
    head, _, rest = md.partition("\n")
    meta = (f"\nprompt `{prompt_name}`; command pass {'off' if args.no_command_pass else 'on'}; "
            f"temperature {args.temperature}; {len(scores)} cases\n")
    (args.out / "report.md").write_text(head + "\n" + meta + rest + "\n", encoding="utf-8")
    print(score.table([("overall", score.aggregate(scores))]))
    print(f"report: {args.out / 'report.md'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
