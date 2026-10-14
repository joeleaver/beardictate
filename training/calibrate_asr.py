#!/usr/bin/env python3
"""Calibrate the Parakeet simulator against real Parakeet output.

Synthesizes eval sentences with a local TTS (sherpa-onnx + Piper voices),
transcribes them with Parakeet TDT 0.6B v3 int8 (sherpa-onnx), and compares
the real transcripts with (a) the clean text and (b) what simulate.py
produces for the same text. TTS speech is cleaner and more regular than a
person dictating, so this calibrates punctuation, casing and segmentation
habits, not disfluency rates.

Two variants per case:
  clean   the expected text with quote marks and breaks removed (fluent reading)
  disfl   the eval raw text (with "um", "uh", repeats), read by the TTS

    calibrate_asr.py --n 24 --out /mnt/data/beardictate-dev/calibration
"""

from __future__ import annotations

import argparse
import json
import random
import re
import sys
from collections import Counter
from pathlib import Path

import numpy as np
import sherpa_onnx

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "eval"))

from simulate import NoiseConfig, simulate  # noqa: E402
from textnorm import FILLERS, edit_distance, words  # noqa: E402

MODELS = Path("/mnt/data/beardictate-dev/models")
ASR = MODELS / "asr-parakeet-tdt-0.6b-v3-int8"
VOICES = ["vits-piper-en_US-lessac-medium", "vits-piper-en_US-ryan-medium"]
MARKS = "“”‘’"


def make_tts(voice: str) -> sherpa_onnx.OfflineTts:
    d = MODELS / voice
    onnx = next(d.glob("*.onnx"))
    cfg = sherpa_onnx.OfflineTtsConfig(
        model=sherpa_onnx.OfflineTtsModelConfig(
            vits=sherpa_onnx.OfflineTtsVitsModelConfig(model=str(onnx), tokens=str(d / "tokens.txt"),
                                                       data_dir=str(d / "espeak-ng-data")),
            num_threads=4,
        ),
    )
    return sherpa_onnx.OfflineTts(cfg)


def make_asr() -> sherpa_onnx.OfflineRecognizer:
    return sherpa_onnx.OfflineRecognizer.from_transducer(
        encoder=str(ASR / "encoder.int8.onnx"), decoder=str(ASR / "decoder.int8.onnx"),
        joiner=str(ASR / "joiner.int8.onnx"), tokens=str(ASR / "tokens.txt"),
        model_type="nemo_transducer", num_threads=4,
    )


def transcribe(asr: sherpa_onnx.OfflineRecognizer, samples: np.ndarray, rate: int) -> str:
    stream = asr.create_stream()
    pad = np.zeros(int(rate * 0.4), dtype=np.float32)
    stream.accept_waveform(rate, np.concatenate([pad, samples.astype(np.float32), pad]))
    asr.decode_stream(stream)
    return str(stream.result.text).strip()


def speakable(text: str) -> str:
    text = re.sub(f"[{MARKS}]", "", text).replace("\n", " ")
    return re.sub(r"\s+", " ", text).strip()


def style_stats(texts: list[str]) -> Counter[str]:
    c: Counter[str] = Counter()
    for t in texts:
        n = max(1, len(t.split()))
        c["words"] += n
        for mark, name in ((",", "comma"), (".", "period"), ("?", "question"), ("!", "exclaim"), (";", "semicolon"),
                           (":", "colon"), ("—", "emdash"), ("-", "hyphen"), ("...", "ellipsis")):
            c[name] += t.count(mark)
        c["quote_marks"] += sum(t.count(m) for m in MARKS + '"')
        # tag after ?/!: "Where? She asked" vs "Where? she asked"
        c["cap_after_qe"] += len(re.findall(r"[?!] [A-Z]", t))
        c["low_after_qe"] += len(re.findall(r"[?!] [a-z]", t))
        c["fillers"] += sum(w in FILLERS for w in words(t))
        c["digits"] += len(re.findall(r"\d", t))
        c["sentences"] += len(re.findall(r"[.?!](\s|$)", t))
    return c


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cases", type=Path, default=HERE.parent / "eval" / "cases.jsonl")
    ap.add_argument("--n", type=int, default=24)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--out", type=Path, default=Path("/mnt/data/beardictate-dev/calibration"))
    args = ap.parse_args()

    cases = [json.loads(line) for line in args.cases.read_text(encoding="utf-8").splitlines()]
    cases = [c for c in cases if c["category"] != "spoken_commands"]
    rng = random.Random(args.seed)
    picked = rng.sample(cases, min(args.n, len(cases)))
    args.out.mkdir(parents=True, exist_ok=True)

    asr = make_asr()
    tts = [make_tts(v) for v in VOICES]
    rows = []
    for k, c in enumerate(picked):
        voice = tts[k % len(tts)]
        for variant, text in (("clean", speakable(c["expected"])), ("disfl", speakable(c["raw"]))):
            audio = voice.generate(text, sid=0, speed=1.0)
            hyp = transcribe(asr, np.array(audio.samples), audio.sample_rate)
            rows.append({"id": c["id"], "variant": variant, "voice": VOICES[k % len(VOICES)],
                         "spoken": text, "parakeet": hyp})
            print(f"{c['id']:<10} {variant}  {hyp}", flush=True)

    sim_rng, sim_stats = random.Random(args.seed), Counter()
    sims = [simulate(c["expected"], sim_rng, NoiseConfig(), sim_stats) for c in picked]
    clean_rows = [r for r in rows if r["variant"] == "clean"]
    disfl_rows = [r for r in rows if r["variant"] == "disfl"]

    wer_clean = sum(edit_distance(words(r["spoken"]), words(r["parakeet"])) for r in clean_rows) / max(
        1, sum(len(words(r["spoken"])) for r in clean_rows))
    spoken_fillers = sum(sum(w in FILLERS for w in words(r["spoken"])) for r in disfl_rows)
    kept_fillers = sum(sum(w in FILLERS for w in words(r["parakeet"])) for r in disfl_rows)

    def per100(c: Counter[str]) -> dict[str, float]:
        return {k: round(100 * v / max(1, c["words"]), 2) for k, v in sorted(c.items()) if k != "words"}

    summary = {
        "cases": len(picked),
        "word_error_rate_clean": round(wer_clean, 4),
        "fillers_spoken": spoken_fillers,
        "fillers_transcribed": kept_fillers,
        "per100_source_text": per100(style_stats([speakable(c["expected"]) for c in picked])),
        "per100_parakeet_clean": per100(style_stats([r["parakeet"] for r in clean_rows])),
        "per100_simulator": per100(style_stats(sims)),
        "per100_parakeet_disfl": per100(style_stats([r["parakeet"] for r in disfl_rows])),
    }
    (args.out / "transcripts.jsonl").write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n",
                                                encoding="utf-8")
    (args.out / "simulated.jsonl").write_text(
        "\n".join(json.dumps({"id": c["id"], "sim": s}, ensure_ascii=False) for c, s in zip(picked, sims)) + "\n",
        encoding="utf-8")
    (args.out / "summary.json").write_text(json.dumps(summary, indent=1), encoding="utf-8")
    print(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
