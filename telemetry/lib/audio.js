// Segment audio: FLAC clips in the object store, an `Audio` row each, kept
// under a byte budget by deleting the oldest.

import { query, rows, scan, obj, str } from "./db.js";
import { count } from "./installs.js";

export const MAX_CLIP = 4 << 20;
const ID = /^[0-9a-f-]{8,64}$/;

/// `audio/<install>/<session>/<segment>.flac`, or null when the parts
/// aren't ids.
export function clipKey(install, session, segment) {
  if (!ID.test(install) || !ID.test(session) || !/^\d{1,12}$/.test(segment)) return null;
  return `audio/${install}/${session}/${Number(segment)}.flac`;
}

export class Clips {
  /// `store` has `put(key, body, type)` and `delete(key)`.
  constructor(store, budgetBytes) {
    this.store = store;
    this.budget = budgetBytes;
    /// Oldest first: `{id, key, install, bytes}`.
    this.clips = [];
    this.total = 0;
    this.chain = Promise.resolve();
  }

  /// Reads what's stored (at start).
  async load() {
    const all = [];
    for await (const r of scan("Audio")) all.push({ id: r.id, key: r.key, install: r.install, bytes: r.bytes ?? 0 });
    all.sort((a, b) => a.id - b.id);
    this.clips = all;
    this.total = all.reduce((n, c) => n + c.bytes, 0);
  }

  /// Stores one clip, replacing one sent before under the same key (a
  /// retried upload), then prunes to the budget. One at a time.
  save(install, session, segment, key, body) {
    const next = this.chain.then(() => this.saveNow(install, session, segment, key, body));
    this.chain = next.catch(() => {});
    return next;
  }

  async saveNow(install, session, segment, key, body) {
    await this.store.put(key, body, "audio/flac");
    const [old] = await rows(`Audio.filter(.key == ${str(key)}).limit(1)`);
    const received = new Date().toISOString();
    if (old) {
      await query(`Audio.get(${old.id}).update(${obj({ bytes: body.length, received })})`);
      const clip = this.clips.find((c) => c.id === old.id);
      if (clip) {
        this.total += body.length - clip.bytes;
        clip.bytes = body.length;
      }
      await count(install, { audio_bytes: body.length - (old.bytes ?? 0) });
    } else {
      const created = await query(`Audio.create(${obj({ key, install, session, segment: Number(segment), bytes: body.length, received })})`);
      this.clips.push({ id: created.object?.id ?? Number.MAX_SAFE_INTEGER, key, install, bytes: body.length });
      this.total += body.length;
      await count(install, { audio_clips: 1, audio_bytes: body.length });
    }
    await this.prune();
  }

  /// Deletes the oldest clips until the rest fit the budget.
  async prune() {
    while (this.total > this.budget && this.clips.length > 1) {
      const clip = this.clips.shift();
      this.total -= clip.bytes;
      try {
        await this.store.delete(clip.key);
        await query(`Audio.get(${clip.id}).delete()`);
        await count(clip.install, { audio_clips: -1, audio_bytes: -clip.bytes });
      } catch (e) {
        console.error(`pruning ${clip.key}: ${e.message}`);
      }
    }
  }
}
