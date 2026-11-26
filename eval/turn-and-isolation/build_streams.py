"""Builds the turn-taking test set: streams of consecutive segments, rejoined.

Picks runs of consecutive segments from one session, fetches their audio,
and joins it with each real gap (the next segment's `pause_ms`) filled with
room noise at the segment's own noise floor. Each boundary between two
segments is a pause the app's VAD ended an utterance on.

Writes to the data directory:
  streams/<id>.npy        each stream's 16 kHz audio
  streams.jsonl           each stream's segments and where they lie in it
  boundaries.jsonl        each boundary: ids, gap, mode, and the texts on
                          either side (for labelling; testers' words)

Usage: build_streams.py --export will.jsonl --data DIR [--seed 7]
"""

from __future__ import annotations

import argparse
import json
import random
import re
import zlib

import numpy as np

import common

# Gaps above this aren't rebuilt: every rule tested ends the utterance long
# before, so they say nothing about the rules and only cost compute.
GAP_CAP_MS = 10_000
# Longest run joined into one stream.
MAX_RUN = 12
# Sessions sampled for normal dictation, and how many boundaries from each.
NORMAL_SESSIONS = {
    "8731bdaf": 60,  # 11/12
    "2e98972e": 50,  # 11/06
    "fb1e2f3e": 50,  # 11/21
    "9913149c": 50,  # 11/26
    "8f495fd2": 40,  # 10/21
    "56b2be93": 40,  # 11/22
}
# Every list-mode run is taken, up to this many boundaries.
LIST_LIMIT = 200
# Tail filler appended after each segment's end for SAVA (run_sava.py):
# the gap's own filler is a prefix of it, so both see the same noise.
TAIL_S = 10.5  # longer than any rebuilt gap (GAP_CAP_MS)

# The listener's "words a sentence doesn't end on" (UNFINISHED in
# crates/bd-pipeline/src/stages/listen.rs): after one it waits 4 s, not 1.5 s.
UNFINISHED = {
    "a", "an", "the", "and", "or", "but", "nor", "to", "of", "with", "for", "in", "on", "at", "from", "by", "into",
    "about", "is", "are", "was", "were", "be", "because", "if", "when", "while", "my", "your", "our", "their", "his",
    "its", "i", "we", "they", "you", "he", "she", "than", "as", "like", "very", "really"
}


def ends_unfinished(text: str) -> bool:
    """Whether the listener would wait 4 s after this text (listen.rs `unfinished`)."""
    words = text.split()
    if not words:
        return False
    if words[-1].endswith(","):
        return True
    if words[-1].endswith((".", "!", "?")):
        return False
    return re.sub(r"[^\w']", "", words[-1].lower()) in UNFINISHED


def usable(seg: common.Seg) -> bool:
    return bool(seg.audio_key) and not seg.far and bool(seg.text.strip())


def runs(segs: list[common.Seg]) -> list[list[common.Seg]]:
    """Splits a session into runs of consecutive usable segments in one mode."""
    out: list[list[common.Seg]] = []
    cur: list[common.Seg] = []
    for seg in segs:
        joins = (cur and usable(seg) and seg.segment == cur[-1].segment + 1 and seg.pause_ms is not None and
                 seg.pause_ms <= GAP_CAP_MS and seg.list_mode == cur[-1].list_mode and len(cur) < MAX_RUN)
        if joins:
            cur.append(seg)
            continue
        if len(cur) > 1:
            out.append(cur)
        cur = [seg] if usable(seg) else []
    if len(cur) > 1:
        out.append(cur)
    return out


def pick(sessions: dict[str, list[common.Seg]], seed: int) -> list[list[common.Seg]]:
    rng = random.Random(seed)
    chosen: list[list[common.Seg]] = []
    for prefix, want in NORMAL_SESSIONS.items():
        session = next(s for s in sessions if s.startswith(prefix))
        candidates = [r for r in runs(sessions[session]) if not r[0].list_mode]
        rng.shuffle(candidates)
        have = 0
        for run in candidates:
            if have >= want:
                break
            chosen.append(run)
            have += len(run) - 1
    listed = [r for segs in sessions.values() for r in runs(segs) if r[0].list_mode]
    rng.shuffle(listed)
    have = 0
    for run in listed:
        if have >= LIST_LIMIT:
            break
        chosen.append(run)
        have += len(run) - 1
    return chosen


def seed_for(stream_id: str, index: int) -> int:
    """The noise seed after segment `index` of a stream: the gap and the tail share it."""
    return zlib.crc32(f"{stream_id}/{index}".encode())


def filler(rms: float, seconds: float, seed: int) -> np.ndarray:
    """Room noise: white noise at the given RMS, the same for the same seed."""
    rng = np.random.default_rng(seed)
    return (rng.standard_normal(int(seconds * common.SAMPLE_RATE)) * rms).astype(np.float32)


def build(run: list[common.Seg], index: int, data, out_streams, out_bounds) -> None:
    stream_id = f"s{index:03d}"
    pieces: list[np.ndarray] = []
    at = 0
    placed = []
    for i, seg in enumerate(run):
        audio = common.read_audio(common.fetch_audio(seg, data))
        if i > 0:
            gap = filler(placed[-1]["floor"], seg.pause_ms / 1000, seed=seed_for(stream_id, i - 1))
            pieces.append(gap)
            at += gap.size
        placed.append({
            "sid": seg.sid,
            "start": at,
            "end": at + audio.size,
            "floor": common.noise_floor(audio),
            "tail_seed": seed_for(stream_id, i),
        })
        pieces.append(audio)
        at += audio.size
    np.save(data / "streams" / f"{stream_id}.npy", np.concatenate(pieces))
    out_streams.write(json.dumps({"stream": stream_id, "segments": placed}) + "\n")
    for i in range(len(run) - 1):
        a, b = run[i], run[i + 1]
        out_bounds.write(
            json.dumps({
                "id": f"{stream_id}.{i}",
                "stream": stream_id,
                "index": i,
                "a": a.sid,
                "b": b.sid,
                "pause_ms": b.pause_ms,
                "list_mode": a.list_mode,
                "unfinished_word": ends_unfinished(a.text),
                "a_closed": a.text.rstrip().endswith((".", "!", "?", "…")),
                "cleaned_together": b.segment in a.cleaned_with,
                "before": run[i - 1].text if i > 0 else "",
                "a_text": a.text,
                "b_text": b.text,
                "after": run[i + 2].text if i + 2 < len(run) else "",
            }) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--export", required=True)
    parser.add_argument("--data", required=True)
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()
    data = common.data_dir(args.data)
    (data / "streams").mkdir(exist_ok=True)
    chosen = pick(common.load_segments(args.export), args.seed)
    with open(data / "streams.jsonl", "w", encoding="utf-8") as streams, \
            open(data / "boundaries.jsonl", "w", encoding="utf-8") as bounds:
        for index, run in enumerate(chosen):
            build(run, index, data, streams, bounds)
    n = sum(len(r) - 1 for r in chosen)
    print(f"{len(chosen)} streams, {n} boundaries "
          f"({sum(len(r) - 1 for r in chosen if r[0].list_mode)} in list mode)")


if __name__ == "__main__":
    main()
