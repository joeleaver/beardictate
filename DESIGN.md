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
 (toggle)  (cpal,     (silero VAD,    (Nemotron  (deterministic:  (small local   (SendInput /
            16k mono)  utterances,     streaming  "open quote",    model, gets    libei / CGEvent,
                       fed live)       1120 ms)   "new paragraph") prior context)  or paste)
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
  (no closing `.`, `!`, `?` or `…`, nor a break) waits for the next one and
  is cleaned joined to it: 1.5 s after it came in, or, while the next
  utterance is already being spoken (Nemotron says so), until that one ends,
  at most 6 s (`sentence_hold`, `sentence_hold_speaking`; 2026-10-19: the
  old flat 8 s, restarting with every unfinished segment, could keep text
  off the page for as long as you talked). A dictated paragraph break or a
  long-pause paragraph is never joined across; the session ending, or the
  wait running out, sends the half sentence on alone. "Scratch that" while
  it waits drops just that half. With words typed as you speak there is
  nothing to hold: each utterance is finished alone.

### 3. ASR
- **The app runs only Nemotron Speech Streaming 0.6B English, 1120 ms**
  (int8, `asr-nemotron-speech-streaming-en-0.6b-1120ms-int8`, required;
  Joe, 2026-10-19; see "Where words go"). Live with beam search and
  hotwords on 8 CPU threads: 0.20 of real time on NICO-TOP's Ryzen 7 6800H,
  done 0.2 s after the speech, 12–13 of 22 special words (the 560 ms export
  of 0.1.8–0.1.9: 0.33 and 11; Parakeet Unified offline: 16). Native
  punctuation and capitalization, no silence hallucination (a transducer,
  not autoregressive like Whisper). English only.
- History: **Parakeet Unified 0.6B English** (offline, a segment at a time)
  was the recognizer in 0.1.5–0.1.9; it replaced Parakeet TDT 0.6B v3 int8
  (0.1.0–0.1.4), whose int8 export lost whole sentences on real microphone
  audio. Both are `retired` in the manifest, and their code is gone.
- Pluggable `Asr`/`StreamingAsr` traits, so another recognizer (Whisper for
  other languages) can be added.

### 4. Command pass (deterministic, before the LLM)
Spoken commands are matched by rules, not left to the model, so they are
*always* right: `open quote` / `close quote` / `quote … end quote`,
`new paragraph`, `new line`, `scratch that` (drop the previous segment; if it
was already typed in this dictation, erase it with Backspace: the user is
talking, not typing, so the cursor is still after it. Repeats reach further
back. Text from an earlier dictation is left alone; the cursor may have
moved. Joe, 2026-10-15). The LLM sees commands already applied as literal marks.

### 5. Cleanup LLM
Input: the raw segment + *our own* recent output (kept in memory — we don't
read the target app) so it knows who's talking and whether we're mid-quote.
Output: the cleaned segment only.

The context is time-windowed (2026-10-19), because a line dictated now in a
chat app must not be read against a story from an hour ago, and stale
dialog in the context made the model quote lines that weren't speech:
- only what was typed in the last 90 s (`context_window`), at most 600
  characters, cut at a sentence (`cleanup_context_chars`; the Bear models
  were trained on up to 900);
- a new passage (no context, no open quotation, "scratch that" can't reach
  back) starts after 60 s with no speech (`passage_gap`), and when the
  segment is dictated into another app than the last one (`Parts::focus`,
  `bd_platform::FocusProbe`: the foreground process on Windows and X11; on
  Wayland no app may know, so only the time rules apply);
- in the Bear Pad the context is the pad's own text, two paragraphs.

The model is warmed up at load with one short request: on a CPU the first
request cost 21 s otherwise. A sentence that has waited more than 4 s for
cleanup, with more queued behind it, is typed as dictated (`BACKLOG`), so a
slow machine falls back to raw text instead of falling further behind.

