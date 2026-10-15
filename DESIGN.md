# beardictate — design sketch

Local, offline dictation for writers. Press a hotkey, talk, and clean prose
lands in whatever app has focus. "Clean" means capitalized and punctuated,
fillers/stutters/false starts removed, and — the part nothing else does —
**dialog correctly quoted**, the way a novelist would type it.

Status: sketch (2026-10-14). Nothing here is built yet.

## Goals

- Works in *any* focused app (keyboard wedge), triggered by a global hotkey.
- Fully local after model download. No account, no cloud, no telemetry.
- CPU-capable; uses a GPU when present but never requires one.
- **Windows is first-class**, Linux (incl. KDE/GNOME Wayland) and macOS follow.
- Ships small: the installer contains no models; models download on demand,
  verified, resumable, and the app is useful as soon as the ASR model lands.
- UI in rinch: lives in the tray (the tray icon *is* the status display),
  plus settings and first-run windows. No floating overlay (decided 2026-10-14).

## Non-goals (for v1)

- Live word-by-word streaming into the target app (we type finished sentences).
- Editing text that's already been typed in the target app.
- Voice control of the OS / commands beyond a small dictation vocabulary.
- Translation, diarization, meeting transcription.

## The pipeline

```
 hotkey ─▶ capture ─▶ VAD/segment ─▶ ASR ─▶ command pass ─▶ cleanup LLM ─▶ injector
 (toggle)  (cpal,     (silero VAD,    (Parakeet  (deterministic:  (small local   (SendInput /
            16k mono)  sentence-ish    TDT 0.6B   "open quote",    model, gets    libei / CGEvent,
                       chunks)         v3 int8)   "new paragraph") prior context)  or paste)
                       tray icon: idle → listening → cleaning → idle (or needs-you)
```

Every stage after capture runs on a worker thread, connected by channels, so
**sentence N is cleaned while you speak sentence N+1**. That pipelining is what
makes a 1–3 s cleanup tolerable during narration.

### 1. Capture
- `cpal` (WASAPI on Windows, PipeWire/ALSA on Linux, CoreAudio on macOS),
  resampled to 16 kHz mono f32 (`rubato`).
- Ring buffer keeps ~300 ms of pre-roll so the first syllable after the hotkey
  isn't clipped.

### 2. Segmentation
- Silero VAD (via sherpa-onnx). An utterance ends on ~600 ms of silence or on
  hotkey release. Segments are what flow down the pipe.
- A pause mid-sentence splits a sentence across segments ("…and said," /
  "Oh no, he's coming."), and cleanup can't quote a line it sees apart from
  its tag. So after the command pass, a segment that doesn't end a sentence
  (no closing `.`, `!`, `?` or `…`, nor a break) waits up to 8 s for the
  next one and is cleaned joined to it. A dictated paragraph break or a
  long-pause paragraph is never joined across; the session ending, or the
  wait running out, sends the half sentence on alone. "Scratch that" while
  it waits drops just that half.

### 3. ASR
- **Parakeet TDT 0.6B v3** (int8 ONNX) via **sherpa-onnx**. Native
  punctuation + capitalization, no silence hallucination (transducer, not
  autoregressive like Whisper), well under real time on CPU, 25 languages.
- Pluggable `Asr` trait so Whisper large-v3-turbo (whisper.cpp) can be offered
  for languages Parakeet doesn't cover.

### 4. Command pass (deterministic, before the LLM)
Spoken commands are matched by rules, not left to the model, so they are
*always* right: `open quote` / `close quote` / `quote … end quote`,
`new paragraph`, `new line`, `scratch that` (drop the previous segment; if it
was already typed in this dictation, erase it with Backspace: the user is
talking, not typing, so the cursor is still after it. Repeats reach further
back. Text from an earlier dictation is left alone; the cursor may have
moved. Joe, 2026-10-15). The LLM sees commands already applied as literal marks.

