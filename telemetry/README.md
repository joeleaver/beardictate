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
jkbase access-key issue --label telemetry-audio     # prints the id and secret once
jkbase secret set S3_ACCESS_KEY=<id>
jkbase secret set S3_SECRET_KEY=<secret>
jkbase deploy
curl https://beardictate-telemetry.jkbase.app/v1/health
```

Alpha builds get the same key at build time: `BD_TELEMETRY_KEY=<key>` in the
environment of `cargo build --features alpha`.

## Reading it

Agents and people alike: `./pull.sh <out.jsonl> [--install <id>] [--since 2026-10-18]`.
It takes the managed-DB key from `~/.config/beardictate/telemetry-db.env`
(see the top of `pull.sh`; Joe makes it once with `jkbase db key create`),
runs the proxy, exports, and stops the proxy. Write the export outside the
repo: it holds testers' words. By hand:

```
jkbase db proxy --project beardictate-telemetry     # RhypeDB on 127.0.0.1:4200
node export.js telemetry.jsonl [--install <id>] [--since 2026-10-18]
curl -H "Authorization: Bearer $KEY" https://beardictate-telemetry.jkbase.app/v1/stats
```

Audio is in the console's storage browser, bucket `telemetry`, under the
keys the export gives.
