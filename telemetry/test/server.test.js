// The server end to end, against a real RhypeDB (RHYPEDB_BIN, default the
// sibling checkout's release build) and a folder for audio. Skipped when
// there's no RhypeDB binary.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir, homedir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { createServer } from "node:net";

const here = dirname(fileURLToPath(import.meta.url));
const BIN = process.env.RHYPEDB_BIN || join(homedir(), "dev/rhypedb/target/release/rhypedb-server");
const KEY = "test-key";
const skip = !existsSync(BIN) && `no RhypeDB at ${BIN}`;

let tmp, db, server, base;

function freePort() {
  return new Promise((resolve) => {
    const s = createServer().listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

async function until(url) {
  for (let i = 0; i < 100; i++) {
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`${url} never came up`);
}

before(async () => {
  if (skip) return;
  tmp = mkdtempSync(join(tmpdir(), "bd-telemetry-"));
  const [dbPort, tcpPort, port] = [await freePort(), await freePort(), await freePort()];
  db = spawn(BIN, ["--schema", join(here, "..", "schema.rhype"), "--data-dir", join(tmp, "db"), "--listen", `127.0.0.1:${dbPort}`, "--tcp-listen", `127.0.0.1:${tcpPort}`], { stdio: ["ignore", "ignore", "inherit"] });
  await until(`http://127.0.0.1:${dbPort}/health`);
  server = spawn(process.execPath, [join(here, "..", "server.js")], {
    env: { ...process.env, PORT: String(port), ALPHA_KEY: KEY, RHYPEDB_URL: `http://127.0.0.1:${dbPort}`, AUDIO_DIR: join(tmp, "audio"), AUDIO_BUDGET_GB: String(3000 / (1 << 30)) },
    stdio: ["ignore", "ignore", "inherit"],
  });
  base = `http://127.0.0.1:${port}`;
  await until(`${base}/v1/health`);
});

const exited = (child) => (child && child.exitCode === null ? new Promise((r) => child.once("exit", r)) : Promise.resolve());

after(async () => {
  server?.kill();
  db?.kill();
  await Promise.all([exited(server), exited(db)]);
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

const install = "7f3a9c0e5b1d4f2a8c6e0b9d7a5f3e1c";
const session = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const auth = { authorization: `Bearer ${KEY}` };

function line(seq, kind, data, segment = null) {
  return JSON.stringify({ v: 1, install, session, seq, ts: "2026-10-18T16:02:11.482Z", app: "0.1.8-alpha", os: "linux", kind, segment, data });
}

async function dbQuery(q) {
  const port = db.spawnargs[db.spawnargs.indexOf("--listen") + 1];
  const r = await fetch(`http://${port}/query`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ query: q }) });
  return r.json();
}

test("events are stored, with awkward text intact", { skip }, async () => {
  const awkward = 'She said "scratch that" \\ then\nleft. ☃';
  const body = gzipSync([
    line(0, "system", { gpus: [{ name: "Vulkan0", free_mb: 7000 }], llama_backend_error: null }),
    line(1, "cleaned", { raw: "um she said", final: awkward, verdict: "accepted", respelled: [["jay kay", "jk"]] }, 3),
    "not json",
  ].join("\n"));
  const r = await fetch(`${base}/v1/events`, { method: "POST", headers: { ...auth, "content-type": "application/x-ndjson", "content-encoding": "gzip" }, body });
  assert.equal(r.status, 200);
  assert.deepEqual(await r.json(), { accepted: 2, rejected: 1 });
  const got = await dbQuery(`Event.filter(.session == "${session}" && .kind == "cleaned")`);
  assert.equal(got.objects.length, 1);
  const f = got.objects[0].fields;
  assert.equal(f.text, awkward);
  assert.equal(f.data.final, awkward);
  assert.equal(f.segment, 3);
  assert.deepEqual(f.data.respelled, [["jay kay", "jk"]]);
  const search = await dbQuery(`Event.matches(.text, "scratch", k: 5)`);
  assert.equal(search.objects.length, 1);
});

test("the key, the size and the body are checked", { skip }, async () => {
  let r = await fetch(`${base}/v1/events`, { method: "POST", headers: { authorization: "Bearer nope" }, body: "x" });
  assert.equal(r.status, 401);
  r = await fetch(`${base}/v1/events`, { method: "POST", headers: { ...auth, "content-encoding": "gzip" }, body: "not gzip" });
  assert.equal(r.status, 400);
  r = await fetch(`${base}/v1/events`, { method: "POST", headers: { ...auth, "content-encoding": "gzip" }, body: gzipSync("garbage\n") });
  assert.equal(r.status, 400);
  r = await fetch(`${base}/v1/events`, { method: "POST", headers: auth, body: Buffer.alloc((4 << 20) + 1) });
  assert.equal(r.status, 413);
  r = await fetch(`${base}/v1/stats`);
  assert.equal(r.status, 401);
});

test("audio is stored, replaced on retry, and kept under the budget", { skip }, async () => {
  const flac = (n) => Buffer.concat([Buffer.from("fLaC"), Buffer.alloc(n - 4, 1)]);
  const put = (seg, body) => fetch(`${base}/v1/audio/${install}/${session}/${seg}.flac`, { method: "PUT", headers: { ...auth, "content-type": "audio/flac" }, body });
  let r = await put("000001", flac(1000));
  assert.equal(r.status, 200);
  r = await put("000001", flac(1200));
  assert.equal(r.status, 200);
  r = await put("000002", Buffer.from("RIFFxxxx"));
  assert.equal(r.status, 400);
  r = await fetch(`${base}/v1/audio/..%2F/${session}/1.flac`, { method: "PUT", headers: auth, body: flac(100) });
  assert.equal(r.status, 400);
  // Budget is 3000 bytes: the third 1000-byte clip pushes out the first.
  for (const seg of ["000002", "000003"]) assert.equal((await put(seg, flac(1000))).status, 200);
  const dir = join(tmp, "audio", "audio", install, session);
  assert.deepEqual(readdirSync(dir).sort(), ["2.flac", "3.flac"]);
  const stats = await (await fetch(`${base}/v1/stats`, { headers: auth })).json();
  assert.equal(stats.audio.clips, 2);
  assert.equal(stats.audio.bytes, 2000);
  const mine = stats.installs.find((i) => i.install === install);
  assert.equal(mine.audio_clips, 2);
  assert.equal(mine.audio_bytes, 2000);
  assert.equal(mine.events, 2);
  assert.equal(mine.app, "0.1.8-alpha");
});

test("an install is held to 60 requests a minute", { skip }, async () => {
  const other = "0123456789abcdef0123456789abcdef";
  const body = gzipSync(JSON.stringify({ v: 1, install: other, session, seq: 0, ts: "2026-10-18T16:02:11Z", kind: "state", data: { state: "idle" } }));
  const statuses = [];
  for (let i = 0; i < 61; i++) {
    const r = await fetch(`${base}/v1/events`, { method: "POST", headers: { ...auth, "content-encoding": "gzip" }, body });
    statuses.push(r.status);
    if (r.status === 429) assert.ok(Number(r.headers.get("retry-after")) > 0);
  }
  assert.ok(statuses.slice(0, 55).every((s) => s === 200));
  assert.ok(statuses.includes(429));
});
