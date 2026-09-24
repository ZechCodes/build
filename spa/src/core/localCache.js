// The local cache: one IndexedDB store holding what each surface last saw.
// Surfaces paint from it while the sync layer asks the bridge for newer data.
// A suspended browser can close its connection without losing those records;
// the next operation reopens it instead of making the records disappear.
//
// Plaintext by decision (2026-08-31): E2EE protects the wire; the browser
// profile is trusted. An unavailable or persistently failing database answers
// no records and announces no writes. Transient connection failures retry once.
//
// One record per (device, entity, kind, sub-key). The device leads the key so
// two paired devices never read each other's world; the entity comes second so
// evicting one branch is a single range delete.
//
// There is no in-memory layer above the store. A mirror of the records would
// be a second copy to keep true — of what this tab wrote, what another tab
// wrote, and what eviction took away — for one frame of latency on a revisit.
// The store is read directly, and every writer announces what it changed.

import { feedWithObservations } from "./cacheFreshness.js";

const DB_NAME = "build-cache";
// v3: the cache-first client's shapes, and the write-time index the lifetime
// rules sweep. A format change is a cold start by design — the records a
// previous version wrote are not this version's shapes, and one sync pass
// refills what the reader is looking at.
const DB_VERSION = 3;
const STORE = "records";
// A board record and a row can be written in the same clock millisecond.
// Keep their order beside the timestamp without changing the timestamp used
// for cache lifetime and page-exit journal comparisons.
let lastWriteOrder = 0;
const nextWriteOrder = () => {
  const clock = globalThis.performance;
  const now = Number.isFinite(clock?.timeOrigin) ? clock.timeOrigin + clock.now() : Date.now();
  lastWriteOrder = Math.max(now, lastWriteOrder + 0.001);
  return lastWriteOrder;
};

/** The index on each record's write time. It exists so "how old is what this
 *  workspace holds" can be answered from index keys alone: a key cursor
 *  yields (`at`, record key) pairs and never the record, and a workspace's
 *  records are where the megabytes are — file bodies, patches, a
 *  working-tree diff. Reading those to look at a timestamp would clone tens
 *  of megabytes onto the main thread and discard every byte. */
const AT_INDEX = "at";

/** A persistently failing cache stands down for the session. */
let disabled = false;

let dbPromise = null;
const RECOVERY_RETRIES = 1;
const intentionalAborts = new WeakMap();

function invalidateDb(db, promise) {
  if (dbPromise !== promise) return false;
  dbPromise = null;
  try { db.close(); } catch { /* already closed by the browser */ }
  return true;
}

function openDb() {
  if (disabled || typeof indexedDB === "undefined") return Promise.resolve(null);
  if (dbPromise) return dbPromise;
  const opening = new Promise((resolve) => {
    const startOpen = (attempt) => {
      let request;
      try {
        request = indexedDB.open(DB_NAME, DB_VERSION);
      } catch (error) {
        if (connectionError(error) && attempt < RECOVERY_RETRIES) {
          startOpen(attempt + 1);
          return;
        }
        standDown(error);
        resolve(null);
        return;
      }
      request.onupgradeneeded = () => {
        const db = request.result;
        if (db.objectStoreNames.contains(STORE)) db.deleteObjectStore(STORE);
        db.createObjectStore(STORE).createIndex(AT_INDEX, "at");
      };
      request.onsuccess = () => {
        const db = request.result;
        if (disabled || dbPromise !== opening) {
          db.close();
          resolve(null);
          return;
        }
        db.onclose = () => {
          if (dbPromise === opening) dbPromise = null;
        };
        resolve(db);
      };
      request.onerror = () => {
        if (dbPromise !== opening) {
          resolve(null);
          return;
        }
        if (connectionError(request.error) && attempt < RECOVERY_RETRIES) {
          startOpen(attempt + 1);
          return;
        }
        standDown(request.error);
        resolve(null);
      };
      request.onblocked = () => {
        standDown(new Error("cache database blocked by another tab"));
        resolve(null);
      };
    };
    startOpen(0);
  });
  if (!disabled) dbPromise = opening;
  return opening;
}

