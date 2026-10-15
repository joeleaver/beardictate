# beardictate — handover

Last updated: 2026-10 (after the weekly-limit reset). Read this, then
[DESIGN.md](DESIGN.md), which is canonical for *what* we're building. This
file is *where things stand*.

## Decisions already made (don't re-litigate)

- Pipeline: hotkey → cpal capture → Silero VAD → **Parakeet TDT 0.6B v3**
  (sherpa-onnx) → deterministic spoken-command pass → **cleanup LLM** with a
  word-diff **guardrail** → text injection (type, else paste).
- **No floating overlay.** Tray icon is the only live status display (Joe,
  2026-10-14). Hotkey is a **toggle** by default; hold-to-talk is optional.
  Exceptions → rate-limited OS notifications. Windows first-run has a
  "pin the bear to the taskbar" step; optional start/stop click sound.
- **Windows is first-class**; Linux (KDE Wayland is Joe's desktop) and macOS follow.
- UI toolkit is **rinch** (Joe's own, ~/dev/rinch). Hotkeys, tray and
  notifications are rinch features, built there, not worked around here.
- Models ship **separately**, downloaded on demand from a pinned, hashed
  manifest. Cleanup is a user-selectable catalog with a hardware-measured
  default (see DESIGN.md "Cleanup model choices").
- Cleanup model plan: stock Qwen3 now; fine-tuned **Qwen3 0.6B/1.7B/4B "Bear"**
  models later (Apache-2.0, so we can host them; Gemma is HF-gated).
- Conventions: files ≲500 lines, clippy-clean, correctness over expediency,
  no TODO stubs. Commit trailer:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Repo state

Public: https://github.com/joeleaver/beardictate (MIT). Branch `main`.

| Crate / dir | State |
|---|---|
| `bd-core` | **Committed.** Traits `Asr`, `Cleanup`, `Injector`; `AudioSegment`, `CleanupRequest`, `InjectOutcome`, `Style`. |
| `bd-models` | **Committed.** Manifest (`manifest.toml`, pinned HF commit SHAs + sha256), store (`BD_MODELS_DIR` override), resumable verified downloader, folder import. 44 tests. Catalog Qwen3 entries use prompt template `generic-v1` (the earlier `qwen3-cleanup-v1` never existed in bd-cleanup). |
| `bd-audio` | **Committed.** cpal capture via `rtrb` lock-free ring, rubato resampling, 300 ms pre-roll, device fallback, RMS level. Recommend enabling its `pipewire` feature on Linux. |
| `bd-asr` | **Committed.** `ParakeetAsr` + `Segmenter` (Silero). WER 0 on the test wav; CPU RTF ≈0.15–0.24 on a loaded box. sherpa-onnx links **shared** (see below). |
| `bd-commands` | **Committed.** Spoken-command rules + prose false-positive tests. |
| `bd-cleanup` | **Committed.** Prompts, guardrail, `LlamaCleanup` (llama-cpp-2, `vulkan` feature), `HttpCleanup`, `examples/bench_cleanup`. Output tidying also unwraps an answer echoed in `"…"` and turns straight quotes into the style's marks (`quotes.rs`). |
| `bd-platform` | **Committed.** Windows SendInput/paste/UIPI (cross-checked with `cargo xwin clippy`, **never run on real Windows** — see `crates/bd-platform/WINDOWS_TESTING.md`); Linux XTest + RemoteDesktop portal/libei (live-tested only in a nested KWin). `prepare()` triggers the Wayland permission dialog; `INJECTED_MARKER` in `dwExtraInfo` lets rinch's keyboard hook ignore our own keystrokes. |
| `eval/`, `training/` | **Committed.** ~150 original cases + UK set, `score.py`, runner, Gutenberg corpus fetch, pair generator, fine-tune recipe. |
| `bd-pipeline` | **Committed.** Threads segment → ASR → (commands, cleanup, join), `Event` stream out, `Feeder` handle in. Scratch-that drops queued/in-cleanup segments, restores quote state; `ScratchTooLate` otherwise. The caller delivers text (so hold-to-talk can queue until key-up). |
| `bd-cli` | **Committed.** Binary `bd`: `file`, `mic` (Enter toggles, `--inject`), `models list/download/verify/import`, `devices`, `eval` (outputs.jsonl for `score.py`). Features `vulkan`, `pipewire`. |
| `design/` | **Committed.** Design-canvas source (`.dc.html` artboards). |

Uncommitted work in the tree belongs to those subagents. If they finished
before this session ended, their output is already committed; if you find
uncommitted files under those paths, **verify then commit** (see checklist).

**Never `git commit -a` while subagents are working** — it sweeps their
half-written files into the commit. Always `git add <paths>` / `git commit -- <paths>`.

### Checklist for each finished crate before committing
```bash
export CARGO_TARGET_DIR=/mnt/data/beardictate-targets/main
export BD_MODELS_DIR=/mnt/data/beardictate-dev/models
cargo test -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
wc -l crates/<crate>/src/**/*.rs   # nothing far over 500
```
bd-platform's Windows half: `cargo xwin check -p bd-platform --target x86_64-pc-windows-msvc`
(and clippy for that target). Its live tests may only run inside a nested
`Xvfb` display, **never on Joe's live desktop.** eval/training: `pytest eval/tests`
in the venv `/mnt/data/venvs/beardictate`.

If a crate is half-done with no agent running, re-brief a new agent with
the crate's section of DESIGN.md plus "only touch this crate, own
CARGO_TARGET_DIR under /mnt/data/beardictate-targets/, don't commit, report
API + tests".

## Next steps (in order)

1. ~~Land the four in-flight pieces~~ (done).
2. ~~**bd-core tweaks**~~ (done) suggested by the ASR agent, worth taking:
   `AudioSegment.start_sample: u64` (alignment/gap detection) and
   `Asr::transcribe(&self)` (sherpa recognizer is stateless per call).
   From the platform agent: add a default `Injector::prepare()` (Wayland
   portal approval through the trait object); `InjectOutcome::Pasted`
   should carry `restored: bool` (restore can be off/skipped/failed); and
   **hold-to-talk**: injection waits for modifiers to be released, so with a
   held hotkey the pipeline must queue injection until key-up (or hold
   bindings must avoid modifiers). Check what the cleanup agent asked for too.
3. ~~**Pipeline + `bd` CLI**~~ (done, see "Running it" below) (`crates/bd-cli`, binary `bd`): a pipeline module
   (threads + crossbeam channels: segmenter → ASR → commands → cleanup →
   sink, cleaning sentence N while N+1 is spoken; context buffer of our own
   last ~2 paragraphs). Subcommands: `bd file <wav>`, `bd mic` (Enter to
   toggle in the terminal; stdout sink, optionally `--inject`), `bd models
   list|download|verify`, `bd eval` (writes outputs.jsonl for
   `eval/score.py`). Consider putting the pipeline in its own `bd-pipeline`
   crate so the rinch app reuses it.
4. Re-run the eval baselines on an **idle** machine (everything measured so
   far was under load avg 17–34).
5. Fine-tune Bear models per `training/README.md` (rent a GPU; Joe has the
   JarvisLabs skill). They must beat Gemma 4 12B on the eval to ship.
6. rinch app (tray, settings, first-run) once rinch#1521 + #1520 land.

## Running it (Joe's machine)

```bash
export CARGO_TARGET_DIR=/mnt/data/beardictate-targets/main BD_MODELS_DIR=/mnt/data/beardictate-dev/models
cargo build --release -p bd-cli --features vulkan,pipewire
B=$CARGO_TARGET_DIR/release/bd
$B models list                      # Qwen3 1.7B and 4B are imported (hard links to /mnt/data/models)
$B file some.wav -v                 # text on stdout, per-segment timings on stderr
$B mic                              # Enter to start/stop; add --inject to type into the focused window
$B eval --cases eval/cases.jsonl --out /tmp/x && python eval/score.py eval/cases.jsonl /tmp/x/outputs.jsonl
```

- Default cleanup is the largest installed catalog model (Qwen3 4B here);
  `--cleanup off|ID|file.gguf` or `--cleanup-url`.
- **sherpa-onnx is linked shared**, not static: static onnxruntime and
  llama.cpp each bring `std::regex` template instances built by different
  GCCs, and one binary with both aborted in `free()` while loading the VAD.
  sherpa's build script copies `libsherpa-onnx-c-api.so` and
  `libonnxruntime.so` (DLLs on Windows) next to the binary; `bd-cli/build.rs`
  sets rpath `$ORIGIN`. **Packaging must ship those libraries with the app.**
- Qwen3 1.7B mostly echoes its input (keeps fillers). 4B cleans fine.
- With cleanup **off**, delivery is immediate, so "scratch that" is always
  too late (`ScratchTooLate`). By design, per DESIGN.md; revisit if it bites.
- `bd mic` was only checked to open the device and quit; not yet dictated
  live, and `--inject` not tried on the live desktop.
- `bd devices` (bd-audio `list_input_devices`) also lists outputs/sinks under
  PipeWire. Worth fixing in bd-audio.
- Test audio: Piper TTS works through the venv's `sherpa_onnx`
  (`vits-piper-en_US-ryan-medium` in the dev models dir).

## External dependencies

- **rinch** (joeleaver/rinch), being worked by Joe's "Rinch" Desktop session:
  - [#1521](https://github.com/joeleaver/rinch/issues/1521) global hotkeys — **priority**
  - [#1520](https://github.com/joeleaver/rinch/issues/1520) tray runtime icon/tooltip/menu, checkable items, bindable left-click, tray-only app — **priority**
  - [#1522](https://github.com/joeleaver/rinch/issues/1522) OS notifications (until then, `notify-rust` behind a small trait)
  - [#1519](https://github.com/joeleaver/rinch/issues/1519) non-activating windows — general rinch feature, **not** on our path
  Status (rinch session, 2026-10-14 evening): **#1527** (tray) in its fix
  round, merges once green, ETA hours; **#1531** (macOS
  `ActivationPolicy::Accessory`, `TrayIconBuilder::with_id`) merges right
  after it. **#1530** (hotkeys) still in adversarial review, no ETA. API as
  below, unchanged. To build early: git deps on `branch = "fix-1520"` /
  `"fix-1521"` pinned by `rev`, copy rinch's `[patch.crates-io]`
  (wgpu/winit forks); the two branches don't contain each other, so both at
  once means waiting for main. #1526 and #1522 not started. Earlier report:
  **#1530** (hotkeys: `rinch::hotkey::register`, feature `global-hotkeys`;
  press/release on main thread, rebind, `Conflict` error, sided/modifier-only
  keys like RightCtrl, `HotkeyCapture`, `Binding::SystemChosen` for the
  Wayland portal) and **#1527** (tray: non-blocking `set_icon`/`set_tooltip`/
  `set_menu`, `MenuItem::checkbox/radio` bound to a Signal, `on_activate`,
  `WindowProps::visible=false` for tray-only). Follow-ups #1528 (macOS
  no-Dock activation policy), #1529 (per-app ksni tray id).
  Gotchas for the app:
  - **Ship a real `.desktop` file on Linux.** The KDE GlobalShortcuts
    portal needs one for an unsandboxed app (rinch writes a NoDisplay stub
    if missing, but ours should be proper).
  - **rinch#1526:** `system-tray` + `file-dialogs` together panic on Linux
    when a file dialog opens. "Import model folder…" needs a file dialog:
    don't enable both until #1526 is fixed (or pick the folder another way).
  - macOS: chord hotkeys need no permission; modifier-only ones need Input
    Monitoring (first-run onboarding step).
  The Rinch session said it would message the beardictate session as each
  lands; check `gh issue view <n> -R joeleaver/rinch` rather than relying on that.

## Where things live (outside the repo)

- Models (dev): `/mnt/data/beardictate-dev/models/` —
  `asr-parakeet-tdt-0.6b-v3-int8/` (+ `test_wavs/`), `vad-silero/silero_vad.onnx`.
- Corpus / training data: `/mnt/data/beardictate-dev/corpus/` etc.
- Build dirs: `/mnt/data/beardictate-targets/<name>/` (root disk is ~93% full —
  keep big things on /mnt/data).
- Stock GGUFs for benchmarking: `/mnt/data/models/{Qwen3-1.7B,Qwen3-4B,Qwen3-8B,gemma-4-12B}/`.
  llama-server: `/mnt/data/llama.cpp/build/bin/llama-server`.
- Python venv: `/mnt/data/venvs/beardictate`.

## Design canvas

Claude Design canvas: https://claude.ai/artifact/So5EqTegtNMQeDsByTWQBt — owned
by the claude.ai account the first session ran under; invisible to other
accounts. Source is in `design/canvas/` regardless. If the Design artifact
type isn't available in a session, the account's Settings › Capabilities →
"Artifacts" is off/greyed (needs "Code execution and file creation" on, or
an org admin: Organization settings › Capabilities / Artifacts).

## Known numbers (all on a heavily loaded box — indicative only)

- Stock cleanup models: only **Gemma 4 12B** handled unmarked dialog (≈5 s GPU);
  Qwen3 1.7B/4B/8B fast but miss dialog inference.
- Parakeet v3 int8 CPU: RTF ≈0.15–0.24.