### 5. Cleanup LLM
Input: the raw segment + the last ~2 paragraphs of *our own* output (kept in
memory — we don't read the target app) so it knows who's talking and whether
we're mid-quote. Output: the cleaned segment only.

Rules (enforced by prompt now, by training later): remove fillers/stutters/
false starts/noise tags; fix caps & punctuation; quote dialog with correct
dialog-tag punctuation and new paragraph per speaker; **change no other words**.

Guardrail: diff the output against the input at the word level. If the model
added/changed words beyond the allowed edits (fillers, repeats, casing,
punctuation; but not turning words into ALL CAPS), the segment doesn't get the
model's words. Cheap and kills the "LLM rewrote my sentence" failure mode.

**Repair before falling back** (Joe, 2026-10-14). Small models often get the
hard part right (quoting dialog) and then miscopy a word ("is coming" → "is
came"); throwing the whole answer away loses the quotes too. Where the model
swapped words one for one, put the dictated words back into its text and keep
its quotes, punctuation, breaks and capitals; the repaired text must pass the
guardrail again. Only when that isn't possible (added, dropped or moved words,
a lost commanded mark) fall back to the ASR text. On the eval with stock
Qwen3 4B: word accuracy 95.2% → 99.1%, words changed against the rules 66 → 0.

Model plan, from the 2026-10-14 bench (on a heavily loaded box — speed numbers
unreliable, quality findings stand):
- Stock Qwen3 1.7B/4B/8B: fine on fillers and spoken commands, **bad at
  inferring unmarked dialog**. Gemma 4 12B got all cases right but is big.
- **Plan: fine-tune a small model (Qwen3 0.6B / 1.7B, Apache-2.0) as a
  specialist.** Training data is nearly free: take properly-formatted fiction,
  strip it to lowercase-no-punctuation-no-quotes, inject synthetic disfluencies
  (fillers, repeats, false starts, `[noise]` tags), train to restore. See
  `eval/` for the seed eval cases; the eval set comes before the fine-tune.
- Runtime: **llama.cpp** via `llama-cpp-2`, GGUF weights.
- The user picks the cleanup model; see **Cleanup model choices** below.

### Paragraphs

One setting, three choices (Joe, 2026-10-14): **off** (only dictated "new
paragraph"/"new line"), **dialog** (default: the cleanup model starts a
paragraph when a different character speaks; off removes any break the model
adds that wasn't dictated), **auto** (dialog, plus a paragraph after a pause of
~3 s within one dictation session, measured from the VAD segment offsets).
Pauses beat asking a small model to spot topic shifts one sentence at a time.

### Cleanup model choices

Users choose from a catalog in settings, with a **Recommended for this
computer** badge on the one we'd default to. Every catalog entry is
auto-downloadable, which means ungated weights with a redistributable license.

| Choice | Weights | Approx. size | Who it's for |
|---|---|---|---|
| **Off** | — | 0 | ASR + spoken commands only. Weak machines, or people who only want raw dictation. |
| **Gemma 4 E2B** (recommended) | stock Gemma 4 E2B, Q4_0, short-turns-v1 prompt | 2.84 GB | The default (Joe, 2026-10-15): "auto" picks it once downloaded. 47% exact, 71% quotes ok (the best quote score), ~0.85 s median: about 40% faster than Qwen3.5-4B. llama.cpp's template engine doesn't know Gemma 4's chat format, so bd-cleanup renders it (`llama/turns.rs`). |
| **Qwen3.5 4B** | stock Qwen3.5-4B, Q4_K_M, short-v1 prompt | 2.74 GB | The most often fully right: 51% exact, 64% quotes ok on 171 cases vs Qwen3-4B's 33% / 49%, ~1.5 s median on a Radeon 890M. |
| *Later: a small/fast tier* | e.g. Qwen3.5-2B with example turns (1.28 GB, ~0.7 s, 36% / 51%), or a Bear fine-tune | ≤ 1.3 GB | Added only when a small or much faster model earns it. The Qwen3 0.6B/1.7B/4B entries and the Bear placeholders were dropped from the catalog until then; saved settings naming them move to Automatic. |
| **Bring your own GGUF** | user's file | — | Any llama.cpp-compatible model, e.g. Gemma 4 12B (gated on HF, so we can't fetch it for them). |
| **External endpoint** | — | — | Any OpenAI-compatible server (Ollama, LM Studio, llama-server). Opt-in, with a clear "text leaves this app" note even for localhost. |