Rules (enforced by prompt now, by training later): remove fillers/stutters/
false starts/noise tags; fix caps & punctuation; quote dialog with correct
dialog-tag punctuation and new paragraph per speaker; **change no other words**.

Small models sometimes start the answer by re-typing the end of the context
(Gemma 4 E2B in about one segment in ten). Before the guardrail reviews an
answer, a leading run of at least four words matching the context's last
words is cut off, keeping any paragraph break or opening quote after it,
unless the dictation itself starts with those words (`guardrail/echo.rs`).

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
| **Bear Gemma 4 E2B** (recommended) | our LoRA fine-tune of Gemma 4 E2B-it (v4), Q4_0, bear-tags-v1 prompt with the special words; hosted at huggingface.co/joeleaver/bear-gemma-4-e2b-GGUF | 2.85 GB | The default since 2026-10-16: "auto" picks it once downloaded. 54% exact, 81% quotes ok on 171 cases; narration quoted in 1 of 30; changes words in 1 case; starts a paragraph at a speaker change (21/23). Fixes misheard special words in the same call, allowed only where the heard span sounds like the name (`bd_cleanup::respell`). ~0.7 s median on the 890M. Still splits one speaker's lines too often (report: eval/research/2026-10-bear-finetune.md). |
| **Qwen3.5 4B** | stock Qwen3.5-4B, Q4_K_M, short-v2 prompt | 2.74 GB | The default until 2026-10-16. 50% exact, 59% quotes ok on 171 cases; adds quotes to narration in 1 of 30 narration-only cases (short-v1: 11 of 30); changes words in 12 cases. ~1.5 s median on a Radeon 890M. |
| **Gemma 4 E2B** | stock Gemma 4 E2B, Q4_0, short-turns-v2 prompt | 2.84 GB | The best dialog recall (49% exact, 72% quotes ok) and about 40% faster, but quotes narration in 5 of 30 and changes words in 21 cases, so not the default. llama.cpp's template engine doesn't know Gemma 4's chat format, so bd-cleanup renders it (`llama/turns.rs`). |
| *Later: a small/fast tier* | e.g. Qwen3.5-2B with example turns (1.28 GB, ~0.7 s, 36% / 51%) | ≤ 1.3 GB | Added only when a small or much faster model earns it. The Qwen3 0.6B/1.7B/4B entries were dropped from the catalog; saved settings naming them move to Automatic. |
| **Bring your own GGUF** | user's file | — | Any llama.cpp-compatible model, e.g. Gemma 4 12B (gated on HF, so we can't fetch it for them). |
| **External endpoint** | — | — | Any OpenAI-compatible server (Ollama, LM Studio, llama-server). Opt-in, with a clear "text leaves this app" note even for localhost. |

Each catalog entry ships with its own tuned prompt (prompt + sampling live in
the manifest next to the weights), so swapping models doesn't mean retuning by
hand. Custom models get the generic prompt, which the user can edit.

**Prefix reuse on recurrent models.** Each request reuses the KV cache for
the prompt prefix it shares with the previous one (~180 of ~225 tokens with
short-v2). Qwen3.5 is hybrid: its gated-delta-net layers keep one running
state, not per-token entries, so llama.cpp can't cut the cache back to the
prefix and every request decoded the whole prompt (reused 0). That's why on
Windows its CPU steady time equalled its first request. For hybrid and
recurrent models, `llama/engine.rs` now saves the recurrent state
(`state_seq_get`, `PARTIAL_ONLY`, ~53 MB for Qwen3.5 4B) just before the
last message's text, and restores it on the next request. The attention
layers' cache keeps the prefix as before. Outputs are unchanged. Medians over
45–60 sentences on the Linux dev box (load average ~30 from other jobs, so
CPU numbers are noisy), 2026-10-15:

