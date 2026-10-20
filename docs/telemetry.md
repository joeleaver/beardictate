# Alpha telemetry: what the app sends, and how

Alpha builds (cargo feature `alpha`) record what happens in each dictation and
send it to the telemetry server on jkbase, for debugging testers' problems and
for training the next cleanup model. Builds without the feature send nothing.
Joe, 2026-10-18: everything on for alpha (stated at install), audio included,
an anonymous id per install. Joe, 2026-10-19: every build is an alpha build
for now, the site's included; the release scripts build with `alpha` and the
key in `~/.config/beardictate-release/telemetry-key`. What users are told:
`packaging/ALPHA-NOTICE.txt` (the installer's first page, the tarball, the
download page).

Only what's said while dictating (listening, or finishing the last
sentences) is recorded: `transcribed`, `partial`, `commanded` and the audio
are dropped while always listening is asleep, so nothing said near the
microphone then leaves the computer (`telemetry::set_dictating`).

Nor is someone else's voice while awake (from 0.1.19): a sentence more than
15 dB quieter than the user's own speech (`bd-pipeline` `voice.rs`) is sent
as a `transcribed` event with `far: true`, an empty `text` and no audio, and
its live text (`partial`) is held back. Awake, the app goes back to sleep
after 10 minutes with nothing said to it (a `state` event, `unspoken_to`).
An alpha tester's room (other people, a video) was recorded for an hour and
a half on 2026-10-20 before this.

This file is the contract between the app (`crates/bd-app/src/telemetry/`)
and the server (`telemetry/`). Change both together; bump `v` when an event's
meaning changes.

## Transport

Base URL: `https://beardictate-telemetry.jkbase.app` (compiled in; `BD_TELEMETRY_URL`
overrides it for testing).

Every request carries `Authorization: Bearer <key>`, the shared alpha key
compiled into alpha builds (`BD_TELEMETRY_KEY` at build time; the server
holds it as the secret `ALPHA_KEY`). It only keeps out strangers and stray
scripts; it's in every alpha binary.

| Request | Body | Answers |
|---|---|---|
| `POST /v1/events` | gzip (`Content-Encoding: gzip`) NDJSON, one event per line, at most 4 MiB compressed, 20 MiB uncompressed | `200 {"accepted": n}` |
| `PUT /v1/audio/<install>/<session>/<segment>.flac` | FLAC, 16 kHz mono 16-bit, at most 4 MiB | `200 {"stored": true}` |
| `GET /v1/health` | | `200 ok` |

Errors: `401` (key), `413` (too big), `429` with `Retry-After` (too many
requests from this install: the server allows 60 requests a minute per
install), `400` (unreadable), `5xx` (retry later). The app retries `429` and
`5xx` with backoff, and drops a batch the server answers `400` or `413`.

## Events

One JSON object per line:

```json
{"v":1,"install":"7f…","session":"a1…","seq":17,"ts":"2026-10-18T16:02:11.482Z",
 "app":"0.1.8-alpha","os":"linux","kind":"transcribed","segment":4,"data":{…}}
```

- `install`: random UUID made at first start, kept in the state folder.
- `session`: random UUID per app run.
- `seq`: per session, from 0; with `session`, unique.
- `segment`: the pipeline's segment id, when the event is about one.
- `data`: the kind's fields, below. Times are milliseconds (`*_ms`), sizes
  bytes unless named otherwise.

