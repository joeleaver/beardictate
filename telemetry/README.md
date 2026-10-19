# beardictate telemetry server

Takes alpha builds' events into RhypeDB and their segment audio into the
object store. What it accepts and why: [`docs/telemetry.md`](../docs/telemetry.md).
Plain Node (no packages), the jkbase project `beardictate-telemetry`, at
`https://beardictate-telemetry.jkbase.app`.

| File | |
|---|---|
| `server.js` | the HTTP server |
| `lib/` | RhypeDB queries, event checks, audio and its budget, SigV4 for the store |
| `schema.rhype` | `Event`, `Audio`, `Install` |
| `export.js` | everything as JSONL, a line per segment (for debugging and training) |
| `dev.sh` | a local server with its own RhypeDB and an audio folder |
| `test/` | `npm test`: unit tests, and the server against a real RhypeDB |

## Locally

```
./dev.sh          # http://127.0.0.1:8787, key dev-key, data in .data/
```

The tests and `dev.sh` use the RhypeDB build in `~/dev/rhypedb`
(`RHYPEDB_BIN` to change it). To send from the app's own uploader:

```
BD_TELEMETRY_URL=http://127.0.0.1:8787 BD_TELEMETRY_KEY=dev-key \
  cargo test -p bd-app --features alpha -- --ignored round_trip
```

## Deploying (first time)

From this folder, with Joe's jkbase login:

```
jkbase project create beardictate-telemetry
jkbase secret set ALPHA_KEY=<the key alpha builds are built with>
jkbase secret set READ_KEY=<a different key, for reading back>
jkbase access-key issue --label telemetry-audio     # prints the id and secret once
jkbase secret set S3_ACCESS_KEY=<id>
jkbase secret set S3_SECRET_KEY=<secret>
jkbase deploy
curl https://beardictate-telemetry.jkbase.app/v1/health
```

Alpha builds get the same key at build time: `BD_TELEMETRY_KEY=<key>` in the
environment of `cargo build --features alpha`.

## Reading it

Agents and people alike, from the repo root:

```
telemetry/pull.sh stats
telemetry/pull.sh export <out.jsonl> [--install <id>] [--since 2026-10-19]
telemetry/pull.sh audio <audio key from the export> <out.flac>
```

It asks the server's read endpoints (`GET /v1/stats`, `/v1/export`,
`/v1/audio/…`) with the read key in
`~/.config/beardictate-release/telemetry-read-key` (the server's `READ_KEY`
secret). That key only reads, and it's never in the app: the app's key
(`ALPHA_KEY`), which every copy carries, can only send. Write exports outside
the repo: they hold testers' words. For the local server, `dev.sh`'s read
key is `dev-read-key` (`BD_TELEMETRY_URL=http://127.0.0.1:8787`,
`BD_TELEMETRY_READ_KEY_FILE`). `export.js` does the same export straight from
a local RhypeDB.