function standDown(error) {
  if (!disabled) console.warn("local cache disabled for this session:", error);
  disabled = true;
  dbPromise = null;
}

/** Record why our callback aborted. A storage call can fail when its connection
 *  closes; a caller's update/merge error must never be retried as a cache fault. */
function abortForError(store, error, recoverable = false) {
  intentionalAborts.set(store.transaction, { error, recoverable });
  store.transaction.abort();
}

const connectionError = (error) => error?.name === "InvalidStateError" || error?.name === "AbortError" ||
  (error?.name === "UnknownError" && /connection to indexed database server lost/i.test(error.message));

function putOrAbort(store, record, key) {
  try {
    store.put(record, key);
    return true;
  } catch (error) {
    abortForError(store, error, connectionError(error));
    return false;
  }
}

/** One transaction, one operation, resolved when the transaction settles with
 *  what it did: whether it committed, and the result of the request `run`
 *  returned. `run` gets the store and returns an IDBRequest (or null for
 *  delete-ranges, where the transaction's own completion is the answer). */
async function transact(mode, run) {
  for (let attempt = 0; attempt <= RECOVERY_RETRIES; attempt += 1) {
    const opening = dbPromise || openDb();
    const db = await opening;
    if (!db) return { committed: false };
    const outcome = await new Promise((resolve) => {
      let request;
      try {
        const transaction = db.transaction(STORE, mode);
        request = run(transaction.objectStore(STORE));
        transaction.onabort = () => {
          const marked = intentionalAborts.get(transaction);
          const error = marked?.error || transaction.error;
          const recoverable = marked ? marked.recoverable : (!error || connectionError(error));
          resolve({ committed: false, error, recoverable });
        };
        transaction.oncomplete = () =>
          resolve({ committed: true, result: request ? request.result : undefined });
      } catch (error) {
        resolve({ committed: false, error, recoverable: connectionError(error) });
      }
    });
    if (outcome.committed) return outcome;
    if (!outcome.recoverable) {
      standDown(outcome.error);
      return outcome;
    }
    const wasCurrent = invalidateDb(db, opening);
    if (attempt === RECOVERY_RETRIES) {
      if (wasCurrent) standDown(outcome.error || new Error("cache transaction repeatedly aborted"));
      return outcome;
    }
  }
}

/** A read: what was read, or undefined when there was nothing to read from. */
const inStore = (mode, run) => transact(mode, run).then((done) => done.result);

/** A write: whether the store actually changed. Only a transaction that
 *  committed is announced — a private window that refuses IndexedDB, or a
 *  session that has stood down, would otherwise send every subscriber to
 *  re-read a record that was never written and blank a surface that was
 *  painting the right thing a frame earlier. */
const wroteStore = (run) => transact("readwrite", run).then((done) => done.committed);

/** The record key. Every part is URI-encoded so a separator inside a branch
 *  name, path, or hash cannot make one record's key a prefix of another's. */
const recordKey = ({ deviceId, entityId, kind, sub = "" }) =>
  [deviceId, entityId, kind, sub].map(encodeURIComponent).join("|");

const prefixRange = (prefix) => IDBKeyRange.bound(prefix, `${prefix}￿`, false, true);

/** The account's device list — the one record that is nobody's device. It
 *  comes from skriftapp rather than from a bridge, so it is addressed under no
 *  device and no entity. Named here because the presence read writes it and
 *  the boot paint reads it, and the two must agree on where it lives. */
export const DEVICES_ADDRESS = Object.freeze({ deviceId: "", entityId: "", kind: "devices" });

// ─── Announcements ───────────────────────────────────────────────────────────
//
// Every writer says what it changed, and a surface that holds a record hears
// it. The message is the address and nothing else: the record is already on
// disk, and only the reader knows which shape it wants out of it — so a
// listener re-reads rather than being handed a body it may not want.
//
// An address is matched part by part, so a listener names as much of the key
// as it cares about: a device, an entity, a kind, or one sub-key. An eviction
// names a prefix, and reaches every listener under it as well as every
// listener it is under.

