"""Runs SAVA, streaming, over each stream build_streams.py made.

The stream is pushed through a `sava.Session` in pieces of `--chunk`
seconds, carrying the streaming state, never cut at a segment's end. 40 ms
is what a microphone would deliver; larger pieces give the same outputs
(`--check` compares the two) and run far faster. At the end of every
segment that has a boundary after it, the session is copied and the copy is
fed TAIL_S of the same room noise the gap holds, so each boundary gets its
`turn` curve for a pause of any length, not just the real one: the gap is a
prefix of the tail (same seed, same level), so up to the real gap the copy
hears exactly what the stream does.

Writes `sava.jsonl` to the data directory: per boundary, the frames from
the segment's speech end (its audio end less the segmenter's padding), each
`[seconds since speech end, turn, speech]`. The streaming cost per frame
is measured by export_sava_onnx.py, which steps one frame at a time.

Usage: run_sava.py --data DIR [--threads 1] [--jobs 4] [--chunk 1.0] [--check]
"""

from __future__ import annotations

import argparse
import copy
import json
import multiprocessing

import numpy as np
import torch
from inworld import sava

import build_streams
import common

FRAME = 640  # one 40 ms frame
_MODEL = None


def snapshot(session: sava.Session) -> sava.Session:
    """A copy of a session that can run on without touching the original."""
    twin = copy.copy(session)
    twin._state = copy.deepcopy(session._state)  # pylint: disable=protected-access
    twin._pending = session._pending.copy()  # pylint: disable=protected-access
    return twin


def frames_from(first: int, out, end_sample: int) -> list[list[float]]:
    """A push's frames as [seconds after speech end, turn, speech].

    Frame k's turn is known once the frame is in, at the end of frame k. The
    speech value at step k judges frame k - lag; it is kept as emitted.
    """
    speech_end = end_sample / common.SAMPLE_RATE - common.PADDING_S
    rows = []
    for i in range(out.turn.size):
        known_at = (first + i + 1) * FRAME / common.SAMPLE_RATE
        rows.append([round(known_at - speech_end, 3), float(out.turn[i]), float(out.speech[i])])
    return rows


def push_until(session: sava.Session, audio: np.ndarray, pos: int, stop: int, chunk: int):
    """Pushes audio[pos:stop] in pieces; returns each push's frames."""
    outs = []
    while pos < stop:
        piece = audio[pos:min(pos + chunk, stop)]
        outs.append(session.push(piece))
        pos += piece.size
    return outs


def run_stream(model, stream: dict, audio: np.ndarray, tail_s: float, chunk: int) -> dict[int, list]:
    session = sava.Session(model)
    segs = stream["segments"]
    curves: dict[int, list] = {}
    pos = 0
    for i, seg in enumerate(segs):
        outs = push_until(session, audio, pos, seg["end"], chunk)
        pos = seg["end"]
        if i == len(segs) - 1:
            break
        # Frames inside the segment's trailing padding come after the
        # speech ended: they count.
        rows = [r for out in outs if out.turn.size for r in frames_from(out.first, out, seg["end"]) if r[0] > 0]
        twin = snapshot(session)
        out = twin.push(build_streams.filler(seg["floor"], tail_s, seg["tail_seed"]))
        curves[i] = rows + frames_from(out.first, out, seg["end"])
        push_until(session, audio, pos, segs[i + 1]["start"], chunk)
        pos = segs[i + 1]["start"]
    return curves


def check_gap_is_tail_prefix(data, streams: list[dict]) -> None:
    """Every rebuilt gap must be the start of the tail SAVA is shown."""
    for stream in streams:
        segs = stream["segments"]
        audio = np.load(data / "streams" / f"{stream['stream']}.npy")
        for a, b in zip(segs, segs[1:]):
            gap = audio[a["end"]:b["start"]]
            tail = build_streams.filler(a["floor"], build_streams.TAIL_S, a["tail_seed"])
            if not np.array_equal(gap, tail[:gap.size]):
                raise SystemExit(f"{stream['stream']}: a gap isn't a prefix of its tail")


def _worker_init(threads: int) -> None:
    global _MODEL  # pylint: disable=global-statement
    torch.set_num_threads(threads)
    _MODEL = sava.load("inworld/sava")  # checks the registry's sha256


def _run(job: tuple[str, dict, int]) -> tuple[str, dict]:
    data, stream, chunk = job
    audio = np.load(f"{data}/streams/{stream['stream']}.npy")
    return stream["stream"], run_stream(_MODEL, stream, audio, build_streams.TAIL_S, chunk)


def check_chunking(data, stream: dict, chunk: int) -> float:
    """The largest difference in any frame's probabilities, 40 ms against `chunk` pushes."""
    audio = np.load(data / "streams" / f"{stream['stream']}.npy")
    small = run_stream(_MODEL, stream, audio, build_streams.TAIL_S, FRAME)
    large = run_stream(_MODEL, stream, audio, build_streams.TAIL_S, chunk)
    worst = 0.0
    for i, rows in small.items():
        a, b = np.array(rows), np.array(large[i])
        if a.shape != b.shape or np.abs(a[:, 0] - b[:, 0]).max() > 1e-9:
            raise SystemExit(f"{stream['stream']}: frame times differ between chunk sizes")
        worst = max(worst, float(np.abs(a[:, 1:] - b[:, 1:]).max()))
    return worst


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True)
    parser.add_argument("--threads", type=int, default=1, help="torch threads per job")
    parser.add_argument("--jobs", type=int, default=4, help="streams run at once")
    parser.add_argument("--chunk", type=float, default=1.0, help="seconds per push, rounded to whole frames")
    parser.add_argument("--check", action="store_true", help="first compare 40 ms pushes with --chunk on two streams")
    args = parser.parse_args()
    data = common.data_dir(args.data)
    chunk = max(1, round(args.chunk / 0.04)) * FRAME
    streams = [json.loads(line) for line in open(data / "streams.jsonl", encoding="utf-8")]
    check_gap_is_tail_prefix(data, streams)
    if args.check:
        _worker_init(args.threads)
        worst = max(check_chunking(data, s, chunk) for s in streams[:2])
        print(json.dumps({"max_chunking_diff": worst}), flush=True)
    jobs = [(str(data), s, chunk) for s in streams]
    with multiprocessing.Pool(args.jobs, _worker_init, (args.threads,)) as pool, \
            open(data / "sava.jsonl", "w", encoding="utf-8") as out:
        for stream_id, curves in pool.imap(_run, jobs):
            for i, rows in curves.items():
                out.write(json.dumps({"id": f"{stream_id}.{i}", "frames": rows}) + "\n")
    print(json.dumps({"streams": len(streams), "chunk_s": chunk / common.SAMPLE_RATE}))


if __name__ == "__main__":
    main()
