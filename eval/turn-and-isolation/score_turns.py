"""Scores end-of-utterance rules on the labelled boundaries.

Each rule gives, for a boundary, the decision time D: how long after the
speech ended the app would decide the sentence (or, in list mode, the item)
is over. The speaker resumed after G (the gap: `pause_ms` plus the
segmenter's padding on both sides). The rule split the boundary when
D < G. Then:

  - at a `mid` boundary, a split is a false split: a full stop and a
    capital, or a new list item, in the middle of what was said;
  - at an `end` boundary, D is the wait the rule adds before the sentence
    is final. When G <= D the next sentence began first and the two ran on
    in one stream, punctuated by the recognizer (harmless: counted apart).

Rules:
  current     the app now (crates/bd-pipeline/src/stages/listen.rs): the
              VAD stops after 0.6 s of silence, then the stream waits 1.5 s
              for more (4 s after a word a sentence doesn't end on), so
              D = 2.1 s or 4.6 s. In list mode every utterance is an item:
              D = 0.6 s.
  sava t/M    SAVA replaces the wait: once the VAD has stopped (0.6 s), end
              at the first frame whose turn probability is above t, or at M
              seconds whatever it says.
  sava+w t/M  the same, keeping the 4.6 s wait after an unfinished word.
  wait W      no model, a flat wait: D = W (in either mode)
              (the baseline a SAVA rule must beat at the same latency).
  hold t      SAVA only lengthens the wait: at the current rule's decision
              time, end if turn > t; otherwise wait on until it is, at most
              4.6 s. (List mode: from 0.6 s.)

Usage: score_turns.py --data DIR [--json out.json]
"""

from __future__ import annotations

import argparse
import json

import numpy as np

import common

VAD_STOP = 0.6
WAIT, WAIT_UNFINISHED = 1.5, 4.0
THRESHOLDS = (0.5, 0.7, 0.8, 0.9, 0.95)
MAX_WAITS = (1.5, 2.1, 3.0, 4.6)
HOLD_MAX = VAD_STOP + WAIT_UNFINISHED
FLAT_WAITS = (0.9, 1.2, 1.5, 2.4, 2.6, 2.8, 3.0)


def gap_s(b: dict) -> float:
    return b["pause_ms"] / 1000 + 2 * common.PADDING_S


def current(b: dict, _frames) -> float:
    if b["list_mode"]:
        return VAD_STOP
    return VAD_STOP + (WAIT_UNFINISHED if b["unfinished_word"] else WAIT)


def sava_rule(t: float, max_wait: float, keep_unfinished: bool):

    def decide(b: dict, frames: np.ndarray) -> float:
        cap = max_wait
        if keep_unfinished and b["unfinished_word"] and not b["list_mode"]:
            cap = max(cap, VAD_STOP + WAIT_UNFINISHED)
        for at, turn, _speech in frames:
            if at < VAD_STOP:
                continue
            if at >= cap:
                break
            if turn > t:
                return float(at)
        return cap

    return decide


def hold_rule(t: float):

    def decide(b: dict, frames: np.ndarray) -> float:
        start = current(b, frames)
        for at, turn, _speech in frames:
            if at < start:
                continue
            if at >= HOLD_MAX:
                break
            if turn > t:
                return float(max(at, start))
        return max(HOLD_MAX, start)

    return decide


def score(rule, bounds: list[dict], labels: dict[str, str], curves: dict[str, np.ndarray]) -> dict:
    mid = [b for b in bounds if labels.get(b["id"]) == "mid"]
    end = [b for b in bounds if labels.get(b["id"]) == "end"]
    false_splits = [b["id"] for b in mid if rule(b, curves[b["id"]]) < gap_s(b)]
    waits = np.array([rule(b, curves[b["id"]]) for b in end])
    ran_on = sum(1 for b, d in zip(end, waits) if gap_s(b) <= d)
    return {
        "mid": len(mid),
        "false_splits": len(false_splits),
        "false_split_ids": false_splits,
        "end": len(end),
        "wait_median": round(float(np.median(waits)), 2) if waits.size else None,
        "wait_mean": round(float(waits.mean()), 2) if waits.size else None,
        "wait_p90": round(float(np.percentile(waits, 90)), 2) if waits.size else None,
        "ends_run_on": ran_on,
    }


