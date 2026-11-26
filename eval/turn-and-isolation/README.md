# Turn detection and voice isolation on a tester's recordings

An offline test of two Apache-2.0 models from Inworld on Will's real
dictation, before any app work:

- **SAVA** (github.com/inworld-ai/sava): streaming speech and end-of-turn
  probabilities every 40 ms. Can it tell a thinking pause from a finished
  sentence better than our silence rule?
- **Voice Isolator** (github.com/inworld-ai/voice-isolator), realtime
  checkpoint: a 48 kHz denoiser. Does cleaning the audio change what our
  recognizer hears?

Results and the recommendation: [FINDINGS.md](FINDINGS.md).

Nothing here holds a tester's words or audio. Every script writes to a data
directory you name with `--data`, and refuses one inside the repository.
Keep it in a scratch folder and delete it when you're done (CLAUDE.md,
"Testers' telemetry").

## Setup

```bash
D=/path/to/scratch            # outside the repo
uv venv -p 3.12 $D/venv && . $D/venv/bin/activate
uv pip install --index-url https://download.pytorch.org/whl/cpu torch torchaudio
uv pip install inworld-sava inworld-voice-isolation onnx onnxruntime onnxscript soundfile soxr numpy
telemetry/pull.sh export $D/will.jsonl --install <install id> --since 2026-10-17
```

For the transcription step, `bd` needs the project's patched sherpa-onnx
and its models:

```bash
scripts/build-sherpa-onnx.sh linux          # or reuse a checkout's target/sherpa-onnx
export SHERPA_ONNX_LIB_DIR=$PWD/target/sherpa-onnx/<version>-linux-x64/lib
cargo build --release -p bd-cli
cp $SHERPA_ONNX_LIB_DIR/lib{sherpa-onnx-c-api,onnxruntime}.so target/release/
export BD_MODELS_DIR=$D/models
target/release/bd models download vad-silero asr-nemotron-speech-streaming-en-0.6b-1120ms-int8
```

Both packages download their weights from Hugging Face on first use and
check them against the sha256 in their own registry
(`sava/registry.py`, `voice_isolation/registry.py`).

## SAVA: pauses

Run from this folder:

```bash
python build_streams.py --export $D/will.jsonl --data $D/data
```

picks runs of consecutive segments (six normal sessions, every list-mode
run), fetches their audio, and rebuilds each run as one stream: the
segments' audio with each real gap (`pause_ms`) filled with white noise at
that segment's own noise floor. Gaps over 10 s aren't rebuilt (every rule
has ended the utterance by then). It writes `boundaries.jsonl` with the
text on either side of each pause, for labelling.

Label the boundaries `mid` / `end` / `unsure` following
[label_prompt.md](label_prompt.md), into `$D/data/labels.jsonl` (we used a
Claude agent given only that file and the texts, then checked a sample by
hand).

```bash
python run_sava.py --data $D/data --check --jobs 6
python score_turns.py --data $D/data --json $D/data/turn_results.json
```

`run_sava.py` streams each rebuilt stream through `sava.Session` and, at
every segment end, runs a copy of the session on 10.5 s more room noise, so
each boundary has its `turn` curve for any length of pause. `--check` first
confirms 1 s pushes give the same probabilities as 40 ms ones.
`score_turns.py` prints, per rule, the false splits at `mid` boundaries and
the wait at `end` ones (its docstring defines the rules).

```bash
python export_sava_onnx.py --out $D/onnx --audio $D/data/streams/s000.npy --frames 1500
```

exports the one-frame streaming step (state in, state out) with the dynamo
exporter, steps onnxruntime and PyTorch through the same audio, and prints
the largest difference and each runtime's time per frame.

## Voice Isolator: transcripts

```bash
python isolate.py select --export $D/will.jsonl --data $D/data
python isolate.py isolate --data $D/data --threads 4
python isolate.py transcribe --export $D/will.jsonl --data $D/data
python score_isolation.py --data $D/data --json $D/data/iso_results.json
python isolate.py cost --data $D/data --threads 1 --seconds 5
```

For the GPU timing, a second venv with ROCm torch (AMD; the CUDA wheel on
NVIDIA) and `--device cuda`. On a Radeon 890M (gfx1150) ROCm 6.4 needs
`HSA_OVERRIDE_GFX_VERSION=11.0.0`:

```bash
uv venv -p 3.12 $D/venv-rocm && . $D/venv-rocm/bin/activate
uv pip install --index-url https://download.pytorch.org/whl/rocm6.4 torch torchaudio
uv pip install inworld-voice-isolation soundfile soxr numpy
HSA_OVERRIDE_GFX_VERSION=11.0.0 python isolate.py cost --data $D/data --device cuda --seconds 5
```

`select` picks the runaway-repeat segments, the 11/25 supper conversation
(session 3890918c, 635–647), quiet, noisy and control segments. `isolate`
runs the realtime checkpoint streaming (20 ms pieces, 48 kHz in and out,
resampled with soxr from and back to 16 kHz). `transcribe` runs
`bd file --cleanup off` on the original and the isolated audio with the
tester's own word list and hotword boost from his settings.
`score_isolation.py` compares them per group.

## Cleaning up

```bash
rm -rf -- "$D"
```
