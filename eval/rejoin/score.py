"""Scores ways of taking back a full stop the listener added at a pause.

Reads boundaries.jsonl (boundaries.py), labels.jsonl (`mid`/`end`/`unsure`,
from label_prompt.md) and any answers-<name>.jsonl
(ask.py). Prints, per mode, how many of the closed pauses inside a sentence
(or list item) each rule rejoins, and how many real ends it wrongly joins.

Closed pauses where the speaker then only said a mark ("question mark") or
began with a command ("scratch that", "new line") aren't scored: the app
already puts a said mark in place of ours, and a command isn't the rest of
a sentence.

In list mode it also scores the pauses he went on from before the
listener's deadline (not closed, so today each is a new item anyway):
joining every one, or every one the recognizer didn't end with . ? !.

Usage: score.py --data DIR [--answers bear-join gemma-join bear-tidy ...] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import re

import segments  # beside this script

# Words a sentence can't end on: the listener's UNFINISHED less the ones
# that end sentences as often as not ("is", "you", "in", "like"…).
DANGLING = {
    "a", "an", "the", "and", "or", "but", "nor", "to", "of", "with", "from", "into", "because", "if", "when", "while",
    "my", "your", "our", "their", "his", "its", "than", "very"
}
# Words that start the rest of a sentence and hardly ever a new one.
CONTINUES = {"that", "which", "whom", "whose", "than", "whereas"}
MARKS = re.compile(r"^(question mark|exclamation (point|mark)|period|full stop|comma|colon|semicolon|dot)\W*$", re.I)
COMMANDS = re.compile(r"^(scratch|new (line|paragraph|item)|bear\b|bare\b)", re.I)
THRESHOLDS = (0.3, 0.5, 0.7, 0.9)


def word(text: str, at: int) -> str:
    words = text.split()
    return re.sub(r"[^\w']", "", words[at].lower()) if words else ""


def last_word(a: str) -> str:
    """A's last word, past the mark the listener closed it with."""
    words = re.sub(r"[.?]\s*$", "", a.rstrip()).split()
    return words[-1] if words else ""


def a_dangles(b: dict, words: set[str]) -> bool:
    last = last_word(b["a_text"])
    return last.endswith(",") or re.sub(r"[^\w']", "", last.lower()) in words


def b_continues(b: dict) -> bool:
    return word(b["b_text"], 0) in CONTINUES


def tidy_keeps_end(b: dict, output: str) -> bool | None:
    """Whether the model's cleanup of A+B put a sentence end at the join:
    True (it did), False (it read on), None (B's start can't be found)."""
    first = [re.sub(r"[^\w']", "", w.lower()) for w in b["b_text"].split()[:2]]
    out = output.split()
    norm = [re.sub(r"[^\w']", "", w.lower()) for w in out]
    a_len = len(re.sub(r"[.?]\s*$", "", b["a_text"].rstrip()).split())
    best = None
    for i in range(1, len(norm)):
        if norm[i:i + len(first)] == first and (best is None or abs(i - a_len) < abs(best - a_len)):
            best = i
    if best is None:
        return None
    return out[best - 1].rstrip("\"'”’)").endswith((".", "!", "?", ":", "…")) or out[best].startswith("\n")


def scored(bounds, labels):
    """The closed pauses to score, and the ones left out, by why."""
    keep, said_mark, command = [], 0, 0
    for b in bounds:
        if not b["closed"] or labels.get(b["id"]) not in ("mid", "end"):
            continue
        if MARKS.match(b["b_text"].strip()):
            said_mark += 1
        elif COMMANDS.match(b["b_text"].strip()):
            command += 1
        else:
            keep.append(b)
    return keep, said_mark, command


