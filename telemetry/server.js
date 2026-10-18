// The alpha telemetry server: takes the app's event batches into RhypeDB and
// its segment audio into the object store. The contract is
// docs/telemetry.md in the beardictate repo.
//
// Environment:
//   ALPHA_KEY        the shared key alpha builds send (required)
//   PORT             default 8080
//   RHYPEDB_URL      default http://127.0.0.1:4200
//   AUDIO_BUDGET_GB  audio kept, default 8
//   S3_ENDPOINT, S3_BUCKET, S3_ACCESS_KEY, S3_SECRET_KEY
//                    the object store (endpoint default
//                    https://storage.jkbase.app, bucket "telemetry")
//   AUDIO_DIR        a folder instead of the object store (testing)

import { createServer } from "node:http";
import { timingSafeEqual } from "node:crypto";

import { createMany, rows, scan } from "./lib/db.js";
import { BadBatch, MAX_COMPRESSED, toEvent, unpack } from "./lib/events.js";
import { Clips, MAX_CLIP, clipKey } from "./lib/audio.js";
import { Limiter, count } from "./lib/installs.js";
import { FolderStore, Store } from "./lib/s3.js";

const KEY = process.env.ALPHA_KEY ?? "";
if (!KEY) {
  console.error("ALPHA_KEY is not set; every request will be refused");
}

function store() {
  if (process.env.AUDIO_DIR) return new FolderStore(process.env.AUDIO_DIR);
  const { S3_ACCESS_KEY: accessKey, S3_SECRET_KEY: secretKey } = process.env;
  if (!accessKey || !secretKey) {
    console.error("no S3_ACCESS_KEY/S3_SECRET_KEY: audio will be refused");
    return null;
  }
  return new Store({
    endpoint: process.env.S3_ENDPOINT || "https://storage.jkbase.app",
    bucket: process.env.S3_BUCKET || "telemetry",
    accessKey,
    secretKey,
  });
}

const budget = Number(process.env.AUDIO_BUDGET_GB || 8) * (1 << 30);
const objects = store();
const clips = objects ? new Clips(objects, budget) : null;
const limiter = new Limiter(60);

/// Starts once, retried by the first requests that need it (the database
/// may come up after the server).
let ready = null;
function prepare() {
  ready ??= (async () => {
    if (clips) {
      await objects.ensureBucket();
      await clips.load();
    }
  })().catch((e) => {
    ready = null;
    throw e;
  });
  return ready;
}

function send(res, status, body, headers = {}) {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  res.writeHead(status, { "content-type": typeof body === "string" ? "text/plain" : "application/json", ...headers });
  res.end(text);
}

function authorized(req) {
  const given = Buffer.from(req.headers.authorization ?? "");
  const want = Buffer.from(`Bearer ${KEY}`);
  return KEY !== "" && given.length === want.length && timingSafeEqual(given, want);
}

/// The request body, or null past `max` bytes.
function body(req, max) {
  return new Promise((resolve, reject) => {
    const parts = [];
    let size = 0;
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > max) {
        resolve(null);
        req.destroy();
        return;
      }
      parts.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(parts)));
    req.on("error", reject);
  });
}

async function postEvents(req, res) {
  if (Number(req.headers["content-length"] ?? 0) > MAX_COMPRESSED) return send(res, 413, { error: "too large" });
  const raw = await body(req, MAX_COMPRESSED);
  if (raw === null) return send(res, 413, { error: "too large" });
  const received = new Date().toISOString();
  let lines;
  try {
    lines = unpack(raw, /gzip/i.test(req.headers["content-encoding"] ?? ""));
  } catch (e) {
    if (e instanceof BadBatch) return send(res, e.message === "too large" ? 413 : 400, { error: e.message });
    throw e;
  }
  const events = lines.map((l) => toEvent(l, received)).filter(Boolean);
  if (!events.length) return send(res, 400, { error: "no events" });
  const install = events[0].install;
  const wait = limiter.take(install);
  if (wait) return send(res, 429, { error: "slow down" }, { "retry-after": String(wait) });
  await createMany("Event", events);
  const last = events[events.length - 1];
  await count(install, { app: last.app, os: last.os, events: events.length });
  send(res, 200, { accepted: events.length, rejected: lines.length - events.length });
}

async function putAudio(req, res, install, session, segment) {
  if (!clips) return send(res, 503, { error: "audio store not configured" });
  const key = clipKey(install, session, segment);
  if (!key) return send(res, 400, { error: "bad clip name" });
  const wait = limiter.take(install);
  if (wait) return send(res, 429, { error: "slow down" }, { "retry-after": String(wait) });
  if (Number(req.headers["content-length"] ?? 0) > MAX_CLIP) return send(res, 413, { error: "too large" });
  const flac = await body(req, MAX_CLIP);
  if (flac === null) return send(res, 413, { error: "too large" });
  if (flac.length < 4 || flac.subarray(0, 4).toString("latin1") !== "fLaC") return send(res, 400, { error: "not FLAC" });
  await prepare();
  await clips.save(install, session, segment, key, flac);
  send(res, 200, { stored: true });
}

async function stats(res) {
  const installs = [];
  for await (const r of scan("Install")) {
    const { id: _, ...fields } = r;
    installs.push(fields);
  }
  installs.sort((a, b) => String(b.last_seen).localeCompare(String(a.last_seen)));
  send(res, 200, {
    installs,
    events: installs.reduce((n, i) => n + (i.events ?? 0), 0),
    audio: clips ? { clips: clips.clips.length, bytes: clips.total, budget_bytes: budget } : null,
  });
}

async function handle(req, res) {
  const url = new URL(req.url, "http://x");
  const path = url.pathname;
  if (req.method === "GET" && (path === "/v1/health" || path === "/health")) {
    await rows("Install.limit(1)");
    return send(res, 200, "ok");
  }
  if (!path.startsWith("/v1/")) return send(res, 404, { error: "not found" });
  if (!authorized(req)) return send(res, 401, { error: "key" });
  if (req.method === "POST" && path === "/v1/events") return postEvents(req, res);
  const audio = path.match(/^\/v1\/audio\/([^/]+)\/([^/]+)\/([^/]+)\.flac$/);
  if (req.method === "PUT" && audio) return putAudio(req, res, audio[1], audio[2], audio[3]);
  if (req.method === "GET" && path === "/v1/stats") {
    await prepare();
    return stats(res);
  }
  send(res, 404, { error: "not found" });
}

const server = createServer((req, res) => {
  handle(req, res).catch((e) => {
    console.error(`${req.method} ${req.url}: ${e.stack ?? e}`);
    if (!res.headersSent) send(res, 503, { error: "try later" });
    else res.end();
  });
});

const port = Number(process.env.PORT || 8080);
server.listen(port, () => {
  console.log(`telemetry listening on ${port}`);
  prepare().catch((e) => console.error(`starting: ${e.message}`));
});
