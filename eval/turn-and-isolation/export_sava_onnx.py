"""Exports SAVA's streaming step to ONNX and checks onnxruntime against PyTorch.

The step takes one 40 ms frame (640 samples) and the whole streaming state
as flat tensors, and returns the frame's speech and turn logits and the new
state, so a Rust host can carry the state between calls exactly as
`sava.Session` does. The check steps both through the same audio, frame by
frame, and compares the probabilities, and times each runtime per frame.

Usage: export_sava_onnx.py --out DIR [--audio stream.npy] [--frames 1500]
                           [--exporter dynamo|torchscript]
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import time
from pathlib import Path

import numpy as np
import onnxruntime
import torch
from inworld import sava
from inworld.sava import config


def sigmoid(x) -> float:
    return float(1 / (1 + np.exp(-np.float64(x))))


def flatten(state) -> tuple[list[torch.Tensor], object]:
    """A state's tensors, depth first, and a spec to rebuild it."""
    if isinstance(state, torch.Tensor):
        return [state], "t"
    if isinstance(state, list):
        parts = [flatten(s) for s in state]
        return [t for p in parts for t in p[0]], ("list", [p[1] for p in parts])
    if dataclasses.is_dataclass(state):
        names = [f.name for f in dataclasses.fields(state)]
        parts = [flatten(getattr(state, n)) for n in names]
        return [t for p in parts for t in p[0]], ("dc", type(state), names, [p[1] for p in parts])
    raise TypeError(f"unexpected state part {type(state)}")


def unflatten(tensors: list[torch.Tensor], spec) -> object:
    it = iter(tensors)

    def build(s):
        if s == "t":
            return next(it)
        if s[0] == "list":
            return [build(x) for x in s[1]]
        _, cls, names, specs = s
        return cls(**{n: build(x) for n, x in zip(names, specs)})

    return build(spec)


class Step(torch.nn.Module):
    """One streaming step with the state as flat tensors."""

    def __init__(self, model, spec):
        super().__init__()
        self.model = model
        self.spec = spec

    def forward(self, pcm, *state):
        out, new = self.model.step(pcm, unflatten(list(state), self.spec))
        speech = out.speech if out.lookahead is None else out.lookahead
        flat, _ = flatten(new)
        return (speech, out.turn, *flat)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True)
    parser.add_argument("--audio", help="a 16 kHz float32 .npy to step through (default: speech-like noise)")
    parser.add_argument("--frames", type=int, default=1500)
    parser.add_argument("--threads", type=int, default=1)
    parser.add_argument("--exporter", choices=["dynamo", "torchscript"], default="dynamo")
    args = parser.parse_args()
    torch.set_num_threads(args.threads)
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    model = sava.load("inworld/sava")
    state = model.init_state(1, torch.device("cpu"), torch.float32)
    flat, spec = flatten(state)
    step = Step(model, spec).eval()
    pcm = torch.zeros(1, config.FRAME_SAMPLES)
    path = out_dir / "sava_step.onnx"
    names = [f"state_{i}" for i in range(len(flat))]
    with torch.inference_mode():
        started = time.time()
        torch.onnx.export(step, (pcm, *flat),
                          str(path),
                          input_names=["pcm", *names],
                          output_names=["speech", "turn", *[f"new_{n}" for n in names]],
                          dynamo=args.exporter == "dynamo",
                          opset_version=18)
        export_s = time.time() - started

    if args.audio:
        audio = np.load(args.audio).astype(np.float32)
    else:
        rng = np.random.default_rng(0)
        audio = (rng.standard_normal(args.frames * config.FRAME_SAMPLES) * 0.05).astype(np.float32)
    frames = min(args.frames, audio.size // config.FRAME_SAMPLES)

    options = onnxruntime.SessionOptions()
    options.intra_op_num_threads = args.threads
    ort = onnxruntime.InferenceSession(str(path), options, providers=["CPUExecutionProvider"])
    ort_inputs = [i.name for i in ort.get_inputs()]
    ort_state = [t.numpy() for t in flat]
    session = sava.Session(model)
    diffs_speech, diffs_turn = [], []
    torch_s = ort_s = 0.0
    for k in range(frames):
        piece = audio[k * config.FRAME_SAMPLES:(k + 1) * config.FRAME_SAMPLES]
        started = time.perf_counter()
        ref = session.push(piece)
        torch_s += time.perf_counter() - started
        started = time.perf_counter()
        outs = ort.run(None, dict(zip(ort_inputs, [piece[None], *ort_state])))
        ort_s += time.perf_counter() - started
        ort_state = outs[2:]
        diffs_speech.append(abs(sigmoid(outs[0].ravel()[0]) - float(ref.speech[0])))
        # turn is [batch, frames, horizons]; horizon 0 is Session's `turn`.
        diffs_turn.append(abs(sigmoid(outs[1][0, 0, 0]) - float(ref.turn[0])))
    print(
        json.dumps({
            "exporter": args.exporter,
            "threads": args.threads,
            "export_s": round(export_s, 1),
            "onnx_mb": round(sum(p.stat().st_size for p in out_dir.glob("sava_step.onnx*")) / 1e6, 1),
            "state_tensors": len(flat),
            "frames": frames,
            "max_abs_diff_speech": max(diffs_speech),
            "max_abs_diff_turn": max(diffs_turn),
            "torch_ms_per_frame": round(1000 * torch_s / frames, 2),
            "ort_ms_per_frame": round(1000 * ort_s / frames, 2),
        }))


if __name__ == "__main__":
    main()
