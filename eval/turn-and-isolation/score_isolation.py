"""Scores isolate.py's results: per group, before and after the isolator.

Per group: the longest word repeat, the segments with a runaway (a word 5+
times in a row), the words heard, the level change, and how far the two
transcripts differ (word edit distance over the original's words). Against
a reference, the cleanup's output where it cleaned the segment alone, else
the app's own transcript: each transcript's word error rate.

Prints a table and, with --ids, the per-segment numbers (ids only).

Usage: score_isolation.py --data DIR [--ids] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict

import common
from isolate import longest_repeat, words


def edits(a: list[str], b: list[str]) -> int:
    """Word-level Levenshtein distance."""
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True)
    parser.add_argument("--ids", action="store_true")
    parser.add_argument("--json")
    args = parser.parse_args()
    data = common.data_dir(args.data)
    rows = [json.loads(line) for line in open(data / "isolation_results.jsonl", encoding="utf-8")]
    groups = defaultdict(list)
    per_segment = []
    for r in rows:
        o, i = words(r["orig_text"]), words(r["iso_text"])
        ref = words(r["cleaned"] or r["app_text"])
        seg = {
            "sid": r["sid"],
            "group": r["group"],
            "app_repeat": longest_repeat(r["app_text"]),
            "orig_repeat": longest_repeat(r["orig_text"]),
            "iso_repeat": longest_repeat(r["iso_text"]),
            "orig_words": len(o),
            "iso_words": len(i),
            "db_change": round(r["iso_dbfs"] - r["orig_dbfs"], 1),
            "iso_vs_orig": edits(o, i) / max(len(o), 1),
            "orig_wer": edits(ref, o) / max(len(ref), 1),
            "iso_wer": edits(ref, i) / max(len(ref), 1),
            "ref_words": len(ref),
        }
        per_segment.append(seg)
        groups[r["group"]].append(seg)

    def mean(xs):
        return sum(xs) / len(xs) if xs else 0.0

    summary = {}
    print(f"{'group':<8} {'n':>3} {'runaway o→i':>12} {'max rep o→i':>12} {'words o→i':>11} {'dB':>6} "
          f"{'diff':>5} {'WER o→i':>11}")
    for g, segs in groups.items():
        s = {
            "n": len(segs),
            "runaway_app": sum(x["app_repeat"] >= 5 for x in segs),
            "runaway_orig": sum(x["orig_repeat"] >= 5 for x in segs),
            "runaway_iso": sum(x["iso_repeat"] >= 5 for x in segs),
            "max_repeat_orig": max(x["orig_repeat"] for x in segs),
            "max_repeat_iso": max(x["iso_repeat"] for x in segs),
            "words_orig": sum(x["orig_words"] for x in segs),
            "words_iso": sum(x["iso_words"] for x in segs),
            "db_change_mean": round(mean([x["db_change"] for x in segs]), 1),
            "iso_vs_orig_mean": round(mean([x["iso_vs_orig"] for x in segs]), 3),
            "orig_wer_mean": round(mean([x["orig_wer"] for x in segs]), 3),
            "iso_wer_mean": round(mean([x["iso_wer"] for x in segs]), 3),
        }
        summary[g] = s
        print(f"{g:<8} {s['n']:>3} {s['runaway_orig']:>5}→{s['runaway_iso']:<6} "
              f"{s['max_repeat_orig']:>5}→{s['max_repeat_iso']:<6} {s['words_orig']:>5}→{s['words_iso']:<5} "
              f"{s['db_change_mean']:>6} {s['iso_vs_orig_mean']:>5} {s['orig_wer_mean']:>5}→{s['iso_wer_mean']:<5}")
    if args.ids:
        for x in per_segment:
            print(json.dumps(x))
    if args.json:
        with open(args.json, "w", encoding="utf-8") as out:
            json.dump({"summary": summary, "segments": per_segment}, out, indent=1)


if __name__ == "__main__":
    main()
