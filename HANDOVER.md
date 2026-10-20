# beardictate — handover

Last updated: 2026-10-20, after 0.1.18. Read this, then
[DESIGN.md](DESIGN.md), which is canonical for *what* we're building. This
file is *where things stand*.

## Start here (open items, 2026-10-19)

1. **rinch#1597** merged (a3de4b99) and is pinned since 0.1.18; Joe tried
   the bear's drag, menu and see-through glow on KDE Wayland (2026-10-20).
2. **The bear's default place**: near the bottom right of the screen (Joe).
   Needs the monitor's size from rinch; not started. `pad_window.rs`
   `load_bear` is where a missing position falls back.
3. **Settings telemetry**: 0.1.17 is the first build that sends a
   "settings" event. Check one arrives (`telemetry/pull.sh export … --setting
   dialog=true` returned nothing on release day, as expected).
4. **Not tried by real voice yet**: "bear, wake up / delete / undo / send it
   / clear / start a list / end the list" (Piper can't say "bear"). The bear
   and its menu are untried on Windows and X11.
5. **Unreleased (2026-10-20), from the Windows tester's telemetry**: other
   voices in the room aren't typed or sent (more than 15 dB below the
   user's learned level, `bd-pipeline` `voice.rs`), it goes back to sleep
   after 10 minutes unspoken to (Settings › General: 5–60 min or never), and "microphone is quiet" shows on the
   bear (amber "!"), the tray tooltip and menu and Settings › Microphone,
   not only as a notification (Windows silenced it). DESIGN.md "Always
   listening", docs/telemetry.md (`far`, `unspoken_to`). Proved by tests
   only; not yet tried by voice in the app. After release, check
   `transcribed` events with `far: true` against testers' own levels.
6. Smaller: the Post-processing model is never chosen automatically; the
   KDE portal keeps its own copy of the shortcut (documented); Nemotron
   sometimes misses a real sentence end across a pause. Command hotwords were
   tried and rejected (they made Nemotron hear commands in noise).

## Decisions already made (don't re-litigate)

- Pipeline: hotkey → cpal capture → Silero VAD → **Nemotron Speech Streaming
  0.6B English, 1120 ms** (int8, sherpa-onnx, fed live; the only recognizer
  since 2026-10-19, replacing Parakeet Unified, whose code is gone) → deterministic spoken-command pass → **cleanup LLM** with a
  word-diff **guardrail** → text injection (type, else paste).