const CHANNEL_NAME = "build-cache";

/** The encoded key parts an address names, in key order, stopping at the
 *  first one it leaves out. Leaving `sub` out means "every sub-key under this
 *  kind"; naming it, even as `""`, means that one record. */
function addressParts(address) {
  const named = [];
  for (const part of [address.deviceId, address.entityId, address.kind, address.sub]) {
    if (part === undefined || part === null) break;
    named.push(encodeURIComponent(part));
  }
  return named;
}

const keyOfParts = (parts) => parts.join("|");
const partsOfKey = (key) => (key ? String(key).split("|") : []);

const addressOfParts = (parts) => {
  const names = ["deviceId", "entityId", "kind", "sub"];
  const address = {};
  parts.forEach((part, index) => {
    if (index < names.length) address[names[index]] = decodeURIComponent(part);
  });
  return address;
};

/** Two addresses touch when neither contradicts the other on a part they both
 *  name: `dev|run-1` covers `dev|run-1|status|`, and is covered by it. */
function partsTouch(one, other) {
  const depth = Math.min(one.length, other.length);
  for (let index = 0; index < depth; index += 1) if (one[index] !== other[index]) return false;
  return true;
}

const listeners = new Set(); // { parts, listener }

let channel; // undefined until first asked for, null where there is none

/** The tab-to-tab channel, opened on the first subscription so a tab that
 *  never writes still hears. A browser without it simply has no cross-tab
 *  announcements; everything in this tab works the same. */
function cacheChannel() {
  if (channel !== undefined) return channel;
  channel = null;
  if (typeof BroadcastChannel === "undefined") return channel;
  try {
    const opened = new BroadcastChannel(CHANNEL_NAME);
    opened.onmessage = (event) => announce(partsOfKey(event?.data?.key), false);
    opened.unref?.(); // node: never hold the process open for the cache
    channel = opened;
  } catch (error) {
    console.warn("cross-tab cache announcements unavailable:", error);
  }
  return channel;
}

/** Tell every listener the change is under or over, and — for a change made
 *  here — the other tabs. A listener that throws is the caller's problem, not
 *  the writer's: the record is already stored. */
function announce(parts, broadcast = true) {
  for (const entry of [...listeners]) {
    if (!partsTouch(entry.parts, parts)) continue;
    try {
      entry.listener(addressOfParts(parts));
    } catch (error) {
      console.warn("a cache listener threw:", error);
    }
  }
  if (broadcast) cacheChannel()?.postMessage({ key: keyOfParts(parts) });
}

/** Hear every write and eviction at or under `prefixAddress`, from this tab
 *  and from every other tab on this browser profile. The listener is handed
 *  the address that changed and reads what it wants; answers the way to stop
 *  listening. */
export function subscribeCache(prefixAddress, listener) {
  const entry = { parts: addressParts(prefixAddress), listener };
  listeners.add(entry);
  cacheChannel();
  return () => listeners.delete(entry);
}

/** Read one record: `{ at, value }`, or undefined when it was never written,
 *  the cache is unavailable, or anything went wrong. */
export function readCached(address) {
  return inStore("readonly", (store) => store.get(recordKey(address)));
}

/** Read many records in one transaction — a view's first paint asks for
 *  everything it draws at once rather than a round trip per surface. Answers
 *  one slot per address, in the order asked, undefined where there is no
 *  record. */
export function readCachedMany(addresses) {
  const keys = addresses.map(recordKey);
  if (!keys.length) return Promise.resolve([]);
  const records = new Array(keys.length);
  return inStore("readonly", (store) => {
    records.fill(undefined);
    keys.forEach((key, index) => {
      const request = store.get(key);
      request.onsuccess = () => {
        records[index] = request.result;
      };
    });
    return null;
  }).then(() => records);
}