| Model, device | Prefill before → after | Per sentence before → after |
|---|---|---|
| Qwen3.5 4B, Radeon 890M (Vulkan) | 1.29 s → 0.44 s | 3.20 s → 2.41 s |
| Qwen3.5 4B, CPU (12 threads) | 2.55 s → 1.86 s | 13.2 s → 9.4 s (load-dominated) |
| Gemma 4 E2B | already reused 203 tokens; unaffected | |

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

ASR may get the same treatment later, with a smaller menu (Whisper for
other languages); today Nemotron is the only one.

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

## Where words go

Two settings, not modes (Joe, 2026-10-19; the four modes of 0.1.8–0.1.9,
`mode = …` in settings.toml, are mapped on load: classic → app, tidy;
streaming → app; pad-streaming → pad; pad-author and the older
`bear_pad = true` → pad, tidy):

| `destination` | `tidy = true` | `tidy = false` |
|---|---|---|
| `app` (default) | typed as you speak, each sentence tidied in place when you pause | typed as you speak |
| `pad` | shown as you speak, the paragraph cleaned in place | shown as you speak |

The General page has the two as a radio group and a "Tidy up" switch; the
tray menu has them as radio items and a checkbox. There's no "type when
finished" choice: the pipeline can still type each sentence once, cleaned
(`PipelineConfig::live_typing = false`, `bd --stream --type-once`), for
apps where editing in place would misbehave, but the app doesn't use it.

**Tidying in place** (`stages/tidy.rs`, 2026-10-19). Words are typed as
spoken; each finished sentence (the utterances up to one that ends a
sentence, or an unfinished one after 1.5 s, 6 s while the next is being
spoken) goes to the cleanup model on its own thread (`stages/cleaner.rs`,
shared with the pad), with the recent context before it, while typing goes
on. When it comes back, the page is taken back to where the old and new
text first differ and typed again from there, the words typed since
included. We can't read the page, only remember what we typed, so it's
swapped only while that is sure to be what's there: every piece still
delivered in this passage (not scratched; no stop, sleep or new passage
since: a stop or sleep first waits for the tidies in progress, a new
passage drops them), typed in the last 15 s, the same app still in front
where the platform can tell (not on Wayland), and at most 400 characters to
take back. Text that didn't reach the page (left on the clipboard, typing
failed, an erase refused) starts a new passage, so nothing earlier is
rewritten.

**Voice edits in your app** (`stages/edit.rs`, 2026-10-19): the pad's
"bear, change Smith to Smythe", "bear, delete very" and "bear, undo" work on
what was typed into your app in the last 60 s of this passage: the last
match (whole words, ignoring case, capitalized like what it replaces) is
changed on the page the same way a tidy is (`stages/rewrite.rs`: same app in
front, at most 400 characters back). "bear, undo" takes back the last edit,
if nothing was typed since, and otherwise the last thing typed, like
"scratch that". An edit is handled in order after what was said before it,
and a tidy of the edited sentence still with the model is dropped, so it
can't undo the edit. "Send it" and "clear" stay dictation outside the pad.
While typing as you speak, a sentence starting "bear, …" is held back until
the utterance ends (it may be a command), with or without voice wake. As for
the pad, an edit needs "bear" heard as a name (bear, bare, bair, baer,
behr); Piper's voices are heard as "There", "Rare", so test with your own.

- **Nemotron** is fed live from the VAD in every case: a stream starts at
  speech onset with the audio from just before it and ends with the segment
  (`Segmenter::accept_live`, `bd_core::StreamingAsr`), so it only runs while
  you speak. Typed once, its text so far only says that someone is
  speaking (for the sentence hold); the final text goes through the command
  pass and cleanup as a segment. Hotwords need our sherpa-onnx build with
  sherpa-onnx#3895 (feature `nemotron-hotwords`, which the release scripts
  turn on).
- **Typing as you speak** (`stages/live_type.rs`): each update types only
  the words that can't change: not the one still being spoken, not a
  trailing run that may be a command ("new" before "paragraph"), not a
  "bear, …" sentence with voice wake on. When a later word changes how
  earlier ones read, the page is taken back to where they differ and typed
  again. An utterance's last word is typed when it ends.