- **The bear on screen** (Joe, 2026-10-19, replacing "no floating overlay"):
  a small glowing bear, on by default, shows the state, starts and stops
  dictating on a click and opens the tray's menu on a right-click; it is the
  Bear Pad window's other shape (DESIGN.md). The tray icon stays. Hotkey is a **toggle** by default; hold-to-talk is optional.
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
| `bd-asr` | **Committed.** `NemotronAsr` + `Segmenter` (Silero). WER 0 on the test wav; CPU RTF ≈0.15–0.24 on a loaded box. sherpa-onnx links **shared** (see below). |
| `bd-commands` | **Committed.** Spoken-command rules + prose false-positive tests. |
| `bd-cleanup` | **Committed.** Prompts, guardrail, `LlamaCleanup` (llama-cpp-2, `vulkan` feature), `HttpCleanup`, `examples/bench_cleanup`. Output tidying also unwraps an answer echoed in `"…"` and turns straight quotes into the style's marks (`quotes.rs`). |
| `bd-platform` | **Committed.** Windows SendInput/paste/UIPI (cross-checked with `cargo xwin clippy`, **never run on real Windows** — see `crates/bd-platform/WINDOWS_TESTING.md`); Linux XTest + RemoteDesktop portal/libei (live-tested only in a nested KWin). `prepare()` triggers the Wayland permission dialog; `INJECTED_MARKER` in `dwExtraInfo` lets rinch's keyboard hook ignore our own keystrokes. |
| `eval/`, `training/` | **Committed.** ~150 original cases + UK set, `score.py`, runner, Gutenberg corpus fetch, pair generator, fine-tune recipe. |
| `bd-pipeline` | **Committed.** Threads segment → ASR → (commands, cleanup, join), `Event` stream out, `Feeder` handle in. Scratch-that drops queued/in-cleanup segments, restores quote state; `ScratchTooLate` otherwise. The caller delivers text (so hold-to-talk can queue until key-up). |
| `bd-vocab` | **Committed.** The user's names/invented words: a list (`Ysolde: Ussel, Isolde`) giving ASR hotwords plus a whole-word fix-up pass before the command pass. See `eval/vocab/README.md` for the measurements. |
| `bd-app` | **Committed.** The tray app, binary `beardictate` (rinch main 422eaa56). Tray glyph states, hotkey toggle (default Ctrl+` = `Ctrl+Backquote`). Always listening by default (DESIGN.md "Always listening"): mic stays on, "bear, wake up"/"bear, go to sleep" or the hotkey switch typing; tray menu: wake/sleep, turn microphone off/on, Settings…, quit. With it off, the hotkey starts and stops mic and dictation as before. `scripts/e2e-nested.sh <wav> <secs> voice` tests it. Settings window (sidebar: General, Microphone, Post-processing, Writing, Special words, About); the Post-processing page holds Tidy up, Dialog and the model manager (models.rs: catalog cleanup models, download with progress/cancel/resume, delete; `Command::ModelsChanged` reloads when on "auto"); special words live in settings.toml as `word_groups` (each group and word has an on/off; the old flat `special_words` list moves into a "My words" group on load; starter entry: beardictate ← bear dictate, bare dictate). Engine thread owns mic+pipeline; typing thread; settings file watched. Special words, hotword strength (slider), paragraph and quote options apply to the running pipeline at once (`Feeder::reconfigure`); only the cleanup model or models folder reloads. `beardictate --toggle`, single instance, window fallback with no tray, log at `~/.local/state/beardictate/beardictate.log` (the previous run's kept as `beardictate.log.1`; it records each heard segment's number and word count, joins, voice-wake drops and empty results, never the words). Install: `scripts/install-linux.sh`. E2E: `scripts/e2e-nested.sh <wav>`. UI checks: build with `--features debug` (rinch-debug) and drive it with the rinch MCP server, or set `RINCH_DEBUG_PORT` and speak its framed JSON protocol, inside a private X server and D-Bus. |
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

**Redesign of how words reach the page** (Joe, 2026-10-19; DESIGN.md
"Where words go", "5. Cleanup LLM"). Testers said classic mode took so long
they wondered if it worked, and cleanup quoted lines that weren't dialog.
- Done: the four modes became `destination` (app/pad) + `tidy`; Nemotron
  1120 ms for everything (catalog entry, required at first run; the 560 ms
  and Parakeet entries stay, not required); cleanup warmed up at load; the
  sentence hold is 1.5 s, longer only while the next utterance is being
  spoken (6 s cap); raw text when cleanup is 4 s behind; cleanup context
  only from the last 90 s, 600 chars; a new passage after 60 s of silence
  or when another app has the focus (`bd_platform::FocusProbe`; not on
  Wayland); "bear, clear" closes a voice-opened quote. `bd file --realtime`
  feeds audio at speaking pace and `-v` shows each text's wait.
- Done (3): with tidy on in your app, words are typed as spoken and each
  sentence is tidied in place on the cleanup thread (DESIGN.md "Tidying in
  place"); guards: still all there, within 15 s, same app (not on Wayland),
  at most 400 characters back; text that didn't land starts a new passage.
- Done (4): voice edits in your app, "bear, change/delete/undo" on what
  was typed in the last 60 s (DESIGN.md "Voice edits in your app").
  Checked with tests and `bd file --stream` on Piper TTS spelled "Bear.";
  not yet by voice in the app.
- Done (5): Parakeet removed (`ParakeetAsr`, `setup::load_asr`, `bd`'s
  non-streaming path; `--stream` is still accepted, hidden). The tests read
  `test_wavs/en.wav` at the root of `BD_MODELS_DIR` and transcribe with
  Nemotron. The manifest marks Parakeet Unified, Parakeet v3 int8 and
  Nemotron 560 ms `retired`: never downloaded, and the General page offers
  "Delete them" while any is on disk (careful on Joe's machine: the dev
  models folder still holds them for the eval scripts). CI: the Windows job
  builds with Ninja and the MSVC environment (the Visual Studio generator
  never built llama.cpp's Vulkan shader tool, so CI had never passed) and
  runs Nemotron greedily on the stock library; a new Linux job builds our
  sherpa-onnx (cached) and runs clippy, the tests and `bd` with hotwords.
- Done (6): punctuation at pauses. A full stop was added at every pause,
  mid-sentence too; now Nemotron's stream is held over a pause and the next
  words decide (DESIGN.md "Pauses", `stages/listen.rs`). Checked with
  `bd file --realtime` on Piper TTS with paused sentences; not yet by
  voice in the app.
- Done (7): spoken punctuation in the app ("comma", "period", "question
  mark", "exclamation point"…; Writing › Punctuation, on by default). It was
  only a `bd` flag, so testers' marks were typed as words. A mark said alone
  after a pause joins the utterance before it (`stages/listen.rs`), so the
  sentence is tidied once with it; `compose::join` lets a mark replace the
  one the recognizer put there. Checked with `bd file` on Piper TTS.
- Done (8): Dialog split from Tidy (Joe, 2026-10-19): a peer of Tidy up on
  General and in the tray, off by default. Off, `keep_dictated_quotes`
  (bd-cleanup guardrail) takes out quote marks the model added and no
  paragraph starts per speaker; Bear, trained on novels, quoted every short
  line. `bd --no-dialog`. Retraining Bear with a dialog tag is the lasting
  fix. "Cleanup" is "Post-processing" everywhere in the app (the site's
  help page still says cleanup).
- Done (9): the bear on screen (DESIGN.md), on by default: state glow,
  click to dictate, drag, right-click for the tray's menu. Settings words made
  consistent: dictating, wake up/go to sleep, Tidy up, Dialog,
  Post-processing (the model and its page, whose own Off/On is gone: an old
  `cleanup = "off"` becomes `tidy = false`), Bear Pad, the bear's menu. The
  bear and the pad are one window (the pad's process); Tidy up and Dialog are
  on the Post-processing page. Tried in a nested KWin (bear, pad with the bear
  in its title, close back to the bear, menu, click); dragging the bear by
  hand not yet re-tried after it went haywire with its own drag start, and
  not yet on Windows or X11.
- Done (10): numbers and lists (Joe, 2026-10-19; testers asked). Numbers
  stay as said; "numeral …" gives digits (25, 21st, 4:30 p.m., 3.5); a
  Writing setting writes them as a style guide does (`bd-commands`
  `numbers.rs`). List mode ("bear, start a list" / "bear, end the list", or
  the bear's menu): items on lines of their own, no capital, no
  recognizer's full stop, not tidied (`stages/list.rs`); never saved. The
  site's help page has a "What you can say" guide, linked from Writing.
  Piper can't say "bear" well enough for Nemotron to hear it, so the list
  commands are proved by a pipeline test, not by audio.
- rinch: the bear's drag and its see-through background are rinch#1597
  (layer-shell drag waits for the frame; GPU alpha mode). beardictate pins
  its branch commit 820b811a until it's merged; then pin the merge.
- Upgrading installs download Nemotron 1120 ms (464 MB archive) through the
  first-run card at the next start. Not yet tried in the app itself: only
  the pipeline tests and `bd file --stream` here and on NICO-TOP.

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
- **Releases link our own sherpa-onnx build** (2026-10-18): v1.13.8 plus
  upstream PR #3895 (`scripts/sherpa-onnx-3895.patch`, its `sherpa-onnx/csrc`
  part only), which adds `modified_beam_search`, and so hotwords, to the
  streaming NeMo transducer (Nemotron). Stock 1.13.8 rejects it with
  "Unsupported decoding method". The C API is untouched: the same 173
  `SherpaOnnx*` exports, the same onnxruntime 1.28.2 bytes, the same imports
  (Windows: static CRT, like upstream's shared-MT build). Master is not
  usable: it added a trailing field to `SherpaOnnxOfflineRecognizerConfig`,
  which the 1.13.8 crate would misread.
  - `scripts/build-sherpa-onnx.sh linux|windows` clones v1.13.8, applies the
    patch and builds just the shared libraries into
    `$CARGO_TARGET_DIR/sherpa-onnx/v1.13.8-bd1-<linux-x64|win-x64>/lib`,
    laid out like the crate's prebuilt download, and prints that folder.
    It skips the build when the folder exists, and a failed build resumes.
    Linux builds in upstream's release container
    (`quay.io/pypa/manylinux2014_x86_64`, gcc 10, glibc 2.17), so it needs
    docker. Windows uses cargo-xwin's clang-cl toolchain
    (it needs ninja and LLVM, like the cross-build). Each takes a few minutes.
    Bump `build_id` in the script when the patch or the options change.
  - The app and `bd` turn Nemotron's beam search on only with the
    `nemotron-hotwords` feature: the stock library exits the process when
    asked for it, so it can't be tried and fallen back from. The three
    scripts below always build with it.
  - `install-linux.sh`, `package-linux.sh` and `cross-build-windows.sh` run
    it, export `SHERPA_ONNX_LIB_DIR`, and copy the sherpa-onnx and
    onnxruntime libraries from that folder, not from `target/release`.
    (A later plain `cargo build` re-runs sherpa-onnx-sys's build script,
    which puts the stock libraries back in `target/release`.)
  - A plain `cargo build`, CI and the native `package-windows.ps1` build
    still use the stock 1.13.8 download. Nemotron decodes greedily there,
    and beam search fails. To develop Nemotron hotwords:
    `export SHERPA_ONNX_LIB_DIR="$(scripts/build-sherpa-onnx.sh linux)"`.
  - Checked 2026-10-18. Parakeet Unified `bd file --cleanup off` gave
    byte-identical text with our libraries and with stock, with and
    without `--words`, on 18 clips on Linux and 5 on NICO-TOP. Nemotron 1120 ms
    int8 with `modified_beam_search` and hotwords decoded on both
    ("Berdictate" became "beardictate").
  - Drop all of this when a sherpa-onnx release includes #3895 and the
    crate is updated to it. Check that release's C API structs against the
    crate first.
- `--words FILE` turns on hotwords (`NemotronConfig::hotword_boost`, beam
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

**Released:** 0.1.18, published 2026-10-20 (site v21; 0.1.17 pruned), from main `d7a7d84`: rinch pinned to rinch#1597's merge a3de4b99 (the bear's drag and see-through glow). Windows packed and smoke-tested on NICO-TOP over SSH (install, `bd file paused.wav` with `BD_MODELS_DIR=D:\beardictate\sherpa-bd\store`, which has Nemotron 1120 ms and Bear; `cli-models` has no Nemotron; uninstall). Joe's rules for that machine: scratch on D: only, compile here, never touch WSL or the oxidegen runner's tasks.
- 0.1.17 (2026-10-19, site v20, main `c49a607`): the app sends its settings at start and on each change, so `pull.sh export --setting dialog=true` (telemetry server v3) finds dictation by mode; data from before 0.1.17 has none.
- Windows: `beardictate-0.1.18-windows-x64-setup.exe` and a `.zip`, cross-built here (`scripts/cross-build-windows.sh`) and packed with Inno Setup on NICO-TOP; unsigned; cleanup on the GPU through Vulkan (backends loaded at run time, CPU fallback), ships the VC++ runtime app-local.
- Linux: `beardictate-0.1.18-linux-x86_64.tar.gz` from `scripts/package-linux.sh`; needs glibc 2.43+.
- Telemetry server: jkbase project `beardictate-telemetry` (v3, 2026-10-19: read endpoints, settings filter), https://beardictate-telemetry.jkbase.app, code in `telemetry/` (README: deploy, `export.js`, stats). Secrets `ALPHA_KEY` (also `~/.config/beardictate-release/telemetry-key`, which the build scripts compile in) and `S3_ACCESS_KEY`/`S3_SECRET_KEY` (object-store key JKBA5E4618E888AAAFE6, bucket `telemetry`). The install `0badc0ffee…` (app `test`) is the deploy check.
- Updates: installed copies (the Windows installer, Linux install.sh) check `downloads/releases.json` every 6 h, download the newest build for their platform (Windows `Installer`, Linux `Archive`), check its SHA-256 and Ed25519 signature, and install at the next start or from "Restart to update" (`crates/bd-app/src/update.rs`). `publish.sh add` signs with `~/.config/beardictate-release/update-signing.pem` (made 2026-10-18; **back it up**: without it no installed copy accepts an update). A build updates only to builds of its channel: `stable` unless built with `BD_UPDATE_CHANNEL` (then publish with `--channel NAME`; the page hides those). Tested 2026-10-18 on Linux (nested KWin) and NICO-TOP; versions before 0.1.8 have no updater, so testers install 0.1.8 by hand once.
- New release: bump `version` in Cargo.toml, `scripts/package-linux.sh`, ask the Windows session to build the same commit, `site/publish.sh add …`, `prune <platform> 1`, deploy (Joe's OK), commit `releases.json`.
- The binaries live in `site/public/downloads/` (gitignored) and `/mnt/data/beardictate-targets/win-dist/`.

## Telemetry (testers' logs)

The server is live, and every build from 0.1.9 sends telemetry (see
"Telemetry server" above; contract: `docs/telemetry.md`). Any agent reads it
with `telemetry/pull.sh stats | export <out.jsonl> [--install <id>]
[--since <date>] | audio <key> <out.flac>`, which uses the read key in
`~/.config/beardictate-release/telemetry-read-key` (server secret
`READ_KEY`, made 2026-10-19; the app's key can only send). Write exports
outside the repo: they hold testers' words. Skip the install `0badc0ffee…`
(the deploy check). A tester's own log
(`~/.local/state/beardictate/beardictate.log` on Linux) has segment numbers,
timings and joins, but never the words.

## Where things live (outside the repo)

- Models (dev): `/mnt/data/beardictate-dev/models/` —
  `asr-nemotron-speech-streaming-en-0.6b-1120ms-int8/` (hard links to the
  bake-off's copy), `test_wavs/en.wav` (which the tests read; copied from the
  Parakeet v3 release), `vad-silero/silero_vad.onnx`. The ASR
  comparison of 2026-10-15 also left v3 int8/fp32, v2 int8/fp16 and Nemotron
  streaming exports under `asr-*`.
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
- Parakeet Unified int8 CPU: RTF ≈0.07–0.1 on Joe's segments (v3 int8 was ≈0.15–0.24 on a loaded box).