/** Every feed write goes through this transaction. A local rewrite carries
 * surviving rows' observation times; a bridge board read explicitly replaces
 * them, less any row it names as superseded — one a push wrote after the read
 * was asked, whose own record is the newer word. An undo can merge into the
 * current value inside the same transaction. */
function writeFeed(address, update, { observedFeedRows = false, supersededFeedRow } = {}) {
  const key = recordKey(address);
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    // Read and put in the same transaction: another tab can write between a
    // separate read and write, and its newer row observation must survive.
    const request = store.get(key);
    request.onsuccess = () => {
      try {
        const previous = request.result;
        const next = update(previous?.value, previous);
        if (next == null) return;
        const at = Date.now();
        const order = nextWriteOrder();
        const stamp = { at, order };
        const record = { at, order, value: feedWithObservations(next, previous, stamp, observedFeedRows, supersededFeedRow) };
        changed = putOrAbort(store, record, key);
      } catch (error) {
        abortForError(store, error);
      }
    };
    return null;
  }).then((committed) => {
    if (committed && changed) announce(partsOfKey(key));
    return Boolean(committed && changed);
  });
}

/** Write one record, stamped with when. A local UI writer may also stamp its
 * owner and edit sequence for page-exit journal ordering. */
export function writeCached(address, value, { source, sequence, observedFeedRows = false } = {}) {
  if (address.kind === "feed") return writeFeed(address, () => value, { observedFeedRows });
  if (address.kind === "bridge-update") return writeBridgeUpdate(address, value);
  const key = recordKey(address);
  const record = { at: Date.now(), order: nextWriteOrder(), value, ...(source ? { source, sequence } : {}) };
  return wroteStore((store) => {
    store.put(record, key);
    return null;
  }).then((wrote) => {
    if (wrote) announce(partsOfKey(key));
  });
}

/** An absent record differs from an older record without a generation. */
export const cachedGeneration = (record) => record ? (record.generation || 0) : -1;
export const cacheAvailable = async () => Boolean(await openDb());
const newCacheGeneration = () => globalThis.crypto?.randomUUID?.() ||
  `${Date.now()}-${Math.random()}-${Math.random()}`;
const withBridgeGeneration = (address, record) => address.kind === "bridge-update"
  ? { ...record, generation: newCacheGeneration() } : record;

/** Snapshot a bridge record before its RPC starts. An absent record gets a
 * stored token so deletion after this point cannot look absent again to the
 * request that captured it. The null value paints as unavailable. */
export function captureCachedGeneration(address) {
  const key = recordKey(address);
  let generation = -1;
  return wroteStore((store) => {
    generation = -1;
    const request = store.get(key);
    request.onsuccess = () => {
      try {
        if (request.result) {
          generation = cachedGeneration(request.result);
          return;
        }
        generation = newCacheGeneration();
        putOrAbort(store, { at: Date.now(), order: nextWriteOrder(), value: null, generation }, key);
      } catch (error) {
        abortForError(store, error);
      }
    };
    return null;
  }).then((committed) => committed ? generation : -1);
}

function writeBridgeUpdate(address, value, expectedGeneration) {
  const key = recordKey(address);
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    const request = store.get(key);
    request.onsuccess = () => {
      try {
        const current = request.result;
        if (expectedGeneration !== undefined && cachedGeneration(current) !== expectedGeneration) return;
        changed = putOrAbort(store, {
          at: Date.now(), order: nextWriteOrder(), value,
          generation: newCacheGeneration(),
        }, key);
      } catch (error) {
        abortForError(store, error);
      }
    };
    return null;
  }).then((committed) => {
    if (committed && changed) announce(partsOfKey(key));
    return Boolean(committed && changed);
  });
}

/** Commit a bridge response only if no writer changed its record since the
 * request began. The comparison and write share the same IndexedDB transaction. */
export function writeCachedIfGeneration(address, value, generation) {
  if (!(typeof generation === "string" || Number.isSafeInteger(generation))) return Promise.resolve(false);
  return writeBridgeUpdate(address, value, generation);
}

