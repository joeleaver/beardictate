// Everything stored, as JSONL lines: one per dictated segment, with all
// that happened to it (what was heard, the commands, the cleanup's input
// and output, what was typed, any "scratch that") and its audio's key, plus
// one per session (segment null) with its system, load, settings and log
// events. Retried uploads can store an event twice; repeats are dropped by
// (session, seq).
//
// Each segment's line carries `settings`: the app's settings when it was
// dictated (the session's last `settings` event before it; word lists left
// out), or null when the app didn't say (before 0.1.17). `settings`
// filters keep only the segments whose settings match, and their sessions'
// own lines.

import { scan, str } from "./db.js";

/// Settings left out of each segment's copy: long, and the same every time.
const BULKY = new Set(["word_groups", "special_words"]);

/// Whether `settings` has every `key: value` in `wanted` (compared as text,
/// so "true" matches true).
export function matches(settings, wanted) {
  return Object.entries(wanted).every(([k, v]) => settings != null && k in settings && String(settings[k]) === String(v));
}

/// `filters`: `{install, since, settings}` (an id, a date, `{key: value}`).
/// Lines are JSON strings.
export async function* exportLines({ install, since, settings: wanted = {} } = {}) {
  const where = [];
  if (install) where.push(`.install == ${str(install)}`);
  if (since) where.push(`.ts >= ${str(new Date(since).toISOString())}`);
  const source = where.length ? `Event.filter(${where.join(" && ")})` : "Event";

  const audio = new Map();
  for await (const a of scan(install ? `Audio.filter(.install == ${str(install)})` : "Audio")) {
    audio.set(`${a.session}/${a.segment}`, a.key);
  }

  // Every settings event, also from before `since`: a session's mode can be
  // set before the window starts.
  const settingsBySession = new Map();
  const settingsSource = install ? `Event.filter(.kind == "settings" && .install == ${str(install)})` : `Event.filter(.kind == "settings")`;
  for await (const e of scan(settingsSource)) {
    const list = settingsBySession.get(e.session) ?? [];
    list.push({ seq: e.seq, data: e.data });
    settingsBySession.set(e.session, list);
  }
  for (const list of settingsBySession.values()) list.sort((a, b) => a.seq - b.seq);
  const settingsAt = (session, seq) => {
    let found = null;
    for (const s of settingsBySession.get(session) ?? []) {
      if (s.seq > seq) break;
      found = s.data;
    }
    if (!found || typeof found !== "object") return found;
    return Object.fromEntries(Object.entries(found).filter(([k]) => !BULKY.has(k)));
  };

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

  for (const g of groups.values()) {
    if (g.segment === null) continue;
    const first = Math.min(...g.events.map((e) => e.seq));
    g.settings = settingsAt(g.session, first);
  }
  const filtering = Object.keys(wanted).length > 0;
  const keptSessions = new Set();
  if (filtering) {
    for (const [key, g] of groups) {
      if (g.segment === null) continue;
      if (matches(g.settings, wanted)) keptSessions.add(g.session);
      else groups.delete(key);
    }
    for (const [key, g] of groups) {
      if (g.segment === null && !keptSessions.has(g.session)) groups.delete(key);
    }
  }

  const sorted = [...groups.values()].sort(
    (a, b) => String(a.first_ts).localeCompare(String(b.first_ts)) || a.session.localeCompare(b.session) || (a.segment ?? -1) - (b.segment ?? -1),
  );
  for (const g of sorted) {
    g.events.sort((a, b) => a.seq - b.seq);
    yield JSON.stringify(g);
  }
}