- **Pauses** (`stages/listen.rs`): Nemotron only punctuates a word once it
  hears what follows, so cutting the stream at every pause put a full stop
  at every mid-sentence pause (testers, 2026-10-19). Instead the stream is
  held open over a pause and the next utterance goes on in it: the stopped
  one ends as Nemotron punctuates it once the next words come in (full
  stop, comma or nothing). After 1.5 s more silence (4 s after "and",
  "the", a comma…) it's a sentence's end: a question mark when it reads as
  one ("can you…", "how do…"), else a full stop. On Piper TTS, all seven
  mid-sentence pauses of 1.2–2.5 s came out right and 3 of 5 real
  sentence ends. The cost: a sentence's end, and so its tidying or a
  typed-once sentence, comes about 1.5 s later.
- Measured 2026-10-19, `bd file --stream --realtime` on NICO-TOP, tidy on:
  cleanup 0.3–0.5 s a sentence on its Radeon iGPU, about 2 s on its CPU
  alone with Nemotron running, never behind, so the tidied sentence lands
  about 1.2 s (GPU) or 3 s (CPU) after you stop speaking, the 0.6 s VAD wait
  included (typed once, that was the first text you saw; tidied in place,
  the words are there while you speak).

## Bear Pad

Dictation collects in a small floating pad instead of being typed, and goes
to the focused app in one piece on "bear, send it" (or the pad's Send
button).

- **Raw first, then cleaned.** The words show in the pad as they're spoken,
  in grey italics, and each finished utterance stays there raw. The cleanup model runs on the
  pad, on a thread of its own, when the speaker pauses (1.5 s for finished
  sentences, 5 s for an unfinished one): it cleans the whole paragraph being
  dictated, already-clean part included, so a later sentence can still fix
  an earlier one, with the two paragraphs before as context. A result for a
  part of the pad that changed meanwhile is dropped and redone.
  (`bd-pipeline/src/pad.rs`, `stages/pad_mode.rs`)
- **Editing by voice** (`stages/pad_voice.rs`), at the start of a segment or
  sentence: "bear, send it", "bear, clear", "bear, undo", "bear, change
  Smith to Smythe" (or spelled: "to S M Y T H E"), "bear, delete very".
  "Scratch that" takes back the last segment, or the last edit.
