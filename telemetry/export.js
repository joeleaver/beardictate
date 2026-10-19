#!/usr/bin/env node
// Dumps the telemetry straight from a RhypeDB (lib/export.js has the
// format). For the live server, use pull.sh, which asks the server's read
// endpoint instead; this is for a local database (dev.sh):
//
//   RHYPEDB_URL=http://127.0.0.1:4290 node export.js out.jsonl [--install <id>] [--since 2026-10-18]

import { createWriteStream } from "node:fs";
import { exportLines } from "./lib/export.js";

const args = process.argv.slice(2);
const opt = (name) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
};
const out = args.find((a, i) => !a.startsWith("--") && !args[i - 1]?.startsWith("--"));
if (!out) {
  console.error("usage: node export.js <out.jsonl> [--install <id>] [--since <date>]");
  process.exit(2);
}

const file = createWriteStream(out);
let lines = 0;
for await (const line of exportLines({ install: opt("install"), since: opt("since") })) {
  file.write(`${line}\n`);
  lines += 1;
}
file.end();
console.error(`${lines} lines → ${out}`);
