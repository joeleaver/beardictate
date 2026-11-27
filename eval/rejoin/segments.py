"""Reads a telemetry export and picks the runs of pauses the rejoin test scores.

Every script here reads a tester's words, so everything they write goes to a
data directory outside the repository (`--data`), never into it.

A run is consecutive segments of one session, in one mode, each said within
GAP_CAP_MS of the one before. The pick (sessions, counts, seed 7) is fixed so
the labelled set can be rebuilt from the export: 110 runs, 373 pauses on
2026-11-26's export of the Windows tester.
"""

from __future__ import annotations

import dataclasses
import json
import random
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
# The segmenter keeps this much audio on each side of the detected speech
# (`SegmenterConfig::padding`, crates/bd-asr/src/segmenter.rs).
PADDING_S = 0.2

# Longer pauses aren't taken: the listener has closed the sentence long
# before, and the window for reopening it is 10 s.
GAP_CAP_MS = 10_000
# Longest run taken.
MAX_RUN = 12
# Sessions sampled for normal dictation, and how many pauses from each.
NORMAL_SESSIONS = {
    "8731bdaf": 60,  # 11/12
    "2e98972e": 50,  # 11/06
    "fb1e2f3e": 50,  # 11/21
    "9913149c": 50,  # 11/26
    "8f495fd2": 40,  # 10/21
    "56b2be93": 40,  # 11/22
}
# Every list-mode run is taken, up to this many pauses.
LIST_LIMIT = 200

# The listener's "words a sentence doesn't end on" (UNFINISHED in
# crates/bd-pipeline/src/stages/listen.rs): after one it waits 4 s, not 1.5 s.
UNFINISHED = {
    "a", "an", "the", "and", "or", "but", "nor", "to", "of", "with", "for", "in", "on", "at", "from", "by", "into",
    "about", "is", "are", "was", "were", "be", "because", "if", "when", "while", "my", "your", "our", "their", "his",
    "its", "i", "we", "they", "you", "he", "she", "than", "as", "like", "very", "really"
}


@dataclasses.dataclass
class Seg:
    """One dictated segment, as the export records it."""

    session: str
    segment: int
    text: str
    pause_ms: int | None
    far: bool
    has_audio: bool
    list_mode: bool

    @property
    def sid(self) -> str:
        """A short id for reports: session prefix and segment number."""
        return f"{self.session[:8]}:{self.segment}"


def data_dir(path: str) -> Path:
    """Returns the data directory, refusing one inside the repository."""
    out = Path(path).resolve()
    if out == REPO or REPO in out.parents:
        raise SystemExit(f"--data {out} is inside the repository; testers' data stays outside it")
    out.mkdir(parents=True, exist_ok=True)
    return out


def load_segments(export: Path) -> dict[str, list[Seg]]:
    """Reads an export: each session's transcribed segments, in order."""
    sessions: dict[str, list[Seg]] = defaultdict(list)
    with open(export, encoding="utf-8") as lines:
        for line in lines:
            row = json.loads(line)
            if row.get("segment") is None:
                continue
            heard = [e for e in row["events"] if e["kind"] == "transcribed"]
            if not heard:
                continue
            data = heard[0]["data"]
            settings = row.get("settings") or {}
            sessions[row["session"]].append(
                Seg(session=row["session"],
                    segment=row["segment"],
                    text=data.get("text") or "",
                    pause_ms=data.get("pause_ms"),
                    far=bool(data.get("far")),
                    has_audio=bool(row.get("audio") and data.get("audio")),
                    list_mode=bool(settings.get("list_mode"))))
    for segs in sessions.values():
        segs.sort(key=lambda s: s.segment)
    return sessions


def usable(seg: Seg) -> bool:
    # Segments without audio are left out too, as the set was first drawn
    # for a test that also played them back; keeping that rebuilds it as labelled.
    return seg.has_audio and not seg.far and bool(seg.text.strip())


def runs(segs: list[Seg]) -> list[list[Seg]]:
    """Splits a session into runs of consecutive usable segments in one mode."""
    out: list[list[Seg]] = []
    cur: list[Seg] = []
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


def pick(sessions: dict[str, list[Seg]], seed: int) -> list[list[Seg]]:
    """The runs scored: NORMAL_SESSIONS' share of normal dictation, and the
    list-mode runs of every session, shuffled with `seed`."""
    rng = random.Random(seed)
    chosen: list[list[Seg]] = []
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