- **The window** is `beardictate --pad`, a process the app starts and feeds
  JSON lines on stdin (`bd-app/src/pad_window.rs`, `engine/pad.rs`): rinch
  can make only an app's one window non-activating. On Wayland it is a
  layer-shell overlay (rinch#1523; KWin, wlroots, Hyprland, niri, COSMIC;
  an ordinary window on GNOME, which lacks layer-shell); elsewhere a
  non-activating window. It floats on top, never takes focus, and is
  borderless with its own title bar to drag and edges to resize. It shows while you
  dictate or there's text in it, and remembers its size and place, across
  restarts too (`pad-window` in the state directory; rinch#1546).
- `bd file --pad` / `bd mic --pad` run the same mode from the CLI.

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
- Parakeet Unified: NVIDIA Open Model License → commercial use and
  redistribution allowed; whoever redistributes the weights must include the
  agreement and a Notice file ("Licensed by NVIDIA Corporation under the
  NVIDIA Open Model License"). We download from HF, so we don't redistribute.
- Parakeet v3: CC-BY-4.0 → attribution in About; fine to mirror.
- Qwen3: Apache-2.0 → our fine-tunes inherit it; we can host them freely.
- **Gemma: HF repos are gated** (license click-through) — can't anonymously
  download from the canonical repo. Another reason to fine-tune on Qwen3.

Rough sizes: Parakeet Unified int8 ≈ 0.65 GB, Silero VAD ≈ 2 MB, cleanup 0.6B Q8
≈ 0.65 GB, 1.7B Q8 ≈ 1.8 GB.

## Inference backends & GPU

- **ASR**: sherpa-onnx with ONNX Runtime. CPU default — Parakeet is already
  fast enough on CPU. (DirectML EP on Windows is an option later.)
- **LLM**: llama.cpp with **Vulkan** (covers AMD/NVIDIA/Intel) + **Metal**
  on macOS, falling back to the CPU. On Windows (feature `vulkan-dl`) it's
  built with `GGML_BACKEND_DL`: the backends are DLLs loaded at run time,
  `ggml-cpu-<level>.dll` for each x86 level (ggml picks the best this CPU
  runs) and `ggml-vulkan.dll`, skipped when there's no Vulkan driver or
  device; see "Windows install and first run". Linux links Vulkan directly
  (feature `vulkan`; the Vulkan loader is a package dependency). A model or
  context that doesn't fit on the GPU loads again on the CPU. llama.cpp
  puts the model on one GPU, a discrete one before an integrated one. No
  CUDA build (size; Vulkan is close enough).
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
`scripts/package-windows.ps1`, `scripts/cross-build-windows.sh` (with
`windows-vulkan-sdk.sh`, `windows-vc-runtime.sh`, `xwin/clang-cl`),
`.cargo/config.toml`, CI in `.github/workflows/ci.yml`.

**Build.** Releases cross-compile from Linux: `scripts/cross-build-windows.sh
<stage>` runs `cargo xwin` with `vulkan-dl` and collects what ships into one
folder, which `package-windows.ps1 -Stage <stage>` packs on a Windows
machine (Inno Setup). Needs LLVM (`clang-cl`, `lld-link`, `llvm-lib`,
`llvm-dlltool`), `ninja`, and Linux's Vulkan headers, SPIR-V headers and
`glslc`, from which `windows-vulkan-sdk.sh` makes a stand-in Windows Vulkan
SDK (an import library generated for `vulkan-1.dll`). Two cross-build
fixes: a `clang-cl` wrapper adds the target features (`-mavxvnni`, …) that
ggml's MSVC branch marks only with defines, and `LINK` adds the Windows
libraries cargo-xwin's CMake toolchain leaves out. A native MSVC build
(`package-windows.ps1` without `-Stage`, which CI runs on every push) builds
the same configuration and stage, with the Vulkan SDK installed; CI then
dictates the test wav with the packaged `bd` and Gemma 4 E2B on a runner
without a GPU, which exercises the backend DLLs and the CPU fallback. Natively
the static C runtime didn't even link: cmake-rs passes `/MT`, but CMake's
own runtime setting adds `/MD` and wins, while cc-rs compiles llama-cpp-sys's
wrapper `/MT` (LNK2038); cargo-xwin's toolchain sets
`CMAKE_MSVC_RUNTIME_LIBRARY`, which is why the cross build agreed.

**C runtime.** llama.cpp's DLLs pass C runtime objects to each other
(llama.dll calls `_fileno` on a `FILE*` from ggml-base.dll's `ggml_fopen`),
so they must share one runtime. Linked statically (`/MT`), each DLL has its
own, and loading a model dies on an invalid-parameter check. So the release
uses the DLL runtime (`RUSTFLAGS=-C target-feature=-crt-static`,
`LLAMA_STATIC_CRT=0`) and ships Microsoft's `vcruntime140.dll`,
`vcruntime140_1.dll` and `msvcp140.dll` next to the exes: no redistributable
installer, no admin. They must be at least as new as the MSVC headers used
(an older `msvcp140.dll`, like the 14.27 one in NICO-TOP's System32, crashes
newer STL code), and Windows loads the app's own copy first.
`windows-vc-runtime.sh` pins the package (VS 2022 CRT redist 14.44.35211,
from the release channel manifest, URL and sha256 in the script); the
license terms are in THIRD-PARTY.txt. The UCRT comes with Windows 10 and
later. `beardictate.exe` is a GUI-subsystem program (no console window);
`bd.exe` stays a console program.

**What ships** (all in one folder; Windows finds DLLs next to the exe, so no
rpath equivalent is needed): `beardictate.exe`, `bd.exe`,
`sherpa-onnx-c-api.dll`, `onnxruntime.dll`, `onnxruntime_providers_shared.dll`,
`llama.dll`, `ggml.dll`, `ggml-base.dll`, `ggml-cpu-*.dll` (x64, sse42,
sandybridge, haswell, skylakex, cannonlake, cascadelake, icelake, alderlake),
`ggml-vulkan.dll` (42 MB of compiled shaders), the VC++ runtime, the icon,
LICENSE and third-party notices. About 110 MB (a 24 MB installer), no models.

**GPU.** The only binary that imports the Vulkan loader (`vulkan-1.dll`,
which GPU drivers install) is `ggml-vulkan.dll`, loaded at run time. With no
driver it doesn't load, with no device it registers none, and cleanup runs
on the CPU in both cases; the log says "no Vulkan GPU … cleanup runs on the
CPU". A model or KV cache that doesn't fit in VRAM falls back the same way
("loading the cleanup model on the CPU instead"). The setup log line says
where it went: "35 of 35 layers on GPU". The first request after a driver
update compiles shaders (a few seconds, once); the driver caches them.

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
weights. When the speech models (Nemotron + Silero VAD, ~630 MB) are
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

**GPU measured on NICO-TOP** (2026-10-15; Ryzen 7 6800H, Radeon 680M iGPU,
RTX 3070 Ti Laptop; `bd eval`, first 20 cases of `eval/cases.jsonl`, default
prompts). "First" is the first request after loading (it also decodes the
system prompt), "then" the mean of the 20 after it:

| | Gemma 4 E2B (short-turns-v2) | Qwen3.5 4B (short-v2) |
|---|---|---|
| CPU, 0.1.1 build (baseline x86-64, no AVX2) | 34.0 s, then 4.06 s | 29.9 s, then 30.0 s |
| CPU, `vulkan-dl` build (haswell DLL, AVX2) | 3.2 s, then 1.94 s | 4.7 s, then 5.56 s |
| GPU, RTX 3070 Ti (llama.cpp's pick) | 0.25 s, then 0.19 s | 0.81 s, then 0.32 s |
| GPU, Radeon 680M only | 1.4 s, then 0.84 s | 2.9 s, then 1.90 s |

The 0.1.1 CPU build compiled llama.cpp for rustc's default x86-64 features
(SSE2): llama-cpp-sys maps the target's features to ggml's, so without AVX2
in the target nothing used it. The CPU backend DLLs fix that for the CPU
fallback too. Qwen3.5's first ≈ then: llama.cpp can't cut its recurrent
state back to the prefix a request shares with the previous one, so the
engine clears the cache and decodes the whole prompt every time. The
Radeon's very first request took 7.8 s compiling shaders. Fallbacks checked
there: no loader (a `ggml-vulkan.dll` patched to import a missing DLL), no
device (`GGML_VK_VISIBLE_DEVICES=99`), and a KV cache too big for the RTX
(`--ctx 524288`) all ran on the CPU. The installer installed, ran `bd file`
with GPU cleanup, and uninstalled cleanly.

Not done yet (in order):
- **Which GPU.** llama.cpp takes the discrete GPU; on a laptop the iGPU uses
  less power at 3–6× the latency. A setting (`GGML_VK_VISIBLE_DEVICES`
  underneath) if users ask.
- Releases are cross-built (`cross-build-windows.sh`, clang-cl) while CI
  builds natively (MSVC): the same configuration, different compilers.
  Publishing CI's artifact instead would make what's tested what ships.
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
    bd-asr/                  sherpa-onnx Nemotron, Silero VAD
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
