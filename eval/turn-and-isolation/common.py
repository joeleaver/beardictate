"""Shared helpers: read a telemetry export, fetch and read segment audio.

Every script here reads testers' words and voices, so everything they write
goes to a data directory outside the repository (`--data`), never into it.
"""

from __future__ import annotations

import dataclasses
import json
import subprocess
from collections import defaultdict
from pathlib import Path

import numpy as np
import soundfile

REPO = Path(__file__).resolve().parents[2]
SAMPLE_RATE = 16_000
# The segmenter keeps this much audio on each side of the detected speech
# (`SegmenterConfig::padding`, crates/bd-asr/src/segmenter.rs).
PADDING_S = 0.2


@dataclasses.dataclass
class Seg:
    """One dictated segment, as the export records it."""

    session: str
    segment: int
    ts: str
    text: str
    pause_ms: int | None
    audio_ms: int
    level_dbfs: float
    far: bool
    audio_key: str | None
    list_mode: bool
    cleaned_with: list[int]

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
            cleaned = [e["data"].get("segments") or [] for e in row["events"] if e["kind"] == "cleaned"]
            settings = row.get("settings") or {}
            sessions[row["session"]].append(
                Seg(session=row["session"],
                    segment=row["segment"],
                    ts=heard[0]["ts"],
                    text=data.get("text") or "",
                    pause_ms=data.get("pause_ms"),
                    audio_ms=data.get("audio_ms") or 0,
                    level_dbfs=data.get("level_dbfs") or -100.0,
                    far=bool(data.get("far")),
                    audio_key=row.get("audio") if data.get("audio") else None,
                    list_mode=bool(settings.get("list_mode")),
                    cleaned_with=cleaned[0] if cleaned else []))
    for segs in sessions.values():
        segs.sort(key=lambda s: s.segment)
    return sessions


def fetch_audio(seg: Seg, data: Path) -> Path:
    """Fetches a segment's FLAC into `data/audio/` (once) with telemetry/pull.sh."""
    if not seg.audio_key:
        raise ValueError(f"{seg.sid} has no audio")
    out = data / "audio" / f"{seg.session}-{seg.segment}.flac"
    if not out.exists():
        out.parent.mkdir(parents=True, exist_ok=True)
        part = out.with_suffix(".part.flac")
        subprocess.run([str(REPO / "telemetry" / "pull.sh"), "audio", seg.audio_key, str(part)],
                       check=True,
                       stdout=subprocess.DEVNULL)
        part.rename(out)
    return out


def read_audio(path: Path) -> np.ndarray:
    """Reads 16 kHz mono audio as float32."""
    audio, rate = soundfile.read(path, dtype="float32", always_2d=True)
    if rate != SAMPLE_RATE:
        raise ValueError(f"{path}: {rate} Hz, expected {SAMPLE_RATE}")
    return audio.mean(axis=1)


def noise_floor(audio: np.ndarray) -> float:
    """The RMS of a segment's quietest 10% of 20 ms windows: its room noise."""
    win = SAMPLE_RATE // 50
    n = audio.size // win
    if n == 0:
        return 1e-4
    rms = np.sqrt((audio[:n * win].reshape(n, win)**2).mean(axis=1))
    return float(max(np.percentile(rms, 10), 1e-5))
