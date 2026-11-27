"""Asks a cleanup model, through llama-server, whether a closed pause goes on.

For every boundary the listener closed (`closed` in boundaries.jsonl), one
of two questions:

  join   a one-token yes/no: "does the text after the pause carry on the
         sentence (or list item) before it?" Records P(yes) from the top
         token probabilities.
  choose the two ways to write it, split and joined, as 1 and 2 (both
         orders): "which is right?". Records P(joined), averaged over the
         two orders so the model's taste for "1" or "2" cancels out.
  tidy   the model's own cleanup prompt (`bear-tags-v1`, as the app sends
         it), given the two pieces as one, without our mark between them;
         records what it wrote, for score.py to read whether it put a
         sentence end back at the join.

The model runs in llama-server (llama.cpp), started separately:

  llama-server -m MODEL.gguf -ngl 99 -c 4096 --port 18431

Writes `<data>/answers-<name>.jsonl`: id, p_yes (join) or output (tidy).

Usage: ask.py --data DIR --name bear-join --mode join [--url http://127.0.0.1:18431]
"""

from __future__ import annotations

import argparse
import json
import math
import re
import urllib.request

import segments  # beside this script

JOIN_PROMPT = """Someone is dictating, and paused. The app ended a sentence at the pause, with a full stop. Read what they said before the pause and what they said after it.

{earlier}Before the pause: {a}
After the pause: {b}

Does what they said after the pause carry on the same {unit} as before it, so that the full stop at the pause is wrong? Answer yes or no."""

CHOOSE_PROMPT = """Someone dictated this, pausing between two parts. Which is written correctly?

{earlier}1: {first}
2: {second}

Answer 1 or 2."""

TIDY_PROMPT = """<style>us</style>
<context>
{context}
</context>
<names></names>
<dictation>
{raw}
</dictation>"""


def gemma4(user: str) -> str:
    """One user turn in Gemma 4's chat format (crates/bd-cleanup/src/llama/turns.rs)."""
    return f"<|turn>user\n{user.strip()}<turn|>\n<|turn>model\n"


def without_our_mark(text: str) -> str:
    """A with the full stop or question mark the listener closed it with."""
    return re.sub(r"[.?]\s*$", "", text.rstrip())


def read_on(text: str) -> str:
    """B's first letter in lower case, as it would read on (the recognizer
    capitalizes the start of every stream, so its capital says nothing)."""
    words = text.split(maxsplit=1)
    if not words or words[0] == "I" or words[0].startswith("I'") or words[0].isupper():
        return text
    return text[:1].lower() + text[1:]


def post(url: str, body: dict) -> dict:
    req = urllib.request.Request(url + "/completion", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as resp:
        return json.load(resp)


def p_first(answer: dict, yes: str, no: str) -> float:
    """P(yes) / (P(yes) + P(no)) over the first token's top candidates."""
    probs = answer["completion_probabilities"][0]["top_logprobs"]
    p = sum(math.exp(c["logprob"]) for c in probs if c["token"].strip().lower() == yes)
    q = sum(math.exp(c["logprob"]) for c in probs if c["token"].strip().lower() == no)
    return p / (p + q) if p + q > 0 else 0.5


def ask_one(url: str, prompt: str) -> dict:
    return post(url, {"prompt": gemma4(prompt), "n_predict": 1, "n_probs": 20, "temperature": 0, "cache_prompt": True})


def split_and_joined(b: dict, a: str) -> tuple[str, str]:
    """The pause as the app writes it now, and read on without our mark (a
    list item break written as " / ")."""
    if b["list_mode"]:
        return f"{a} / {b['b_text']}", f"{a} {read_on(b['b_text'])}"
    return f"{a}{b['a_text'].rstrip()[-1:]} {b['b_text']}", f"{a} {read_on(b['b_text'])}"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--mode", choices=("join", "choose", "tidy"), required=True)
    parser.add_argument("--url", default="http://127.0.0.1:18431")
    args = parser.parse_args()
    data = segments.data_dir(args.data)
    bounds = [json.loads(line) for line in open(data / "boundaries.jsonl", encoding="utf-8")]
    with open(data / f"answers-{args.name}.jsonl", "w", encoding="utf-8") as out:
        for b in bounds:
            if not b["closed"]:
                continue
            a, after = without_our_mark(b["a_text"]), read_on(b["b_text"])
            if args.mode == "join":
                earlier = f"Earlier: {b['before']}\n" if b["before"] else ""
                unit = "list item" if b["list_mode"] else "sentence"
                prompt = JOIN_PROMPT.format(earlier=earlier, a=a, b=after, unit=unit)
                row = {"id": b["id"], "p_yes": round(p_first(ask_one(args.url, prompt), "yes", "no"), 4)}
            elif args.mode == "choose":
                earlier = f"Earlier: {b['before']}\n" if b["before"] else ""
                split, joined = split_and_joined(b, a)
                one = p_first(ask_one(args.url, CHOOSE_PROMPT.format(earlier=earlier, first=split, second=joined)), "2", "1")
                two = p_first(ask_one(args.url, CHOOSE_PROMPT.format(earlier=earlier, first=joined, second=split)), "1", "2")
                row = {"id": b["id"], "p_yes": round((one + two) / 2, 4)}
            else:
                prompt = TIDY_PROMPT.format(context=b["before"], raw=f"{a} {after}")
                answer = post(args.url, {"prompt": gemma4(prompt), "n_predict": 256, "temperature": 0, "cache_prompt": True})
                row = {"id": b["id"], "output": answer["content"]}
            out.write(json.dumps(row) + "\n")
            out.flush()


if __name__ == "__main__":
    main()
