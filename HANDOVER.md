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
  Bindings are **always at least two keys**, no lone modifiers, on every OS.
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
| `bd-vocab` | **Committed.** The user's names/invented words: a list (`Ysolde: Ussel, Isolde`) giving ASR hotwords plus a whole-word fix-up pass before the command pass. See `eval/vocab/README.md` for the measurements. |
| `bd-app` | **Committed.** The tray app, binary `beardictate` (rinch main 422eaa56). Tray glyph states, hotkey toggle (default Ctrl+` = `Ctrl+Backquote`). Always listening by default (DESIGN.md "Always listening"): mic stays on, "bear, wake up"/"bear, go to sleep" or the hotkey switch typing; tray menu: wake/sleep, turn microphone off/on, Settings…, quit. With it off, the hotkey starts and stops mic and dictation as before. `scripts/e2e-nested.sh <wav> <secs> voice` tests it. Settings window (sidebar: General, Microphone, Cleanup, Writing, Special words, About); Cleanup page is the model manager (models.rs: catalog cleanup models, download with progress/cancel/resume, delete; `Command::ModelsChanged` reloads when on "auto"); special words live in settings.toml as `word_groups` (each group and word has an on/off; the old flat `special_words` list moves into a "My words" group on load; starter entry: beardictate ← bear dictate, bare dictate). Engine thread owns mic+pipeline; typing thread; settings file watched. Special words, hotword strength (slider), paragraph and quote options apply to the running pipeline at once (`Feeder::reconfigure`); only the cleanup model or models folder reloads. `beardictate --toggle`, single instance, window fallback with no tray, log at `~/.local/state/beardictate/beardictate.log` (the previous run's kept as `beardictate.log.1`; it records each heard segment's number and word count, joins, voice-wake drops and empty results, never the words). Install: `scripts/install-linux.sh`. E2E: `scripts/e2e-nested.sh <wav>`. UI checks: build with `--features debug` (rinch-debug) and drive it with the rinch MCP server, or set `RINCH_DEBUG_PORT` and speak its framed JSON protocol, inside a private X server and D-Bus. |
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
6. ~~rinch app~~ first version done (`bd-app`). Next for it: Joe's first run on
   KDE Wayland (shortcut + remote-input portals untested), a settings window
   (rebind hotkey, mic, models), first-run flow (DESIGN.md "UI"), rinch
   notifications (#1535) instead of notify-rust, Windows build/test.

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
- `--words FILE` turns on hotwords (`ParakeetConfig::hotword_boost`, beam
  search, per-stream word list) and the fix-up list. Not yet in the cleanup
  prompt: the guardrail already stops the model changing those words.
- Qwen3 1.7B mostly echoes its input (keeps fillers). 4B cleans fine.
- With cleanup **off**, delivery is immediate, so "scratch that" is always
  too late (`ScratchTooLate`). By design, per DESIGN.md; revisit if it bites.
- `bd mic` was only checked to open the device and quit; not yet dictated
  live, and `--inject` not tried on the live desktop.
- `bd devices` (bd-audio `list_input_devices`) also lists outputs/sinks under
  PipeWire. Worth fixing in bd-audio.
- Test audio: Piper TTS works through the venv's `sherpa_onnx`
  (`vits-piper-en_US-ryan-medium` in the dev models dir).

## The app on Joe's machine

Installed 2026-10-14 night under `~/.local` (`scripts/install-linux.sh`), not
autostarted; `~/.config/beardictate/settings.toml` has
`models_dir = "/mnt/data/beardictate-dev/models"`. First launch on KDE
Wayland should ask twice: approve the global shortcut, and allow remote
input (kept in `~/.local/share/beardictate/portal-token`). Reinstall after
changes: `CARGO_TARGET_DIR=... scripts/install-linux.sh`.

- Tested only in a nested KWin on a rootless Xwayland (X11 hotkey, XTest
  typing), via `scripts/e2e-nested.sh`. XTEST from X clients does not reach
  the Xwayland that a nested KWin starts; use a separate `Xwayland :99
  -rootless`, and send a throwaway key (F24) first.
- Mic opens only while dictating (`keep_mic_open = true` to keep pre-roll).
- Changing cleanup/paragraphs/quotes/words reloads all models (~2–5 s).
- After 60 s idle, the next dictation is a new passage (no context).

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

## Website and publishing installers (beardictate.jkbase.app)

The site is live as jkbase project `beardictate`: a static site with landing,
download and help pages, source in this repo's `site/` (deployed with the
`jkbase` CLI; `../jkbase` is only the hosting platform, not where the site
lives). It hosts only the installers; the app downloads models
from the pinned URLs in `crates/bd-models/manifest.toml`. Deploys use Joe's
`~/.jkbase` credentials. **Ask Joe before every deploy.**

Publish a build (the download list is data, `public/downloads/releases.json`):

    cd site
    ./publish.sh add windows 0.1.0 <path/to/beardictate-0.1.0-windows-x64-setup.exe>
    ./publish.sh add windows 0.1.0 <path/to/beardictate-0.1.0-windows-x64.zip>
    ./publish.sh check     # every listed file present, size + SHA-256 match
    ./publish.sh deploy    # check, then `jkbase deploy`

Then commit `site/public/downloads/releases.json`. The binaries are gitignored, so
the checkout you publish from holds the only copy; `deploy` refuses if a
listed file is missing or its hash differs. Platforms: windows, linux, macos;
the button label follows the extension (override with `--kind "…"`).
`prune <platform> <n>` keeps the newest n versions, `remove <file>` drops
one. A page-only change is the same `./publish.sh deploy`. The project was
created with `jkbase project create beardictate`. Details: that folder's
README.md.

**Released:** 0.1.3, built from main `cbf1770`, published 2026-10-15 (site v6; 0.1.2 pruned).
- Windows: `beardictate-0.1.3-windows-x64-setup.exe` and a `.zip`, cross-built here (`scripts/cross-build-windows.sh`) and packed with Inno Setup on NICO-TOP; unsigned; cleanup on the GPU through Vulkan (backends loaded at run time, CPU fallback), ships the VC++ runtime app-local.
- Linux: `beardictate-0.1.3-linux-x86_64.tar.gz` from `scripts/package-linux.sh`; needs glibc 2.43+.
- New release: bump `version` in Cargo.toml, `scripts/package-linux.sh`, ask the Windows session to build the same commit, `site/publish.sh add …`, `prune <platform> 1`, deploy (Joe's OK), commit `releases.json`.
- The binaries live in `site/public/downloads/` (gitignored) and `/mnt/data/beardictate-targets/win-dist/`.

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
