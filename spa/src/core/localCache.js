// The local cache: one IndexedDB store holding what each surface last saw, so
// a revisit paints from disk while the bridge is still being asked. It is an
// optimization, never a source of truth — every cached paint is followed by the
// live fetch the surface already makes, and anything here can be dropped.
//
// Plaintext by decision (2026-08-31): E2EE protects the wire; the browser
// profile is trusted. A browser without IndexedDB, a private window that
// refuses it, or a corrupted database all degrade to "no cache" silently —
// the app works exactly as it did before this module existed.
//
// One record per (device, entity, kind, sub-key). The device leads the key so
// two paired devices never read each other's world; the entity comes second so
// evicting one branch is a single range delete.
//
// There is no in-memory layer above the store. A mirror of the records would
// be a second copy to keep true — of what this tab wrote, what another tab
// wrote, and what eviction took away — for one frame of latency on a revisit.
// The store is read directly, and every writer announces what it changed.

const DB_NAME = "build-cache";
// v2: the cache-first client's shapes. A format change is a cold start by
// design — the records a previous version wrote are not this version's shapes,
// and one sync pass refills what the reader is looking at.
const DB_VERSION = 2;
const STORE = "records";

/** After the first failure the cache stands down for the session: a cache that
 *  errors on every call is worse than none, and nothing above this module is
 *  allowed to notice either way. */
let disabled = false;

let dbPromise = null;

function openDb() {
  if (disabled || typeof indexedDB === "undefined") return Promise.resolve(null);
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve) => {
    let request;
    try {
      request = indexedDB.open(DB_NAME, DB_VERSION);
    } catch (error) {
      standDown(error);
      resolve(null);
      return;
    }
    request.onupgradeneeded = () => {
      const db = request.result;
      if (db.objectStoreNames.contains(STORE)) db.deleteObjectStore(STORE);
      db.createObjectStore(STORE);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => {
      standDown(request.error);
      resolve(null);
    };
    request.onblocked = () => {
      standDown(new Error("cache database blocked by another tab"));
      resolve(null);
    };
  });
  return dbPromise;
}

function standDown(error) {
  if (!disabled) console.warn("local cache disabled for this session:", error);
  disabled = true;
  dbPromise = null;
}

/** One transaction, one operation, resolved when the transaction settles with
 *  what it did: whether it committed, and the result of the request `run`
 *  returned. `run` gets the store and returns an IDBRequest (or null for
 *  delete-ranges, where the transaction's own completion is the answer). */
function transact(mode, run) {
  return openDb().then(
    (db) =>
      new Promise((resolve) => {
        if (!db) {
          resolve({ committed: false });
          return;
        }
        let request;
        try {
          const transaction = db.transaction(STORE, mode);
          request = run(transaction.objectStore(STORE));
          transaction.onabort = () => {
            standDown(transaction.error);
            resolve({ committed: false });
          };
          transaction.oncomplete = () =>
            resolve({ committed: true, result: request ? request.result : undefined });
        } catch (error) {
          standDown(error);
          resolve({ committed: false });
        }
      }),
  );
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
    keys.forEach((key, index) => {
      const request = store.get(key);
      request.onsuccess = () => {
        records[index] = request.result;
      };
    });
    return null;
  }).then(() => records);
}

/** Write one record, stamped with when. Resolves to undefined always. */
export function writeCached(address, value) {
  const record = { at: Date.now(), value };
  const key = recordKey(address);
  return wroteStore((store) => {
    store.put(record, key);
    return null;
  }).then((wrote) => {
    // The key that was stored is the address announced, so a listener is
    // never sent to re-read an address the record is not under.
    if (wrote) announce(partsOfKey(key));
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

/** Every record under an address prefix: its full address, when it was
 *  written, and its value — what the lifetime rules read to decide what has
 *  aged out and what a workspace is holding too much of. */
export async function cachedRecords(prefixAddress) {
  const prefix = `${keyOfParts(addressParts(prefixAddress))}|`;
  const range = prefixRange(prefix);
  let keys = [];
  let records = [];
  await inStore("readonly", (store) => {
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
