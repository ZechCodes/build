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

import { readCached, subscribeCache, writeCached } from "./localCache.js";

export function createCachedBodies({
  addressOf,
  fetchMissing,
  valueOf,
  // The activity reader still owns its in-memory delivery semantics. Git
  // bodies opt into the stricter path: a wire answer is written, announced,
  // read back, and only that stored value enters `held`.
  cacheDriven = false,
  onChange = () => {},
}) {
  const held = new Map(); // key → body
  const consulted = new Set(); // keys whose stored record has been looked at
  const observedAt = new Map(); // key → write stamp seen before a fetch
  const unwatches = new Map();
  const rereads = new Map();
  let disposed = false;

  const storedRecord = async (key) => {
    const at = addressOf(key);
    return at ? readCached(at) : undefined;
  };

  const takeRecord = (key, record) => {
    observedAt.set(key, record?.at);
    if (record) held.set(key, record.value);
    else held.delete(key);
    return Boolean(record);
  };

  /** Serialize announcement reads per body. An own write announces before its
   * promise settles; the explicit readback queues behind that announcement so
   * `ensure` cannot finish while `held` still contains the pulled object. */
  const reread = (key, notify = true) => {
    const stringKey = String(key);
    const run = async () => {
      const record = await storedRecord(stringKey);
      if (disposed) return false;
      const present = takeRecord(stringKey, record);
      if (notify) onChange(stringKey);
      return present;
    };
    const next = (rereads.get(stringKey) || Promise.resolve()).then(run, run);
    const settled = next.finally(() => {
      if (rereads.get(stringKey) === settled) rereads.delete(stringKey);
    });
    rereads.set(stringKey, settled);
    return settled;
  };

  const watch = (key) => {
    if (!cacheDriven || disposed || unwatches.has(key)) return;
    const at = addressOf(key);
    if (at) unwatches.set(key, subscribeCache(at, () => void reread(key)));
  };

  /** Fill from the local cache every key that has never been looked up there,
   *  and answer the ones it could. A key looked up once is never looked up
   *  again: the record is this session's own write after that. */
  async function hydrate(keys) {
    const fresh = keys.filter((key) => !consulted.has(key));
    fresh.forEach(watch);
    const stored = await Promise.all(fresh.map(storedRecord));
    const filled = [];
    fresh.forEach((key, index) => {
      consulted.add(key);
      if (!takeRecord(key, stored[index])) return;
      filled.push(key);
      if (cacheDriven) onChange(key);
    });
    return filled;
  }

  /** The wire, then the write-through. The keys are handed over in one call:
   *  batching is the caller's decision, because only the caller knows what the
   *  verb will take. */
  async function fetchBodies(keys) {
    if (!keys.length) return [];
    const startedAt = new Map(keys.map((key) => [String(key), observedAt.get(String(key))]));
    const filled = [];
    for (const item of await fetchMissing(keys)) {
      const { key, value } = valueOf(item);
      const stringKey = String(key);
      const at = addressOf(stringKey);
      if (!cacheDriven || !at) {
        held.set(stringKey, value);
        if (at) writeCached(at, value); // legacy/activity path stays intentionally unchanged
        onChange(stringKey);
        filled.push(stringKey);
        continue;
      }

      // A cache writer may have won while the RPC was in flight. Never put the
      // older answer over that announcement; take up the newer record instead.
      const current = await readCached(at);
      if (current?.at === startedAt.get(stringKey)) await writeCached(at, value);
      await reread(stringKey, current?.at !== startedAt.get(stringKey));
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
    dispose: () => {
      disposed = true;
      for (const unwatch of unwatches.values()) unwatch();
      unwatches.clear();
    },
  };
}