def auc(pos: list[float], neg: list[float]) -> float | None:
    """P(a random `end` scores above a random `mid`): ties count half."""
    if not pos or not neg:
        return None
    p, n = np.array(pos)[:, None], np.array(neg)[None, :]
    return float(((p > n).sum() + 0.5 * (p == n).sum()) / (p.size * n.size))


def separability(bounds, labels, curves, at: float) -> dict:
    """How well `turn` at `at` seconds after the speech tells end from mid,
    over boundaries whose gap is longer than that (so it's still silence)."""
    out = {}
    for mode in (False, True):
        pos, neg = [], []
        for b in bounds:
            if b["list_mode"] != mode or gap_s(b) <= at:
                continue
            frames = curves[b["id"]]
            k = int(np.searchsorted(frames[:, 0], at))
            if k >= len(frames):
                continue
            (pos if labels.get(b["id"]) == "end" else neg if labels.get(b["id"]) == "mid" else []).append(frames[k, 1])
        out["list" if mode else "normal"] = {"end": len(pos), "mid": len(neg), "auc": auc(pos, neg)}
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True)
    parser.add_argument("--labels", default="labels.jsonl")
    parser.add_argument("--json", help="write the full results here (ids only, no text)")
    args = parser.parse_args()
    data = common.data_dir(args.data)
    bounds = [json.loads(line) for line in open(data / "boundaries.jsonl", encoding="utf-8")]
    labels = {r["id"]: r["label"] for r in map(json.loads, open(data / args.labels, encoding="utf-8"))}
    curves = {r["id"]: np.array(r["frames"]) for r in map(json.loads, open(data / "sava.jsonl", encoding="utf-8"))}
    bounds = [b for b in bounds if b["id"] in curves]

    rules = {"current": current}
    for t in THRESHOLDS:
        for m in MAX_WAITS:
            rules[f"sava {t}/{m}"] = sava_rule(t, m, keep_unfinished=False)
            rules[f"sava+w {t}/{m}"] = sava_rule(t, m, keep_unfinished=True)
    for t in THRESHOLDS:
        rules[f"hold {t}"] = hold_rule(t)
    for w in FLAT_WAITS:
        rules[f"wait {w}"] = lambda _b, _f, w=w: w
    results = {"labels": {}, "separability": {}, "rules": {}}
    for mode, name in ((False, "normal"), (True, "list")):
        subset = [b for b in bounds if b["list_mode"] == mode]
        results["labels"][name] = {k: sum(1 for b in subset if labels.get(b["id"]) == k) for k in ("mid", "end", "unsure")}
        results["rules"][name] = {r: score(fn, subset, labels, curves) for r, fn in rules.items()}
    for at in (0.8, 1.2, 2.0):
        results["separability"][str(at)] = separability(bounds, labels, curves, at)

    for name, table in results["rules"].items():
        print(f"\n## {name} ({results['labels'][name]})")
        print(f"{'rule':<20} {'false splits':>14} {'end wait med':>13} {'mean':>6} {'p90':>6} {'ends run on':>12}")
        for rule, r in table.items():
            if rule != "current" and not rule.startswith(("sava 0.", "sava+w 0.", "hold", "wait")):
                continue
            print(f"{rule:<20} {r['false_splits']:>6}/{r['mid']:<7} {r['wait_median']!s:>13} {r['wait_mean']!s:>6} "
                  f"{r['wait_p90']!s:>6} {r['ends_run_on']:>6}/{r['end']}")
    print("\nseparability (AUC of turn at t s, end vs mid):", json.dumps(results["separability"]))
    if args.json:
        with open(args.json, "w", encoding="utf-8") as out:
            json.dump(results, out, indent=1)


if __name__ == "__main__":
    main()
