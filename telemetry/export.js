#!/usr/bin/env node
// Dumps the telemetry as JSONL, one line per dictated segment, with
// everything that happened to it: what was heard, the commands, the
// cleanup's input and output, what was typed, any "scratch that", and its
// audio's object-store key. Plus one line per session (segment null) with
// its system, load, settings and log events, for GPU and setup problems.
//
//   jkbase db proxy --project beardictate-telemetry    # in another shell
//   node export.js telemetry.jsonl [--install <id>] [--since 2026-10-18]
//
// RHYPEDB_URL overrides the database (default the proxy's 127.0.0.1:4200).
// Retried uploads can store an event twice; they're dropped here by
// (session, seq).

import { createWriteStream } from "node:fs";
import { scan, str } from "./lib/db.js";

const args = process.argv.slice(2);
const out = args.find((a) => !a.startsWith("--") && args[args.indexOf(a) - 1]?.startsWith("--") !== true);
const opt = (name) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
};
if (!out) {
  console.error("usage: node export.js <out.jsonl> [--install <id>] [--since <date>]");
  process.exit(2);
}

const filters = [];
if (opt("install")) filters.push(`.install == ${str(opt("install"))}`);
if (opt("since")) filters.push(`.ts >= ${str(new Date(opt("since")).toISOString())}`);
const source = filters.length ? `Event.filter(${filters.join(" && ")})` : "Event";

const audio = new Map();
for await (const a of scan(opt("install") ? `Audio.filter(.install == ${str(opt("install"))})` : "Audio")) {
  audio.set(`${a.session}/${a.segment}`, a.key);
}

/// session/segment → the line being built.
const groups = new Map();
const seen = new Set();
let events = 0;
for await (const e of scan(source)) {
  const id = `${e.session}/${e.seq}`;
  if (seen.has(id)) continue;
  seen.add(id);
  events += 1;
  const segment = e.segment ?? null;
  const key = `${e.session}/${segment}`;
  let g = groups.get(key);
  if (!g) {
    g = { install: e.install, session: e.session, segment, app: e.app, os: e.os, first_ts: e.ts, audio: audio.get(key) ?? null, events: [] };
    groups.set(key, g);
  }
  g.events.push({ seq: e.seq, ts: e.ts, kind: e.kind, data: e.data });
}

const file = createWriteStream(out);
const sorted = [...groups.values()].sort((a, b) => a.session.localeCompare(b.session) || (a.segment ?? -1) - (b.segment ?? -1));
for (const g of sorted) {
  g.events.sort((a, b) => a.seq - b.seq);
  file.write(`${JSON.stringify(g)}\n`);
}
file.end();
console.error(`${events} events in ${groups.size} lines → ${out}`);
