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

/** One transaction, one operation, resolved when the transaction settles.
 *  `run` gets the store and returns an IDBRequest (or null for delete-ranges,
 *  where the transaction's own completion is the answer). */
function inStore(mode, run) {
  return openDb().then(
    (db) =>
      new Promise((resolve) => {
        if (!db) {
          resolve(undefined);
          return;
        }
        let request;
        try {
          const transaction = db.transaction(STORE, mode);
          request = run(transaction.objectStore(STORE));
          transaction.onabort = () => {
            standDown(transaction.error);
            resolve(undefined);
          };
          transaction.oncomplete = () => resolve(request ? request.result : undefined);
        } catch (error) {
          standDown(error);
          resolve(undefined);
        }
      }),
  );
}

/** The record key. Every part is URI-encoded so a separator inside a branch
 *  name, path, or hash cannot make one record's key a prefix of another's. */
const recordKey = ({ deviceId, entityId, kind, sub = "" }) =>
  [deviceId, entityId, kind, sub].map(encodeURIComponent).join("|");

const prefixRange = (prefix) => IDBKeyRange.bound(prefix, `${prefix}￿`, false, true);

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
  return inStore("readwrite", (store) => {
    store.put(record, recordKey(address));
    return null;
  });
}

/** Drop every record one entity holds on one device — a single range delete,
 *  which is why the entity sits second in the key. */
export function evictEntity(deviceId, entityId) {
  const prefix = `${encodeURIComponent(deviceId)}|${encodeURIComponent(entityId)}|`;
  return inStore("readwrite", (store) => {
    store.delete(prefixRange(prefix));
    return null;
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

/** Drop the whole database. For sign-out, and for a format change. */
export function wipeCache() {
  return inStore("readwrite", (store) => {
    store.clear();
    return null;
  });
}
