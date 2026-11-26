"""Voice Isolator test: clean chosen segments, transcribe before and after.

  select     picks the segments (runaway repeats, the 11/25 supper
             conversation, quiet, noisy and a control set) into
             `isolation_set.jsonl`
  isolate    runs the realtime checkpoint at 48 kHz (resampled in from
             16 kHz and back out) with `isolate()`, which runs the live
             stream over the clip ("exactly what a live session gives"),
             and writes `iso/<sid>.orig.wav` and `iso/<sid>.iso.wav`
  cost       times a true live session, 20 ms pushes, on the first
             --seconds of one segment: the realtime cost per audio second
             and each push's latency, on --device cpu or cuda
  transcribe runs our recognizer on both with `bd file` (the release
             build, cleanup off, the tester's own word list and hotword
             boost) into `isolation_results.jsonl`

Usage: isolate.py select --export will.jsonl --data DIR
       isolate.py isolate --data DIR [--threads 1]
       isolate.py cost --data DIR [--threads 1] [--seconds 3] [--device cpu]
       isolate.py transcribe --export will.jsonl --data DIR [--bd target/release/bd]
"""

from __future__ import annotations

import argparse
import json
import random
import re
import subprocess
import time

import numpy as np
import soundfile
import soxr

import common

SUPPER = ("3890918c", range(635, 648))
CHUNK_48K = 960  # 20 ms


def words(text: str) -> list[str]:
    return [w for w in (re.sub(r"[^\w']", "", x.lower()) for x in text.split()) if w]


def longest_repeat(text: str) -> int:
    """The longest run of one word said again and again."""
    best = run = 0
    prev = None
    for w in words(text):
        run = run + 1 if w == prev else 1
        prev = w
        best = max(best, run)
    return best


def dbfs(audio: np.ndarray) -> float:
    return float(20 * np.log10(max(np.sqrt(np.mean(audio**2)), 1e-9)))


