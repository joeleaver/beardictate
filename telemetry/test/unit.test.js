import { test } from "node:test";
import assert from "node:assert/strict";
import { gzipSync } from "node:zlib";
import { createHash } from "node:crypto";

import { lit, obj, str } from "../lib/db.js";
import { BadBatch, toEvent, unpack, words } from "../lib/events.js";
import { clipKey } from "../lib/audio.js";
import { Limiter } from "../lib/installs.js";
import { encodePath, sign } from "../lib/s3.js";

const event = {
  v: 1,
  install: "7f3a9c0e5b1d4f2a8c6e0b9d7a5f3e1c",
  session: "a1b2c3d4e5f60718293a4b5c6d7e8f90",
  seq: 17,
  ts: "2026-10-18T16:02:11.482Z",
  app: "0.1.8-alpha",
  os: "linux",
  kind: "cleaned",
  segment: 4,
  data: { raw: "um hello", final: "Hello.", verdict: "accepted" },
};

test("strings are escaped the way the query parser reads them", () => {
  assert.equal(str('say "hi"\\ok\n\tx\r'), '"say \\"hi\\"\\\\ok\\n\\tx\\r"');
  assert.equal(str("bell\u0007 é ☃"), '"bell é ☃"');
  assert.equal(lit(null), "null");
  assert.equal(lit(3.5), "3.5");
  assert.equal(lit(NaN), "null");
  assert.equal(lit({ a: ['"'] }), '{"a":["\\""]}');
  assert.equal(obj({ a: 1, b: undefined, c: "x" }), '{ a: 1, c: "x" }');
});

test("a good line becomes an event, with its words copied out", () => {
  const e = toEvent(JSON.stringify(event), "2026-10-18T16:03:00.000Z");
  assert.equal(e.kind, "cleaned");
  assert.equal(e.segment, 4);
  assert.equal(e.text, "Hello.");
  assert.deepEqual(e.data, event.data);
  assert.equal(toEvent(JSON.stringify({ ...event, segment: null }), "").segment, undefined);
});

test("bad lines are refused", () => {
  for (const bad of [
    { install: "NOT HEX" },
    { session: "" },
    { seq: -1 },
    { seq: 1.5 },
    { ts: "yesterday" },
    { kind: "Drop Table" },
    { v: 0 },
    { segment: "4" },
  ]) {
    assert.equal(toEvent(JSON.stringify({ ...event, ...bad }), ""), null, JSON.stringify(bad));
  }
  assert.equal(toEvent("{not json", ""), null);
  assert.equal(toEvent("[1,2]", ""), null);
});

test("batches unpack, gzipped or not, and garbage is a bad batch", () => {
  const ndjson = `${JSON.stringify(event)}\n\n${JSON.stringify(event)}\n`;
  assert.equal(unpack(gzipSync(ndjson), true).length, 2);
  assert.equal(unpack(Buffer.from(ndjson), false).length, 2);
  assert.throws(() => unpack(Buffer.from("plain"), true), BadBatch);
  const bomb = gzipSync(Buffer.alloc(21 << 20, 0x20));
  assert.throws(() => unpack(bomb, true), (e) => e instanceof BadBatch && e.message === "too large");
});

test("words prefers the final text", () => {
  assert.equal(words({ text: "a", final: "b" }), "b");
  assert.equal(words({ heard: "c" }), "c");
  assert.equal(words(null), "");
});

test("clip keys only from ids", () => {
  assert.equal(clipKey(event.install, event.session, "000012"), `audio/${event.install}/${event.session}/12.flac`);
  assert.equal(clipKey("../x", event.session, "1"), null);
  assert.equal(clipKey(event.install, event.session, "1e3"), null);
});

test("the limiter allows 60 a minute per install", () => {
  const l = new Limiter(60);
  const t = 1_800_000_000_000;
  for (let i = 0; i < 60; i++) assert.equal(l.take("a", t), 0);
  assert.ok(l.take("a", t + 1000) > 0);
  assert.equal(l.take("b", t + 1000), 0);
  assert.equal(l.take("a", t + 60_000), 0);
});

test("SigV4 matches AWS's get-vanilla test vector", () => {
  const empty = createHash("sha256").update("").digest("hex");
  const { authorization } = sign({
    method: "GET",
    host: "example.amazonaws.com",
    path: "/",
    payloadHash: empty,
    accessKey: "AKIDEXAMPLE",
    secretKey: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
    service: "service",
    amzDate: "20150830T123600Z",
  });
  assert.equal(
    authorization,
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
  );
  assert.equal(encodePath("/b/audio/a b/1.flac"), "/b/audio/a%20b/1.flac");
});
