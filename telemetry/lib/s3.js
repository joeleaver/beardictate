// The project's object store: S3 with SigV4, path-style, region us-east-1.
// Just PUT and DELETE, which is all the audio needs.

import { createHash, createHmac } from "node:crypto";

const sha256 = (data) => createHash("sha256").update(data).digest("hex");
const hmac = (key, data) => createHmac("sha256", key).update(data).digest();

/// Each path segment percent-encoded as SigV4 wants (RFC 3986 unreserved
/// characters kept).
export function encodePath(path) {
  return path
    .split("/")
    .map((seg) => encodeURIComponent(seg).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`))
    .join("/");
}

/// The `Authorization` header (and the headers it signed) for a request.
/// `amzDate` is `YYYYMMDDTHHMMSSZ`.
export function sign({ method, host, path, query = "", headers = {}, payloadHash, accessKey, secretKey, region = "us-east-1", service = "s3", amzDate }) {
  const date = amzDate.slice(0, 8);
  const all = { ...headers, host, "x-amz-date": amzDate };
  if (service === "s3") all["x-amz-content-sha256"] = payloadHash;
  const names = Object.keys(all).map((k) => k.toLowerCase()).sort();
  const lower = Object.fromEntries(Object.entries(all).map(([k, v]) => [k.toLowerCase(), String(v).trim()]));
  const canonicalHeaders = names.map((n) => `${n}:${lower[n]}\n`).join("");
  const signedHeaders = names.join(";");
  const canonical = [method, path, query, canonicalHeaders, signedHeaders, payloadHash].join("\n");
  const scope = `${date}/${region}/${service}/aws4_request`;
  const toSign = ["AWS4-HMAC-SHA256", amzDate, scope, sha256(canonical)].join("\n");
  let key = hmac(`AWS4${secretKey}`, date);
  for (const part of [region, service, "aws4_request"]) key = hmac(key, part);
  const signature = createHmac("sha256", key).update(toSign).digest("hex");
  return {
    headers: lower,
    authorization: `AWS4-HMAC-SHA256 Credential=${accessKey}/${scope}, SignedHeaders=${signedHeaders}, Signature=${signature}`,
  };
}

export class Store {
  constructor({ endpoint, bucket, accessKey, secretKey }) {
    this.endpoint = new URL(endpoint);
    this.bucket = bucket;
    this.accessKey = accessKey;
    this.secretKey = secretKey;
  }

  async request(method, key, body, contentType) {
    const path = encodePath(key === null ? `/${this.bucket}` : `/${this.bucket}/${key}`);
    const payload = body ?? Buffer.alloc(0);
    const payloadHash = sha256(payload);
    const amzDate = new Date().toISOString().replace(/[-:]/g, "").replace(/\.\d+/, "");
    const headers = contentType ? { "content-type": contentType } : {};
    const signed = sign({ method, host: this.endpoint.host, path, headers, payloadHash, accessKey: this.accessKey, secretKey: this.secretKey, amzDate });
    const { host: _, ...sendHeaders } = signed.headers;
    const response = await fetch(new URL(path, this.endpoint), {
      method,
      headers: { ...sendHeaders, authorization: signed.authorization },
      body: method === "PUT" ? payload : undefined,
    });
    const text = await response.text();
    return { status: response.status, text };
  }

  async put(key, body, contentType) {
    const r = await this.request("PUT", key, body, contentType);
    if (r.status >= 300) throw new Error(`store PUT ${key}: ${r.status} ${r.text.slice(0, 200)}`);
  }

  async delete(key) {
    const r = await this.request("DELETE", key);
    if (r.status >= 300 && r.status !== 404) throw new Error(`store DELETE ${key}: ${r.status} ${r.text.slice(0, 200)}`);
  }

  /// Makes the bucket if it isn't there.
  async ensureBucket() {
    const r = await this.request("PUT", null);
    if (r.status >= 300 && r.status !== 409 && !/BucketAlready/.test(r.text)) {
      throw new Error(`store bucket ${this.bucket}: ${r.status} ${r.text.slice(0, 200)}`);
    }
  }
}

/// A folder standing in for the object store (local testing).
export class FolderStore {
  constructor(dir) {
    this.dir = dir;
  }

  async put(key, body) {
    const { mkdir, writeFile } = await import("node:fs/promises");
    const { dirname, join } = await import("node:path");
    const path = join(this.dir, key);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, body);
  }

  async delete(key) {
    const { rm } = await import("node:fs/promises");
    const { join } = await import("node:path");
    await rm(join(this.dir, key), { force: true });
  }

  async ensureBucket() {}
}