Each catalog entry ships with its own tuned prompt (prompt + sampling live in
the manifest next to the weights), so swapping models doesn't mean retuning by
hand. Custom models get the generic prompt, which the user can edit.

**Picking the default: probe, then measure.**
1. *Probe* (instant, at first run): total/free RAM; ggml backend enumeration
   (each Vulkan/Metal device, discrete vs integrated, VRAM / shared memory);
   CPU cores and ISA (AVX2, AVX-512, NEON); battery vs AC.
2. *Shortlist*: drop anything that won't fit in ~60% of free memory on the
   device it would run on.
3. *Measure* (≈5 s, shown as "Tuning for your computer…" in first-run): run
   the largest shortlisted model on 3 canned eval sentences. Pick the largest
   model whose p95 latency for a ~40-token sentence is **≤ 1.5 s**; otherwise
   step down. Benchmarks beat heuristics: an 890M iGPU, a 4 GB GTX 1650 and a
   16-core desktop CPU defy any simple rule.
4. Re-runnable from settings ("Re-check my hardware"). On battery, optionally
   drop one size.

Only the chosen model downloads at first run. A small built-in benchmark
model isn't needed, because step 3 runs after the shortlisted model downloads,
and the app is already usable in ASR-only mode while that happens.

ASR gets the same treatment, with a smaller menu: Parakeet v3 (default),
Moonshine base (very weak hardware), Whisper large-v3-turbo (languages
Parakeet lacks).

### 6. Injector
Per-platform `Injector` trait, two strategies each: **type** and **paste**.

| Platform | Type | Paste fallback | Notes |
|---|---|---|---|
| Windows | `SendInput` + `KEYEVENTF_UNICODE` | clipboard + Ctrl+V, then restore clipboard | UIPI: can't inject into elevated windows unless we're elevated — detect and tell the user |
| Linux X11 | XTest | clipboard + Ctrl+V | |
| Linux Wayland | libei via RemoteDesktop portal | `wl-copy` + virtual paste | wtype fails on KWin; ydotool needs uinput perms — last resort |
| macOS | `CGEventKeyboardSetUnicodeString` | NSPasteboard + Cmd+V | needs Accessibility permission (onboarding step) |

Unicode typing handles curly quotes and em-dashes; paste is for apps that drop
synthetic unicode keystrokes (some games, RDP, a few Electron apps). Per-app
override list in settings ("always paste in X").

## Always listening (default)

