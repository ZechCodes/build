// One shape's bodies, fetched per unit and kept.
//
// The wire carries shape — a status names its files, a page names its runs —
// and the body of each unit is fetched on its own and cached under its own
// record. Two surfaces need exactly that: the per-file diffs behind a status,
// and the activity items behind a run. Both do the same four things in the
// same order: decide what is missing, cross ONE async boundary, write what came
// back through to the local cache, then answer bodies from that stored record.
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

import { deleteCached, readCached, subscribeCache, writeCached } from "./localCache.js";

export function createCachedBodies({
  addressOf,
  fetchMissing,
  valueOf,
  cacheable = () => true,
  onChange = () => {},
}) {
  const held = new Map(); // key → body
  const direct = new Map(); // oversized response bodies, never stored (#94; paging is #95)
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
    if (record) {
      direct.delete(key);
      held.set(key, record.value);
    }
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
    if (disposed || unwatches.has(key)) return;
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
      onChange(key);
    });
    return filled;
  }

  const acceptFetched = async (item, startedAt) => {
    const { key, value } = valueOf(item);
    const stringKey = String(key);
    const at = addressOf(stringKey);
    const current = at ? await readCached(at) : undefined;
    if (at && current?.at !== startedAt.get(stringKey)) {
      await reread(stringKey);
      return stringKey;
    }
    if (!cacheable(value)) {
      // #94: paint an oversized or cut response only in this mount; #95 adds
      // pages. Remove an older record so a revisit cannot show stale content.
      if (at) {
        await deleteCached([at]);
        await rereads.get(stringKey);
      }
      if (disposed) return null;
      direct.set(stringKey, value);
      onChange(stringKey);
      return stringKey;
    }
    if (!at) {
      held.set(stringKey, value);
      onChange(stringKey);
      return stringKey;
    }
    await writeCached(at, value);
    await reread(stringKey, false);
    return stringKey;
  };

  /** The wire, then the write-through. Batching belongs to the caller. */
  async function fetchBodies(keys) {
    if (!keys.length) return [];
    const startedAt = new Map(keys.map((key) => [String(key), observedAt.get(String(key))]));
    const filled = [];
    for (const item of await fetchMissing(keys)) {
      const accepted = await acceptFetched(item, startedAt);
      if (accepted) filled.push(accepted);
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
    read: (key) => direct.get(String(key)) ?? held.get(String(key)),
    has: (key) => direct.has(String(key)) || held.has(String(key)),
    ensure,
    dispose: () => {
      disposed = true;
      for (const unwatch of unwatches.values()) unwatch();
      unwatches.clear();
    },
  };
}
