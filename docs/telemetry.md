# Alpha telemetry: what the app sends, and how

Alpha builds (cargo feature `alpha`) record what happens in each dictation and
send it to the telemetry server on jkbase, for debugging testers' problems and
for training the next cleanup model. Release builds send nothing. Joe,
2026-10-18: everything on for alpha (stated at install), audio included, an
anonymous id per install.

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
| `settings` | start, and each change | the settings file, as saved (word lists included) |
| `load` | models loaded or failed | `mode`, `asr`, `cleanup` (name or null), `template`, `gpu_layers`, `n_layer`, `n_ctx`, `threads`, `load_ms`, `error` |
| `log` | a warning or error, and llama.cpp's and the cleanup loader's messages while a model loads | `level`, `target`, `message` (at most 200 a session) |
| `state` | dictation starts, stops, wakes, sleeps | `state` (`listening`, `idle`, `asleep`, `cleaning`…) |
| `mic` | microphone opened, lost, or warned about | `device`, `event`, `detail` |
| `partial` | the streaming recognizer's text so far | `text`, `since_speech_ms` |
| `transcribed` | a segment finished | `text`, `audio_ms`, `level_dbfs`, `gain`, `pause_ms`, `asr_ms`, `recognizer` (`parakeet`/`nemotron`), `audio` (true when its FLAC is sent) |
| `commanded` | after the vocabulary and command pass | `heard`, `vocabulary` (text after special-word fixes), `commanded`, `actions` (`["new_paragraph", "open_quote", …]`), `pad_command` |
| `cleaned` | the cleanup model ran (a sentence, or a pad paragraph) | `segments`, `context`, `raw`, `names`, `model_output`, `final`, `verdict` (`accepted`/`repaired`/`fell_back`), `reason`, `respelled` `[[heard, word]]`, `removed`, `prompt_tokens`, `reused_tokens`, `generated_tokens`, `prefill_ms`, `decode_ms`, `total_ms`, `pad` (bool) |
| `typed` | text sent to the focused app | `text`, `take_back`, `erase` (characters taken back), `backend`, `outcome` (`typed`/`pasted`/`clipboard`/`failed`), `inject_ms`, `live` (typed while speaking) |
| `scratch` | "scratch that" / pad undo | `what` (`queued`/`erased`/`too_late`/`pad`), `text` |
| `pad` | a pad action | `action` (`send`/`clear`/`undo`/`change`/`delete`), `words`, `found`, `sent_chars` |
| `error` | a pipeline stage failed | `stage`, `message` |

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

- Events: RhypeDB, type `Event` (see `telemetry/schema.rhype`): the envelope
  fields as indexed columns, `data` as `Json`, `text` copied out of `data`
  for full-text search.
- Audio: the project's object store, `audio/<install>/<session>/<segment>.flac`,
  with an `Audio` row per clip (size, time). The server keeps audio under a
  budget (8 GiB by default, `AUDIO_BUDGET_GB`) by deleting the oldest clips
  and their rows; events are kept.
- `GET /v1/stats` (with the key): events and audio stored, per install.
