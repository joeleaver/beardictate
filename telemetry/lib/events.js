// Reading an events batch: gzip NDJSON, one envelope per line
// (docs/telemetry.md).

import { gunzipSync } from "node:zlib";

export const MAX_COMPRESSED = 4 << 20;
export const MAX_UNCOMPRESSED = 20 << 20;

const ID = /^[0-9a-f-]{8,64}$/;
const KIND = /^[a-z_]{1,32}$/;
const TS = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/;

export class BadBatch extends Error {}

/// The batch's lines. Throws `BadBatch` when the body can't be read at all.
export function unpack(body, gzipped) {
  let text;
  try {
    text = (gzipped ? gunzipSync(body, { maxOutputLength: MAX_UNCOMPRESSED }) : body).toString("utf8");
  } catch (e) {
    throw new BadBatch(e.code === "ERR_BUFFER_TOO_LARGE" ? "too large" : "not gzip");
  }
  if (text.length > MAX_UNCOMPRESSED) throw new BadBatch("too large");
  return text.split("\n").filter((l) => l.trim());
}

/// One line as a stored `Event`, or null when it isn't an event.
export function toEvent(line, received) {
  let e;
  try {
    e = JSON.parse(line);
  } catch {
    return null;
  }
  if (typeof e !== "object" || e === null) return null;
  const { v, install, session, seq, ts, app, os, kind, segment, data } = e;
  if (!Number.isInteger(v) || v < 1) return null;
  if (typeof install !== "string" || !ID.test(install)) return null;
  if (typeof session !== "string" || !ID.test(session)) return null;
  if (!Number.isInteger(seq) || seq < 0) return null;
  if (typeof ts !== "string" || !TS.test(ts) || Number.isNaN(Date.parse(ts))) return null;
  if (typeof kind !== "string" || !KIND.test(kind)) return null;
  if (segment != null && !Number.isInteger(segment)) return null;
  return {
    v,
    install,
    session,
    seq,
    ts,
    received,
    app: typeof app === "string" ? app.slice(0, 64) : "",
    os: typeof os === "string" ? os.slice(0, 32) : "",
    kind,
    segment: segment ?? undefined,
    text: words(data),
    data: data && typeof data === "object" ? data : { value: data ?? null },
  };
}

/// The words an event is about, for search.
export function words(data) {
  if (!data || typeof data !== "object") return "";
  for (const k of ["final", "text", "heard", "message", "raw"]) {
    if (typeof data[k] === "string") return data[k].slice(0, 8000);
  }
  return "";
}
