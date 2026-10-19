// Everything stored, as JSONL lines: one per dictated segment, with all
// that happened to it (what was heard, the commands, the cleanup's input
// and output, what was typed, any "scratch that") and its audio's key, plus
// one per session (segment null) with its system, load, settings and log
// events. Retried uploads can store an event twice; repeats are dropped by
// (session, seq).

import { scan, str } from "./db.js";

/// `filters`: `{install, since}` (an id, a date). Lines are JSON strings.
export async function* exportLines({ install, since } = {}) {
  const where = [];
  if (install) where.push(`.install == ${str(install)}`);
  if (since) where.push(`.ts >= ${str(new Date(since).toISOString())}`);
  const source = where.length ? `Event.filter(${where.join(" && ")})` : "Event";

  const audio = new Map();
  for await (const a of scan(install ? `Audio.filter(.install == ${str(install)})` : "Audio")) {
    audio.set(`${a.session}/${a.segment}`, a.key);
  }

  /// session/segment → the line being built.
  const groups = new Map();
  const seen = new Set();
  for await (const e of scan(source)) {
    const id = `${e.session}/${e.seq}`;
    if (seen.has(id)) continue;
    seen.add(id);
    const segment = e.segment ?? null;
    const key = `${e.session}/${segment}`;
    let g = groups.get(key);
    if (!g) {
      g = { install: e.install, session: e.session, segment, app: e.app, os: e.os, first_ts: e.ts, audio: audio.get(key) ?? null, events: [] };
      groups.set(key, g);
    }
    g.events.push({ seq: e.seq, ts: e.ts, kind: e.kind, data: e.data });
  }

  const sorted = [...groups.values()].sort(
    (a, b) => String(a.first_ts).localeCompare(String(b.first_ts)) || a.session.localeCompare(b.session) || (a.segment ?? -1) - (b.segment ?? -1),
  );
  for (const g of sorted) {
    g.events.sort((a, b) => a.seq - b.seq);
    yield JSON.stringify(g);
  }
}
