# beardictate: notes for agents

Start with HANDOVER.md (where things stand) and DESIGN.md (what we're
building).

## Testers' telemetry

Every build sends what happens in each dictation to the telemetry server
(contract: `docs/telemetry.md`). To read it, from the repo root:

```
telemetry/pull.sh stats                                  # installs, versions, counts
telemetry/pull.sh export <out.jsonl> [--install <id>] [--since 2026-10-19]
telemetry/pull.sh audio <audio key from the export> <out.flac>
```

The export is a JSON line per dictated segment: what was heard, the
commands, the cleanup's before and after, what was typed, "scratch that",
and the audio's key. Each session also gets a line with `segment: null`,
holding its system, load, settings and log events; GPU and setup problems
start there. The read key is `~/.config/beardictate-release/telemetry-read-key`.
Never copy it into the repo, a command line or a message.

Exports hold testers' own words and recordings, so write them to your
scratchpad, never into the repo, and delete them when you're done. Install
`0badc0ffee…` (app `test`) is a deploy check, not a tester.