/** Merge a local undo — or a board read racing the pushes — into the current
 * feed inside the same transaction that preserves its row observation times.
 * Null leaves the feed untouched. */
export const updateCachedFeed = (address, update, options) => writeFeed(address, update, options);

/** Replay one page-exit UI edit only if it is still the newest edit. The get
 * and conditional put share a readwrite transaction, so another tab cannot
 * insert a newer record between the comparison and the write. A same-owner
 * in-flight write made just before page exit is ordered by edit sequence;
 * other writers are ordered by timestamp, with ties left untouched. */
export function writeCachedIfNewer(address, value, { at, source, sequence }) {
  if (!Number.isFinite(at) || !source || !Number.isFinite(sequence)) return Promise.resolve(false);
  const key = recordKey(address);
  let applied = false;
  return wroteStore((store) => {
    applied = false;
    const request = store.get(key);
    request.onsuccess = () => {
      const current = request.result;
      const newer = !current || (current.source === source
        ? (Number(current.sequence) || 0) < sequence
        : (Number(current.at) || 0) < at);
      if (!newer) return;
      try {
        const record = withBridgeGeneration(address, { at: Date.now(), order: nextWriteOrder(), value, source, sequence });
        applied = putOrAbort(store, record, key);
      } catch (error) {
        abortForError(store, error);
      }
    };
    return null;
  }).then((committed) => {
    if (committed && applied) announce(partsOfKey(key));
    return Boolean(committed && applied);
  });
}

/** The merges in flight, one queue per record key. */
const merges = new Map();

/**
 * Read a record, put something into it, and write it back — one writer at a
 * time under that address.
 *
 * Two writers meet on a record all the time: the sync layer folding in a page
 * while a view writes the message its reader just sent. Both read, merge and
 * write, and neither waits for the other — so the one that read first writes
 * the other's work back out of existence. The merge therefore runs UNDER the
 * address, handed the record as it stands at that moment rather than one read
 * earlier. `null` from the merge leaves the record alone.
 */
export function mergeCached(address, merge) {
  const key = recordKey(address);
  const run = async () => {
    const next = merge((await readCached(address))?.value);
    if (next) await writeCached(address, next);
  };
  const ran = (merges.get(key) || Promise.resolve()).then(run, run);
  const settled = ran.catch(() => {});
  merges.set(key, settled);
  void settled.then(() => {
    if (merges.get(key) === settled) merges.delete(key);
  });
  return ran;
}

/** Merge inside one IndexedDB readwrite transaction. The ordinary merge above
 * serializes this tab's writers; this one also keeps a monotonic value safe
 * when another tab writes the same address at the same time. `merge` is sync
 * and returns null to leave the record alone. */
export function mergeCachedAtomically(address, merge) {
  const key = recordKey(address);
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    const request = store.get(key);
    request.onsuccess = () => {
      try {
        const next = merge(request.result?.value);
        if (next == null) return;
        const record = withBridgeGeneration(address, { at: Date.now(), order: nextWriteOrder(), value: next });
        changed = putOrAbort(store, record, key);
      } catch (error) {
        abortForError(store, error);
      }
    };
    return null;
  }).then((committed) => {
    if (committed && changed) announce(partsOfKey(key));
    return Boolean(committed && changed);
  });
}

/** Drop every record one entity holds on one device — a single range delete,
 *  which is why the entity sits second in the key. */
export function evictEntity(deviceId, entityId) {
  const prefix = `${encodeURIComponent(deviceId)}|${encodeURIComponent(entityId)}|`;
  return wroteStore((store) => {
    store.delete(prefixRange(prefix));
    return null;
  }).then((wrote) => {
    if (wrote) announce([encodeURIComponent(deviceId), encodeURIComponent(entityId)]);
  });
}

/** Every entity id that holds at least one record on this device. What the
 *  sync layer diffs the live active set against to find leavers. */