def select(args) -> None:
    data = common.data_dir(args.data)
    sessions = common.load_segments(args.export)
    every = [s for segs in sessions.values() for s in segs if s.audio_key and not s.far]
    rng = random.Random(11)
    chosen: dict[str, str] = {}
    repeats = sorted((s for s in every if longest_repeat(s.text) >= 5), key=lambda s: -longest_repeat(s.text))
    for s in repeats[:16]:
        chosen.setdefault(s.sid, "repeat")
    for s in every:
        if s.session.startswith(SUPPER[0]) and s.segment in SUPPER[1]:
            chosen.setdefault(s.sid, "supper")
    quiet = [s for s in every if s.level_dbfs < -48 and s.audio_ms > 1500 and s.sid not in chosen]
    for s in rng.sample(quiet, min(8, len(quiet))):
        chosen.setdefault(s.sid, "quiet")
    # Noisy: the highest noise floors (quietest 10% of 20 ms windows) of
    # 200 random segments over 1.5 s, a stand-in for a busy room.
    loud_floor = []
    pool = rng.sample([s for s in every if s.sid not in chosen and s.audio_ms > 1500], 200)
    for s in pool:
        floor = common.noise_floor(common.read_audio(common.fetch_audio(s, data)))
        loud_floor.append((20 * np.log10(floor), s))
    loud_floor.sort(key=lambda x: -x[0])
    for _, s in loud_floor[:8]:
        chosen.setdefault(s.sid, "noisy")
    control = [s for _, s in loud_floor[len(loud_floor) // 4:] if s.sid not in chosen and -40 < s.level_dbfs < -20]
    for s in rng.sample(control, min(20, len(control))):
        chosen.setdefault(s.sid, "control")
    by_sid = {s.sid: s for s in every}
    with open(data / "isolation_set.jsonl", "w", encoding="utf-8") as out:
        for sid, group in chosen.items():
            s = by_sid[sid]
            common.fetch_audio(s, data)
            out.write(json.dumps({"sid": sid, "group": group, "session": s.session, "segment": s.segment}) + "\n")
    counts: dict[str, int] = {}
    for g in chosen.values():
        counts[g] = counts.get(g, 0) + 1
    print(json.dumps(counts))


def isolate(args) -> None:
    import torch  # pylint: disable=import-outside-toplevel
    from inworld import voice_isolation  # pylint: disable=import-outside-toplevel

    data = common.data_dir(args.data)
    torch.set_num_threads(args.threads)
    live = voice_isolation.VoiceIsolator("inworld/voice-isolator", mode="streaming", lookahead_frames=2, device="cpu")
    (data / "iso").mkdir(exist_ok=True)
    total_cpu = total_wall = total_audio = 0.0
    for row in map(json.loads, open(data / "isolation_set.jsonl", encoding="utf-8")):
        audio = common.read_audio(data / "audio" / f"{row['session']}-{row['segment']}.flac")
        cpu0, wall0 = time.process_time(), time.perf_counter()
        up = soxr.resample(audio, common.SAMPLE_RATE, 48_000).astype(np.float32)
        voice = np.asarray(live.isolate(torch.from_numpy(up), 48_000), dtype=np.float32).reshape(-1)
        down = soxr.resample(voice, 48_000, common.SAMPLE_RATE).astype(np.float32)[:audio.size]
        total_cpu += time.process_time() - cpu0
        total_wall += time.perf_counter() - wall0
        total_audio += audio.size / common.SAMPLE_RATE
        name = row["sid"].replace(":", "-")
        soundfile.write(data / "iso" / f"{name}.orig.wav", audio, common.SAMPLE_RATE, subtype="FLOAT")
        soundfile.write(data / "iso" / f"{name}.iso.wav", down, common.SAMPLE_RATE, subtype="FLOAT")
    print(
        json.dumps({
            "threads": args.threads,
            "audio_s": round(total_audio, 1),
            "cpu_per_audio_s": round(total_cpu / total_audio, 3),
            "wall_per_audio_s": round(total_wall / total_audio, 3),
        }))


def cost(args) -> None:
    import torch  # pylint: disable=import-outside-toplevel
    from inworld import voice_isolation  # pylint: disable=import-outside-toplevel

    data = common.data_dir(args.data)
    torch.set_num_threads(args.threads)
    live = voice_isolation.VoiceIsolator(
        "inworld/voice-isolator", mode="streaming", lookahead_frames=2, device=args.device)
    row = json.loads(open(data / "isolation_set.jsonl", encoding="utf-8").readline())
    audio = common.read_audio(data / "audio" / f"{row['session']}-{row['segment']}.flac")
    up = soxr.resample(audio, common.SAMPLE_RATE, 48_000).astype(np.float32)[:int(args.seconds * 48_000)]

    def sync() -> None:
        if args.device != "cpu":
            torch.cuda.synchronize()

    live.isolate(torch.from_numpy(up[:48_000]), 48_000)  # warm-up: kernels, allocator
    sync()
    session = live.stream()
    pieces, push_ms = [], []
    cpu0, wall0 = time.process_time(), time.perf_counter()
    for i in range(0, up.size, CHUNK_48K):
        t0 = time.perf_counter()
        pieces.append(session.push(up[i:i + CHUNK_48K]))
        sync()
        push_ms.append((time.perf_counter() - t0) * 1000)
    pieces.append(session.flush())
    sync()
    cpu, wall = time.process_time() - cpu0, time.perf_counter() - wall0
    streamed = np.concatenate([np.asarray(p, dtype=np.float32).reshape(-1) for p in pieces])
    whole = np.asarray(live.isolate(torch.from_numpy(up), 48_000), dtype=np.float32).reshape(-1)
    n = min(streamed.size, whole.size)
    audio_s = up.size / 48_000
    print(
        json.dumps({
            "device": args.device,
            "threads": args.threads,
            "audio_s": round(audio_s, 2),
            "cpu_per_audio_s": round(cpu / audio_s, 2),
            "wall_per_audio_s": round(wall / audio_s, 2),
            "push_ms_median_p95_max": [round(float(np.percentile(push_ms, q)), 1) for q in (50, 95, 100)],
            "max_diff_stream_vs_isolate": float(np.abs(streamed[:n] - whole[:n]).max()),
        }))


def words_file(export: str, out) -> float | None:
    """The tester's latest word list, in `bd --words` form."""
    groups = None
    for line in open(export, encoding="utf-8"):
        row = json.loads(line)
        if row.get("segment") is not None:
            continue
        for e in row["events"]:
            if e["kind"] == "settings" and e["data"].get("word_groups") is not None:
                if groups is None or e["ts"] > groups[0]:
                    groups = (e["ts"], e["data"]["word_groups"], e["data"].get("hotword_boost"))
    lines = []
    for group in groups[1]:
        if not group.get("enabled", True):
            continue
        for w in group.get("words", []):
            # Symbol shortcuts ("=====") are replacements the CLI's word
            # list can't hold; they don't steer the recognizer's names.
            if w.get("enabled", True) and re.search(r"\w", w.get("word") or ""):
                heard = [h for h in w.get("heard") or [] if h]
                lines.append(w["word"] + (": " + ", ".join(heard) if heard else ""))
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return groups[2]


def transcribe(args) -> None:
    data = common.data_dir(args.data)
    vocab = data / "words.txt"
    boost = words_file(args.export, vocab)
    sessions = common.load_segments(args.export)
    by_sid = {s.sid: s for segs in sessions.values() for s in segs}
    finals = cleaned_finals(args.export)
    with open(data / "isolation_results.jsonl", "w", encoding="utf-8") as out:
        for row in map(json.loads, open(data / "isolation_set.jsonl", encoding="utf-8")):
            name = row["sid"].replace(":", "-")
            texts = {}
            for kind in ("orig", "iso"):
                wav = data / "iso" / f"{name}.{kind}.wav"
                done = subprocess.run([
                    args.bd, "file",
                    str(wav), "--cleanup", "off", "--paragraphs", "off", "--words",
                    str(vocab), "--hotword-boost",
                    str(boost or 2.5)
                ],
                                      capture_output=True,
                                      text=True,
                                      check=True)
                texts[kind] = " ".join(done.stdout.split())
            audio = {k: soundfile.read(data / "iso" / f"{name}.{k}.wav", dtype="float32")[0] for k in ("orig", "iso")}
            seg = by_sid[row["sid"]]
            out.write(
                json.dumps({
                    **row,
                    "app_text": seg.text,
                    "cleaned": finals.get((seg.session, seg.segment)),
                    "orig_text": texts["orig"],
                    "iso_text": texts["iso"],
                    "orig_dbfs": dbfs(audio["orig"]),
                    "iso_dbfs": dbfs(audio["iso"]),
                }) + "\n")
    print(f"boost {boost}")


def cleaned_finals(export: str) -> dict:
    """Each segment's cleanup output, where it was cleaned alone."""
    out = {}
    for line in open(export, encoding="utf-8"):
        row = json.loads(line)
        if row.get("segment") is None:
            continue
        for e in row["events"]:
            if e["kind"] == "cleaned" and e["data"].get("segments") == [row["segment"]]:
                out[(row["session"], row["segment"])] = e["data"].get("final")
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("select")
    p.add_argument("--export", required=True)
    p.add_argument("--data", required=True)
    p = sub.add_parser("isolate")
    p.add_argument("--data", required=True)
    p.add_argument("--threads", type=int, default=1)
    p = sub.add_parser("cost")
    p.add_argument("--data", required=True)
    p.add_argument("--threads", type=int, default=1)
    p.add_argument("--seconds", type=float, default=3.0)
    p.add_argument("--device", default="cpu", help="cpu, or cuda (also ROCm builds of torch)")
    p = sub.add_parser("transcribe")
    p.add_argument("--export", required=True)
    p.add_argument("--data", required=True)
    p.add_argument("--bd", default=str(common.REPO / "target" / "release" / "bd"))
    args = parser.parse_args()
    {"select": select, "isolate": isolate, "cost": cost, "transcribe": transcribe}[args.cmd](args)


if __name__ == "__main__":
    main()
