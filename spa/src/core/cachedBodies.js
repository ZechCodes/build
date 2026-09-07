// One shape's bodies, fetched per unit and kept.
//
// The wire carries shape — a status names its files, a page names its runs —
// and the body of each unit is fetched on its own and cached under its own
// record. Two surfaces need exactly that: the per-file diffs behind a status,
// and the activity items behind a run. Both do the same four things in the
// same order: decide what is missing, cross ONE async boundary, write what came
// back through to the local cache, and answer bodies by key.
//
// The three things a configuration has to say:
//   `addressOf(key)`      the local-cache address for that key, or null where
//                         the surface has no entity to cache under.
//   `fetchMissing(keys)`  the one wire call, given every key it must answer.
//   `valueOf(item)`       one answered item as `{ key, value }` — the value is
//                         what is cached and what `read` hands back.
//
// `ensure` is the only place that waits. `read` and `has` answer from what is
// held right now, because a paint asks them.

import { readCached, writeCached } from "./localCache.js";

export function createCachedBodies({ addressOf, fetchMissing, valueOf }) {
  const held = new Map(); // key → body
  const consulted = new Set(); // keys whose stored record has been looked at

  const storedBody = async (key) => {
    const at = addressOf(key);
    const record = at ? await readCached(at) : undefined;
    return record ? record.value : undefined;
  };

  /** Fill from the local cache every key that has never been looked up there,
   *  and answer the ones it could. A key looked up once is never looked up
   *  again: the record is this session's own write after that. */
  async function hydrate(keys) {
    const fresh = keys.filter((key) => !consulted.has(key));
    const stored = await Promise.all(fresh.map(storedBody));
    const filled = [];
    fresh.forEach((key, index) => {
      consulted.add(key);
      if (stored[index] === undefined) return;
      held.set(key, stored[index]);
      filled.push(key);
    });
    return filled;
  }

  /** The wire, then the write-through. The keys are handed over in one call:
   *  batching is the caller's decision, because only the caller knows what the
   *  verb will take. */
  async function fetchBodies(keys) {
    if (!keys.length) return [];
    const filled = [];
    for (const item of await fetchMissing(keys)) {
      const { key, value } = valueOf(item);
      const stringKey = String(key);
      held.set(stringKey, value);
      const at = addressOf(stringKey);
      if (at) writeCached(at, value); // fire and forget — a failed write is a cold revisit
      filled.push(stringKey);
    }
    return filled;
  }

  /** Hold a body for each of `keys`, and answer the keys that were filled.
   *
   *  A key never seen before is taken from the local cache when it holds one.
   *  Everything else is fetched — including a key already held, which is how a
   *  caller replaces a body it has decided is stale. */
  async function ensure(keys) {
    const wanted = [...new Set(keys.map(String))];
    if (!wanted.length) return [];
    const hydrated = new Set(await hydrate(wanted));
    const fetched = await fetchBodies(wanted.filter((key) => !hydrated.has(key)));
    return [...hydrated, ...fetched];
  }

  return {
    read: (key) => held.get(String(key)),
    has: (key) => held.has(String(key)),
    ensure,
  };
}