New installs start in **always listening** mode (Joe, 2026-10-15). The
microphone stays on; typing switches on with "bear, wake up" or the hotkey
and off with "bear, go to sleep" or the hotkey. While asleep, speech is
transcribed and dropped: nothing is typed and nothing leaves the app. Only
the tray menu turns the microphone itself off and on, and turning it on
also wakes (you asked for the mic because you're about to talk). A
microphone that fails to open, or disappears, is recorded as off, so the
menu offers to turn it back on. Waking after more than a minute asleep
starts a new passage (no cleanup context from before).

Phrases are matched on the ASR text, ignoring case and punctuation, with
the spellings the recognizer uses for "bear" (bare, bair, …). A wake phrase
counts anywhere in a segment and what follows it is dictation; a sleep
phrase only at the end of one, and what comes before it is still typed.
Sound-alikes the recognizer sometimes writes instead ("Where? Go to
sleep.") count only when the phrase is the whole segment; a hotword boost
didn't fix that mishearing. So a bear in the story told to go to sleep
mid-sentence doesn't stop dictation.

With the mode off, the hotkey works as below: it opens the microphone and
starts dictating, then stops both.

**Starting the app starts dictating**, in either mode (Joe, 2026-10-15):
once the models load, the microphone comes on and typing is awake. The
tray shows a violet dot while models load: nothing to do but wait.

## Hotkey

**Toggle by default** (press to start, press again to stop; long passages
without holding a key). Hold-to-talk stays available as a setting, since
rinch#1521 reports release anyway.

| Platform | Mechanism |
|---|---|
| Windows | Toggle: `RegisterHotKey`. Hold: `WH_KEYBOARD_LL` low-level hook (needed for key-up; `RegisterHotKey` has no release event). Hook thread must pump messages and return fast or Windows silently unhooks it. |
| macOS | `CGEventTap` (Accessibility/Input Monitoring permission). |
| Linux X11 | `XGrabKey`. |
| Linux Wayland | `org.freedesktop.portal.GlobalShortcuts` (KDE ✓, GNOME ✓ recent) — gives activated/deactivated, so hold works. |

**Provided by rinch** ([rinch#1521](https://github.com/joeleaver/rinch/issues/1521)):
press/release callbacks on the main thread, rebinding, conflict errors,
modifier-only bindings. The table above is what rinch implements underneath.
Default binding must avoid common app shortcuts; something like
Ctrl+Alt+Space, checked against conflicts at first run.

**Every binding is at least two keys, on every platform** (Joe,
2026-10-14). No single keys and no lone modifiers (RightCtrl, a bare
Super): the rebinding UI rejects them. One key is too easy to hit by
accident, and on the Wayland portal a lone modifier can't even tell a tap
from Right Ctrl+C (rinch's `chorded` flag is always false there).

## UI (rinch)

- **Tray icon = the status display.** States: idle / listening / cleaning /
  paused / needs-you, carried by the snout colour and a corner dot so the
  silhouette never changes. Tooltip mirrors the state ("Listening — 0:12",
  "Downloading Bear Medium 43%"). Left-click toggles dictation; right-click
  menu: start/stop, cleanup model (radio), paste-in-this-app, settings, quit.
- **No floating overlay.** Dropped 2026-10-14: it bought little and cost a
  focus-stealing risk on every platform (rinch#1519 is no longer on our
  critical path). Things the overlay would have said go to:
  - tray icon + tooltip for live state;
  - an **optional start/stop sound** (soft click, off by default) for people
    who aren't looking at the tray;
  - an **OS notification** only for events that need the user's attention:
    guardrail fallback ("typed the raw text"), elevated-window clipboard
    fallback ("press Ctrl+V"), hotkey conflict, mic lost, model missing.
    Rate-limited; never one per sentence.
    Uses rinch's notification API
    ([rinch#1522](https://github.com/joeleaver/rinch/issues/1522)).
- **Settings window** — hotkey, mode, mic, cleanup tier, models (download
  state, disk usage, delete), per-app paste overrides, start at login.
- **First-run** — mic permission/selection, pick hotkey, download ASR model
  (progress), optional cleanup model, quick test pad, and on **Windows a
  "pin the bear to your taskbar" step**: new tray icons land in the overflow
  flyout, where a status icon is useless. Show the Settings → Personalization
  → Taskbar → Other system tray icons path (and a deep link if one works).

### rinch work this depends on (filed, prioritized)
rinch already has multi-window, borderless, transparent, always-on-top,
start-hidden, close-intercept (hide-to-tray), and a tray icon (ksni / tray-icon).
Missing, and being built in rinch rather than worked around here:
1. [rinch#1521](https://github.com/joeleaver/rinch/issues/1521):
   **global hotkeys** with press/release.
2. [rinch#1520](https://github.com/joeleaver/rinch/issues/1520):
   **tray runtime updates** (icon/tooltip/menu), checkable items,
   configurable left-click, tray-only apps.

([rinch#1519](https://github.com/joeleaver/rinch/issues/1519), non-activating
windows, continues as a general rinch feature; not needed now that there's no
overlay.)

## Models: shipping without them

The installer ships **no weights**. A compiled-in **manifest** lists every
model the app knows about:

```toml
[[model]]
id       = "asr-parakeet-tdt-0.6b-v3-int8"
kind     = "asr"
files    = [{ path = "encoder.int8.onnx", sha256 = "…", size = … }, …]
sources  = ["https://huggingface.co/…/resolve/<commit>/…",   # pinned commit, not main
            "https://github.com/joeleaver/beardictate-models/releases/download/…"]
license  = "CC-BY-4.0"
required = true
```

Downloader:
- HTTP range requests into `<file>.part`, resume on restart, verify sha256,
  atomic rename. Mirror fallback in listed order.
- **Pinned revisions** (HF commit hash in the URL), so the bytes never change
  under us and the hashes stay valid.
- Stores under the platform data dir (`directories` crate):
  `%LOCALAPPDATA%\beardictate\models`, `~/.local/share/beardictate/models`,
  `~/Library/Application Support/beardictate/models`.
- **Offline install**: "Import model folder…" accepts a pre-downloaded copy,
  verified against the same hashes (for air-gapped machines / IT deploys).
- App is usable in ASR-only mode the moment the ASR model verifies; cleanup
  model downloads in the background.

Licensing notes that drive model choice:
- Parakeet v3: CC-BY-4.0 → attribution in About; fine to mirror.
- Qwen3: Apache-2.0 → our fine-tunes inherit it; we can host them freely.
- **Gemma: HF repos are gated** (license click-through) — can't anonymously
  download from the canonical repo. Another reason to fine-tune on Qwen3.

Rough sizes: Parakeet v3 int8 ≈ 0.65 GB, Silero VAD ≈ 2 MB, cleanup 0.6B Q8
≈ 0.65 GB, 1.7B Q8 ≈ 1.8 GB.

## Inference backends & GPU

- **ASR**: sherpa-onnx with ONNX Runtime. CPU default — Parakeet is already
  fast enough on CPU. (DirectML EP on Windows is an option later.)
- **LLM**: llama.cpp built with `GGML_BACKEND_DL` so backends are separate
  DLLs/.so's loaded at runtime: ship **CPU** (several ISA variants) +
  **Vulkan** (covers AMD/NVIDIA/Intel on Windows and Linux) + **Metal** on
  macOS. Probe at startup, pick the best that initializes, fall back to CPU.
  No CUDA build (size; Vulkan is close enough for a ≤2B model).
- Keep the LLM resident while the app runs (load once; first-segment latency
  matters). Unload after N minutes idle, configurable.

## Packaging & distribution

| | Format | Notes |
|---|---|---|
| Windows | Inno Setup `.exe`, per user, plus a portable zip | See "Windows install and first run" below. |
| Linux | AppImage + Flatpak | Flatpak needs portals anyway (GlobalShortcuts, RemoteDesktop) — which is the Wayland path we're using. `.deb` for convenience. |
| macOS | signed + notarized `.app` in a DMG | |

Updates: check a release feed, offer download; no silent self-update in v1.

## Windows install and first run

Decided 2026-10-15. Files: `packaging/windows/beardictate.iss`,
`scripts/package-windows.ps1`, `.cargo/config.toml`, CI in
`.github/workflows/ci.yml`.

**Build.** Cross-compiles from Linux with `cargo xwin` (needs `llvm-lib`,
`clang-cl` and `ninja` on `PATH` for llama.cpp's CMake build) or builds
natively with MSVC + CMake + Ninja. The C runtime is linked **statically**
(`+crt-static`, and `LLAMA_STATIC_CRT=1` so ggml builds with `/MT`): without
it llama.cpp's C++ needs MSVCP140/VCRUNTIME140, and an installer would have
to carry or require the VC++ redistributable. sherpa-onnx's prebuilt DLLs are
static-CRT already. `beardictate.exe` is a GUI-subsystem program (no console
window); `bd.exe` stays a console program.

**What ships** (all in one folder; Windows finds DLLs next to the exe, so no
rpath equivalent is needed): `beardictate.exe`, `bd.exe`,
`sherpa-onnx-c-api.dll`, `onnxruntime.dll`, `onnxruntime_providers_shared.dll`,
the icon, LICENSE and third-party notices. About 70 MB, no models.

**Installer: Inno Setup**, per user. Considered:
- *MSIX*: Store-friendly, clean uninstall, but every package must be signed
  even to sideload, and the container redirects `%LOCALAPPDATA%` writes into
  a package-private copy (models would land somewhere users and `bd` can't
  find), and a packaged app's global keyboard hook and `SendInput` behave the
  same but are harder to debug. Revisit for the Store later.
- *WiX / MSI* (`cargo-wix`): the enterprise format, but per-user MSIs are
  awkward (`MSIINSTALLPERUSER`, ICE validation noise), and IT deploys are
  served well enough by Inno's `/VERYSILENT` plus the offline model import.
- *Zip*: built anyway as the portable option, but no Start menu entry,
  uninstaller or upgrade-in-place.
- *Inno Setup*: one script, `PrivilegesRequired=lowest` (no UAC prompt),
  installs to `%LOCALAPPDATA%\Programs\beardictate` like VS Code's user
  setup, closes and restarts the running app on upgrade (Restart Manager),
  signs installer and uninstaller with any signtool command.

**Where things live:** app in `%LOCALAPPDATA%\Programs\beardictate`; models
in `%LOCALAPPDATA%\beardictate\models` (bd-models' default, so `bd` and the
app share them); settings and special words in
`%APPDATA%\beardictate\config\settings.toml`; the log in
`%LOCALAPPDATA%\beardictate\data\beardictate.log`.

**Start at login:** the installer's "Start beardictate when I sign in" task
(on by default) and the app's own setting write the same value,
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\beardictate`, so either
can turn it off, and Task Manager's Startup apps shows it. Uninstall removes it.

**Single instance and `--toggle`:** a per-user named pipe,
`\\.\pipe\beardictate-<user>`, created with `FILE_FLAG_FIRST_PIPE_INSTANCE`
and `PIPE_REJECT_REMOTE_CLIENTS` (the Unix socket's counterpart).

**Uninstall:** stops the app, removes the program folder, shortcuts and Run
value, then asks (default No) whether to delete settings and models too, so a
reinstall needn't download 0.7–3 GB again. A silent uninstall keeps them.

**Code signing and SmartScreen.** Unsigned, the installer gets SmartScreen's
"Windows protected your PC" (More info → Run anyway) and Smart App Control
may block it outright. Plan: **Azure Trusted Signing** (Microsoft's
managed signing, ~$10/month, no hardware token, reputation builds on the
publisher identity rather than per certificate); EV certificates no longer
skip SmartScreen, so they aren't worth the cost. Sign the exes and DLLs
before packaging and the installer after (`BD_SIGN_CMD` in the package
script). Until then, releases say how to get past SmartScreen and publish
SHA-256 sums.

**First run (cross-platform, implemented).** The installer ships no
weights. When the speech models (Parakeet + Silero VAD, ~670 MB) are
missing, the settings window opens by itself (instead of starting hidden in
the tray) with a card above every page, "One more step: the speech models",
and the engine reports "the speech models aren't downloaded yet" without a
notification:
1. One **Download** button with the size ("Resume the download" when part
   of it is on disk). A progress bar and "Downloading… 280 MB of 671 MB";
   **Cancel** keeps what arrived; a failure shows under the button.
2. When they verify, the engine reloads (`ModelsChanged`) and dictation
   works at once with cleanup off ("ASR-only usable meanwhile"); the card
   goes away.
3. Cleanup models are optional, on the Cleanup page's model manager
   (download, resume, delete); "Automatic" picks up the largest downloaded.
Downloads go through the one model manager (`crates/bd-app/src/models.rs`,
which tracks the speech models alongside the cleanup catalog); the card is
`window/setup.rs`.

**Checked on Windows 11** (Joe's laptop, 2026-10-15; the cross-built zip):
`bd models download` and `bd file` (Parakeet: 3.85 s of audio in 0.4–0.5 s;
Qwen3 0.6B cleanup on CPU ~7.5 s for that sentence, with the laptop busy);
the app's first run from a clean profile (Models page, 671 MB in ~22 s,
verified, engine loaded); single instance and `--toggle` over the pipe.
Started over SSH, outside the desktop session, the tray and hotkey fail
(no interactive window station) and the app falls back to its window, as
designed.

Not done yet (in order):
- **Vulkan on Windows.** Built CPU-only for now. A `vulkan` build links
  `vulkan-1.dll`, which only GPU drivers install, so on a machine without one
  the exe would not start. llama-cpp-2's `dynamic-backends` feature builds
  ggml with `GGML_BACKEND_DL` + `GGML_CPU_ALL_VARIANTS` and loads the backend
  DLLs from a folder at runtime (`ggml_backend_load_all_from_path`), falling
  back to CPU; the Vulkan DLL needs the Vulkan SDK at build time, so it's
  built natively in CI rather than cross-built.
- **Hardware probe + measured default** for the cleanup model (DESIGN
  "Cleanup model choices"), and offering it from the first-run card.
- The "pin the bear to the taskbar" first-run step (new tray icons land in
  the overflow flyout).
- Icon and version info embedded in the exe (shortcuts use the shipped
  `.ico` meanwhile).
- A generated list of Rust crate licenses (`cargo about`) in THIRD-PARTY.txt.
- Updates: the installer upgrades in place; an update check is still to come.

## Repo layout (proposed)

```
beardictate/
  Cargo.toml                 workspace
  crates/
    bd-core/                 pipeline, Asr/Cleanup/Injector/Hotkey traits, config
    bd-audio/                cpal capture, resample, VAD
    bd-asr/                  sherpa-onnx Parakeet (+ whisper.cpp later)
    bd-cleanup/              llama.cpp runner, prompt, word-diff guardrail
    bd-commands/             spoken command rules
    bd-platform/             text injector per OS (hotkeys come from rinch)
    bd-models/               manifest, downloader, verification, storage
    beardictate/             the rinch app: tray, settings, first-run, notifications
  eval/                      cleanup eval cases + runner (scores any model/endpoint)
  training/                  synthetic data generator + fine-tune recipe
```

`bd-core` has no UI and no platform deps, so the whole pipeline is testable
headless from a CLI (`bd-cli file.wav` → cleaned text) on any OS.

## Milestones

0. **Eval first.** Grow `eval/` to ~200 cases (narration, dialog with and
   without tags, interruptions, fillers, commands). Score stock models to get
   a baseline. *Exit: we know what "good enough" is.*
1. **Headless pipeline CLI** on Linux + Windows: wav/mic → Parakeet →
   commands → cleanup (stock Qwen3 4B for now) → stdout. Measure latency on
   an *idle* machine.
2. **Windows wedge**: hotkey (both modes, via rinch#1521) + SendInput + clipboard fallback,
   still no UI beyond a console. Dogfood in Word, Scrivener, Google Docs.
3. **Model manager**: manifest, downloader, verify, import. Installer with no
   weights.
4. **rinch UI**: tray / settings / first-run, once rinch#1520 lands.
5. **Fine-tuned cleanup model**: synthetic data → train 0.6B & 1.7B →
   must beat Gemma 4 12B on the eval set → publish weights.
6. **Linux Wayland + macOS** backends; Flatpak, DMG.
7. Signed Windows release.

## Open questions

- Name in the UI / tray: "beardictate" or "Bear Dictate"?
- Where to host model mirrors (HF org vs GitHub releases vs own CDN)?
- Does the fine-tune need per-author style memory (e.g., single vs double
  quotes, UK vs US punctuation-inside-quotes)? Probably a setting fed to the
  prompt, plus training data in both styles.
- Should context include text *around the cursor* in the target app (UI
  Automation on Windows, AT-SPI on Linux)? Better continuity, much more
  platform work and privacy surface. v1: no — our own output buffer only.
