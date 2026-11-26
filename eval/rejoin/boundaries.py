"""Builds the rejoin test set: the pauses between a tester's segments, as text.

Takes the runs of consecutive segments `segments.pick` chooses (fixed
sessions, seed 7) and writes one line per pause between two segments with the
text on either side and whether today's listener closed the sentence there
with a full stop of its own (`closed`: it waited out its deadline before the
speaker went on).

Writes to the data directory (testers' words: keep it outside the repo):
  boundaries.jsonl   id, a, b, pause_ms, list_mode, closed, unfinished_word,
                     before, a_text, b_text, after

Usage: boundaries.py --export will.jsonl --data DIR [--seed 7]
"""

from __future__ import annotations

import argparse
import json
import re

import segments  # beside this script

# The listener (crates/bd-pipeline/src/stages/listen.rs): the VAD stops 0.6 s
# into a pause, then the stream waits 1.5 s more, 4 s after a word a sentence
# doesn't end on or a comma.
VAD_STOP, WAIT, WAIT_UNFINISHED = 0.6, 1.5, 4.0


def unfinished(text: str) -> bool:
    """The listener's test on the last word, before it added its own mark
    (the export has the text after: a trailing . or ? may be ours)."""
    words = text.split()
    if not words:
        return False
    last = words[-1]
    if last.endswith(","):
        return True
    bare = re.sub(r"[^\w']", "", last.lower())
    return bare in segments.UNFINISHED and not last.endswith(("!", "…"))


def decided_at(a_text: str) -> float:
    """Seconds after the speech when the listener closes the sentence."""
    return VAD_STOP + (WAIT_UNFINISHED if unfinished(a_text) else WAIT)


def gap(pause_ms: int) -> float:
    """Seconds after the speech when the speaker went on (padding both sides)."""
    return pause_ms / 1000 + 2 * segments.PADDING_S


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--export", required=True)
    parser.add_argument("--data", required=True)
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()
    data = segments.data_dir(args.data)
    chosen = segments.pick(segments.load_segments(args.export), args.seed)
    n = 0
    with open(data / "boundaries.jsonl", "w", encoding="utf-8") as out:
        for index, run in enumerate(chosen):
            for i in range(len(run) - 1):
                a, b = run[i], run[i + 1]
                out.write(
                    json.dumps({
                        "id": f"s{index:03d}.{i}",
                        "a": a.sid,
                        "b": b.sid,
                        "pause_ms": b.pause_ms,
                        "list_mode": a.list_mode,
                        "closed": decided_at(a.text) < gap(b.pause_ms),
                        "unfinished_word": unfinished(a.text),
                        "before": run[i - 1].text if i > 0 else "",
                        "a_text": a.text,
                        "b_text": b.text,
                        "after": run[i + 2].text if i + 2 < len(run) else "",
                    }) + "\n")
                n += 1
    print(f"{len(chosen)} runs, {n} boundaries")


if __name__ == "__main__":
    main()