export async function cachedEntityIds(deviceId) {
  const prefix = `${encodeURIComponent(deviceId)}|`;
  const keys = (await inStore("readonly", (store) => store.getAllKeys(prefixRange(prefix)))) || [];
  const ids = new Set();
  for (const key of keys) {
    const entityPart = String(key).split("|")[1];
    if (entityPart) ids.add(decodeURIComponent(entityPart));
  }
  return [...ids];
}

/** The sub-keys one entity holds under one kind — for the sync layer, whose
 *  question is "which conversations were ever warmed here". */
export async function cachedSubKeys(deviceId, entityId, kind) {
  const prefix = `${encodeURIComponent(deviceId)}|${encodeURIComponent(entityId)}|${encodeURIComponent(kind)}|`;
  const keys = (await inStore("readonly", (store) => store.getAllKeys(prefixRange(prefix)))) || [];
  return keys.map((key) => decodeURIComponent(String(key).slice(prefix.length)));
}

/** Every address under a prefix — record keys only, so nothing a workspace
 *  holds is deserialized to list what it holds. */
export async function cachedAddresses(prefixAddress) {
  const prefix = `${keyOfParts(addressParts(prefixAddress))}|`;
  const keys = (await inStore("readonly", (store) => store.getAllKeys(prefixRange(prefix)))) || [];
  return keys.map((key) => addressOfParts(partsOfKey(key)));
}

/** Every address under a prefix last written before a moment, oldest first —
 *  what the expiry sweep drops. Walked over the write-time index's keys and
 *  bounded to the stale end of it, so a sweep reads neither the bodies it is
 *  dropping nor the ones it is keeping. */
export async function cachedAddressesWrittenBefore(prefixAddress, writtenBefore) {
  const prefix = `${keyOfParts(addressParts(prefixAddress))}|`;
  const stale = [];
  await inStore("readonly", (store) => {
    stale.length = 0;
    const walk = store.index(AT_INDEX).openKeyCursor(IDBKeyRange.upperBound(writtenBefore, true));
    walk.onsuccess = () => {
      const cursor = walk.result;
      if (!cursor) return;
      const key = String(cursor.primaryKey);
      if (key.startsWith(prefix)) stale.push(addressOfParts(partsOfKey(key)));
      cursor.continue();
    };
    return null;
  });
  return stale;
}

/** Every record under an address prefix: its full address, when it was
 *  written, and its value. For the one reader whose question is about the
 *  value itself — everything that only wants addresses or ages asks above. */
export async function cachedRecords(prefixAddress) {
  const prefix = `${keyOfParts(addressParts(prefixAddress))}|`;
  let keys = [];
  let records = [];
  // The range is built inside the transaction, like every other one: a browser
  // with no `IDBKeyRange` would otherwise throw out of this module rather than
  // standing the cache down, and nothing above here is allowed to notice.
  await inStore("readonly", (store) => {
    keys = [];
    records = [];
    const range = prefixRange(prefix);
    const keyRequest = store.getAllKeys(range);
    const recordRequest = store.getAll(range);
    keyRequest.onsuccess = () => {
      keys = keyRequest.result || [];
    };
    recordRequest.onsuccess = () => {
      records = recordRequest.result || [];
    };
    return null;
  });
  return keys.map((key, index) => ({
    address: addressOfParts(partsOfKey(key)),
    at: records[index]?.at || 0,
    value: records[index]?.value,
  }));
}

/** Delete named records — one transaction, then one announcement each, so a
 *  surface holding a record that has aged out hears it go. */
export function deleteCached(addresses) {
  const keys = addresses.map(recordKey);
  if (!keys.length) return Promise.resolve();
  return wroteStore((store) => {
    for (const key of keys) store.delete(key);
    return null;
  }).then((wrote) => {
    if (!wrote) return;
    for (const key of keys) announce(partsOfKey(key));
  });
}

/** Drop the whole database. For sign-out, and for a format change. Announced
 *  as the empty address, which every listener is under: nothing anyone holds
 *  is still there. */
export function wipeCache() {
  return wroteStore((store) => {
    store.clear();
    return null;
  }).then((wrote) => {
    if (wrote) announce([]);
  });
}