| `kind` | When | `data` |
|---|---|---|
| `system` | start | `os_version`, `arch`, `cpu`, `cores`, `ram_mb`, `features` (`["vulkan-dl","nemotron-hotwords"]`…), `gpus`: `[{name, description, backend, free_mb, total_mb}]` as llama.cpp lists them |
| `settings` | start, and each change (list mode by voice too) | the settings file, as saved (word lists included), plus `list_mode`; since 8cff5bb `destination` (`app`/`pad`) and `tidy` (bool) instead of `mode`. **Not sent before 0.1.17** (the contract said so; the app didn't) |
| `load` | models loaded or failed (at start, and each reload) | `destination`, `tidy`, `asr`, `cleanup` (name, or null with tidy off), `template`, `gpu_layers`, `n_layer`, `n_ctx`, `threads` (null for a server or no cleanup), `load_ms`, `error` (null when it loaded). **Not sent before 0.1.19** |
| `log` | a warning or error, and llama.cpp's and the loaders' other messages while a model loads | `level`, `target`, `message`. At most 200 warnings and errors a session, and 100 of the loaders' other messages, counted apart (before 0.1.19, 200 of both together, which a model load used up). llama.cpp's line per layer and per tensor is left out from 0.1.19: `load` has `gpu_layers` |
| `state` | dictation starts, stops, wakes, sleeps; list mode switched by voice | `state` (`listening`, `idle`, `asleep`, `cleaning`…; `unspoken_to` with `minutes` when it goes to sleep or stops by itself, from 0.1.19), or `list_mode` (bool, from 0.1.13) |
| `mic` | microphone opened, lost, or warned about | `device`, `event`, `detail` |
| `partial` | the streaming recognizer's text so far | `text`, `since_speech_ms` |
| `transcribed` | a segment finished | `text`, `audio_ms`, `level_dbfs`, `gain`, `pause_ms`, `asr_ms` (from the utterance's start; from 0.1.11 it includes the wait to hear whether the sentence went on), `recognizer` (`nemotron`; `parakeet` only from 0.1.9 and earlier, before 8cff5bb), `audio` (true when its FLAC is sent), `far` (from 0.1.19: someone else's voice, so `text` is empty and no audio) |
| `commanded` | after the vocabulary and command pass | `heard`, `vocabulary` (text after special-word fixes), `commanded`, `actions` (`["new_paragraph", "open_quote", …]`), `pad_command` |
| `cleaned` | the cleanup model ran (a sentence, or a pad paragraph) | `segments`, `context`, `raw`, `names`, `model_output`, `final`, `verdict` (`accepted`/`repaired`/`fell_back`), `reason`, `respelled` `[[heard, word]]`, `removed`, `prompt_tokens`, `reused_tokens`, `generated_tokens`, `prefill_ms`, `decode_ms`, `total_ms`, `pad` (bool) |
| `typed` | text sent to the focused app, once it's typed (or wasn't) | `text`, `take_back` (characters taken back before it), `backend` (`windows`/`x11`/`wayland`), `outcome` (`typed`/`pasted`/`clipboard`/`failed`; `test` in a microphone test, which types nothing), `restored` (pasted: the clipboard put back), `reason` (clipboard/failed: why), `inject_ms`, `live` (typed while spoken: your app with tidy off), `pad` (the Bear Pad's text, sent; `segment` null). **Before 0.1.19 only `text`, `take_back` and `live`**, recorded before typing |
| `scratch` | "scratch that" / pad undo | `what` (`queued`/`erased`/`too_late`/`pad`), `text` |
| `pad` | a pad action | `action` (`send`/`clear`/`undo`/`change`/`delete`), `words`, `found`, `sent_chars` |
| `error` | a pipeline stage failed | `stage`, `message` |
| `update` | at start, and each check for a newer build (from 0.1.19) | `event`: `start` with `auto`, `can_update` (this copy installs updates itself), `channel`, `note` (how the last update went: "Updated to …", "Couldn't install …"), `staged` (a downloaded version waiting), `staged_failed`; or `check` with `result` (`up_to_date`/`ready`/`manual`/`failed`), `version`, `error` |

`scratch` and a `pad` undo right after a segment are the strongest signs a
segment came out wrong; keep them next to it.

## On the tester's computer

Events are appended to `telemetry/events-<n>.ndjson` in the state folder and
audio to `telemetry/audio/<session>-<segment>.flac`. The uploader sends
what's there every minute, and deletes each file once the server has it.
Caps: 20 MB of events and 200 MB of audio waiting; past them the oldest
audio goes first, then the oldest events. Nothing else is kept. The folder
is deleted when telemetry is off (a release build) and by the Windows
uninstaller.

## On the server

`telemetry/` in this repo: a dependency-free Node server (`server.js`), the
jkbase project `beardictate-telemetry`. Its README has the deploy steps.

- Events: RhypeDB, type `Event` (`telemetry/schema.rhype`): the envelope
  fields as indexed columns, `data` as `Json`, and the event's words (`final`,
  `text`, `heard`, `message` or `raw`, the first there) copied to `text` for
  full-text search. Lines that aren't events are skipped and counted
  (`{"accepted": n, "rejected": m}`); a batch with none is `400`.
- A retried batch can be stored twice; `(session, seq)` is unique, and the
  export drops repeats.
- Audio: the project's object store (bucket `telemetry`, secrets
  `S3_ACCESS_KEY`/`S3_SECRET_KEY`), `audio/<install>/<session>/<segment>.flac`,
  with an `Audio` row per clip (size, time). A clip sent again replaces the
  first. The server keeps audio under a budget (8 GiB by default,
  `AUDIO_BUDGET_GB`) by deleting the oldest clips and their rows; events are
  kept. Without the store's secrets, audio is answered `503` and stays queued
  on the tester's computer.
- `Install` rows count each install's events and audio as they arrive.
- `GET /v1/stats` (with the key): the installs, most recent first, and the
  audio stored against the budget.
- `telemetry/export.js` dumps everything as JSONL, one line per segment with
  its events in order and its audio key, plus one line per session for the
  events about no segment (`system`, `load`, `settings`, `log`).

Each segment's line also carries `settings`: the session's last `settings`
event before it, word lists left out (null before 0.1.17). `GET
/v1/export?setting=dialog=true` (repeatable; `pull.sh export --setting
dialog=true`) keeps only the segments dictated with those settings, and
their sessions' own lines; values compare as text, so any settings key
works (`tidy=false`, `destination=pad`, `list_mode=true`).
