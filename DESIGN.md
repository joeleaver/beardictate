# beardictate — design sketch

Local, offline dictation for writers. Hold a hotkey, talk, and clean prose
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
- UI in rinch: lives in the tray, plus a small overlay and a settings window.

## Non-goals (for v1)

- Live word-by-word streaming into the target app (we type finished sentences).
- Editing text that's already been typed in the target app.
- Voice control of the OS / commands beyond a small dictation vocabulary.
- Translation, diarization, meeting transcription.

## The pipeline

```
 hotkey ─▶ capture ─▶ VAD/segment ─▶ ASR ─▶ command pass ─▶ cleanup LLM ─▶ injector
 (press/   (cpal,     (silero VAD,    (Parakeet  (deterministic:  (small local   (SendInput /
  hold)     16k mono)  sentence-ish    TDT 0.6B   "open quote",    model, gets    libei / CGEvent,
                       chunks)         v3 int8)   "new paragraph") prior context)  or paste)
                                          │                            │
                                          └──── overlay shows raw ─────┴─ then cleaned text
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

### 3. ASR
- **Parakeet TDT 0.6B v3** (int8 ONNX) via **sherpa-onnx**. Native
  punctuation + capitalization, no silence hallucination (transducer, not
  autoregressive like Whisper), well under real time on CPU, 25 languages.
- Pluggable `Asr` trait so Whisper large-v3-turbo (whisper.cpp) can be offered
  for languages Parakeet doesn't cover.

### 4. Command pass (deterministic, before the LLM)
Spoken commands are matched by rules, not left to the model, so they are
*always* right: `open quote` / `close quote` / `quote … end quote`,
`new paragraph`, `new line`, `scratch that` (drop previous segment, only if it
hasn't been typed yet). The LLM sees commands already applied as literal marks.

### 5. Cleanup LLM
Input: the raw segment + the last ~2 paragraphs of *our own* output (kept in
memory — we don't read the target app) so it knows who's talking and whether
we're mid-quote. Output: the cleaned segment only.

Rules (enforced by prompt now, by training later): remove fillers/stutters/
false starts/noise tags; fix caps & punctuation; quote dialog with correct
dialog-tag punctuation and new paragraph per speaker; **change no other words**.

Guardrail: diff the output against the input at the word level. If the model
added/changed words beyond the allowed edits (fillers, repeats, casing,
punctuation), fall back to the ASR text for that segment. Cheap and kills the
"LLM rewrote my sentence" failure mode.

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

### Cleanup model choices

Users choose from a catalog in settings, with a **Recommended for this
computer** badge on the one we'd default to. Every catalog entry is
auto-downloadable, which means ungated weights with a redistributable license.

| Choice | Weights | Approx. size | Who it's for |
|---|---|---|---|
| **Off** | — | 0 | ASR + spoken commands only. Weak machines, or people who only want raw dictation. |
| **Bear Small** | our fine-tune of Qwen3-0.6B, Q8_0 | ~0.65 GB | Default for CPU-only laptops. |
| **Bear Medium** | our fine-tune of Qwen3-1.7B, Q8_0 | ~1.8 GB | Default for fast CPUs or any usable GPU. |
| **Bear Large** | our fine-tune of Qwen3-4B, Q4_K_M | ~2.5 GB | Default for a discrete GPU ≥ 6 GB or Apple Silicon ≥ 16 GB. |
| **General: Qwen3 4B / 8B** | stock instruct GGUF | 2.5 / 5 GB | Until the Bear fine-tunes exist, these are the catalog. Kept later as a fallback/comparison. |
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

## Hotkey

Two modes, user's choice: **hold-to-talk** and **toggle**.

| Platform | Mechanism |
|---|---|
| Windows | Toggle: `RegisterHotKey`. Hold: `WH_KEYBOARD_LL` low-level hook (needed for key-up; `RegisterHotKey` has no release event). Hook thread must pump messages and return fast or Windows silently unhooks it. |
| macOS | `CGEventTap` (Accessibility/Input Monitoring permission). |
| Linux X11 | `XGrabKey`. |
| Linux Wayland | `org.freedesktop.portal.GlobalShortcuts` (KDE ✓, GNOME ✓ recent) — gives activated/deactivated, so hold works. |

**Provided by rinch** ([rinch#1521](https://github.com/joeleaver/rinch/issues/1521)):
press/release callbacks on the main thread, rebinding, conflict errors,
modifier-only bindings. The table above is what rinch implements underneath.
Default binding must avoid common app shortcuts; something like Right-Ctrl
hold or Ctrl+Alt+Space.

## UI (rinch)

- **Tray icon** — idle / listening / processing / error states; menu:
  start/stop, cleanup tier, open settings, quit.
- **Overlay** — small borderless, transparent, always-on-top pill near the
  cursor or a screen corner: level meter while listening, then raw text, then
  cleaned text as segments complete. **Must never take focus** or the
  keystrokes go to us instead of the target app. Rules that make this hold
  on every platform:
  - Create the overlay **once at startup and keep it mapped**. Idle =
    transparent/empty, not hidden. Show/hide is a content change, never a
    map/unmap, because on Wayland the *compositor* decides whether a newly
    mapped window gets focus.
  - **Click-through by default** (empty input region works on native
    Wayland too). Interactive only while a modifier is held, to drag it.
  - Windows/macOS/X11 get true non-activating + skip-taskbar from rinch#1519.
    Native KDE Wayland: those flags are documented no-ops (no layer-shell
    yet), so the pill may show a taskbar entry. Cosmetic, accepted
    2026-10-14. Layer-shell is a low-priority rinch follow-up.
- **Settings window** — hotkey, mode, mic, cleanup tier, models (download
  state, disk usage, delete), per-app paste overrides, start at login.
- **First-run** — mic permission/selection, pick hotkey, download ASR model
  (progress), optional cleanup model, quick test pad.

### rinch work this depends on (filed, prioritized)
rinch already has multi-window, borderless, transparent, always-on-top,
start-hidden, close-intercept (hide-to-tray), and a tray icon (ksni / tray-icon).
Missing, and being built in rinch rather than worked around here:
1. [rinch#1519](https://github.com/joeleaver/rinch/issues/1519):
   **non-activating, click-through, skip-taskbar windows**, for the overlay.
2. [rinch#1521](https://github.com/joeleaver/rinch/issues/1521):
   **global hotkeys** with press/release.
3. [rinch#1520](https://github.com/joeleaver/rinch/issues/1520):
   **tray runtime updates** (icon/tooltip/menu), checkable items,
   configurable left-click, tray-only apps.

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
| Windows | MSI (`cargo-wix`) or MSIX | **Code signing** (Azure Trusted Signing) or SmartScreen will scare users off. Autostart via HKCU `Run`. Per-user install, no admin. |
| Linux | AppImage + Flatpak | Flatpak needs portals anyway (GlobalShortcuts, RemoteDesktop) — which is the Wayland path we're using. `.deb` for convenience. |
| macOS | signed + notarized `.app` in a DMG | |

Updates: check a release feed, offer download; no silent self-update in v1.

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
    beardictate/             the rinch app: tray, overlay, settings, first-run
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
4. **rinch UI**: tray / overlay / settings / first-run, once rinch#1519 and
   rinch#1520 land.
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