def tally(rule, subset, labels) -> dict:
    mid = [b for b in subset if labels[b["id"]] == "mid"]
    end = [b for b in subset if labels[b["id"]] == "end"]
    joined = [b["id"] for b in mid if rule(b)]
    wrong = [b["id"] for b in end if rule(b)]
    return {"mid": len(mid), "rejoined": len(joined), "rejoined_ids": joined, "end": len(end), "wrongly_joined": len(wrong),
            "wrongly_joined_ids": wrong}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True)
    parser.add_argument("--answers", nargs="*", default=[])
    parser.add_argument("--json", help="write the results here (ids only, no text)")
    args = parser.parse_args()
    data = segments.data_dir(args.data)
    bounds = [json.loads(line) for line in open(data / "boundaries.jsonl", encoding="utf-8")]
    labels = {r["id"]: r["label"] for r in map(json.loads, open(data / "labels.jsonl", encoding="utf-8"))}
    answers = {name: {r["id"]: r for r in map(json.loads, open(data / f"answers-{name}.jsonl", encoding="utf-8"))}
               for name in args.answers}

    text_full = lambda b: a_dangles(b, segments.UNFINISHED) or b_continues(b)
    text = lambda b: a_dangles(b, DANGLING) or b_continues(b)
    rules = {
        "A ends on a listener UNFINISHED word": lambda b: a_dangles(b, segments.UNFINISHED),
        "A ends on a dangling word": lambda b: a_dangles(b, DANGLING),
        "B starts with a continuing word": b_continues,
        "text rule (UNFINISHED)": text_full,
        "text rule (dangling)": text,
    }
    for name, got in answers.items():
        if any("p_yes" in r for r in got.values()):
            for t in THRESHOLDS:
                model = lambda b, got=got, t=t: got[b["id"]]["p_yes"] >= t
                rules[f"{name} >= {t}"] = model
                rules[f"text rule + {name} >= {t}"] = lambda b, model=model: text(b) or model(b)
        else:
            reads_on = lambda b, got=got: tidy_keeps_end(b, got[b["id"]]["output"]) is False
            rules[f"{name} reads on"] = reads_on
            rules[f"text rule + {name} reads on"] = lambda b, reads_on=reads_on: text(b) or reads_on(b)

    results = {}
    for mode, name in ((False, "normal"), (True, "list")):
        subset, said_mark, command = scored([b for b in bounds if b["list_mode"] == mode], labels)
        table = {r: tally(fn, subset, labels) for r, fn in rules.items()}
        results[name] = {"closed_scored": len(subset), "said_a_mark": said_mark, "command": command, "rules": table}
        mid = table["text rule (dangling)"]["mid"]
        end = table["text rule (dangling)"]["end"]
        print(f"\n## {name}: closed pauses {mid} mid, {end} end (left out: {said_mark} said a mark, {command} a command)")
        print(f"{'rule':<44} {'rejoined':>9} {'wrongly joined':>15}")
        for rule, r in table.items():
            print(f"{rule:<44} {r['rejoined']:>4}/{r['mid']:<4} {r['wrongly_joined']:>9}/{r['end']}")
        if mode:
            went_on = [b for b in bounds if b["list_mode"] and not b["closed"] and labels.get(b["id"]) in ("mid", "end")]
            unmarked = lambda b: not b["a_text"].rstrip().endswith((".", "?", "!"))
            results[name]["went_on_in_time"] = {"join all": tally(lambda b: True, went_on, labels),
                                                "join unless marked": tally(unmarked, went_on, labels)}
            for rule, r in results[name]["went_on_in_time"].items():
                print(f"went on in time, {rule:<27} {r['rejoined']:>4}/{r['mid']:<4} {r['wrongly_joined']:>9}/{r['end']}")
        for answer, got in answers.items():
            if all("output" in r for r in got.values()):
                lost = sum(1 for b in subset if tidy_keeps_end(b, got[b["id"]]["output"]) is None)
                print(f"{answer}: B's start not found in {lost} of {len(subset)} outputs (counted as not rejoined)")
    if args.json:
        with open(args.json, "w", encoding="utf-8") as out:
            json.dump(results, out, indent=1)


if __name__ == "__main__":
    main()
