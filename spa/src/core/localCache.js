// The local cache: IndexedDB replicas plus isolated durable write-operation stores.
// Surfaces paint from it while the sync layer asks the bridge for newer data.
// A suspended browser can close its connection without losing those records;
// the next operation reopens it instead of making the records disappear.
//
// Plaintext by decision (2026-08-31): E2EE protects the wire; the browser
// profile is trusted. An unavailable database answers no records and
// announces no writes. A lost connection is reopened, not given up on.
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
import { abortForError, createIdbDatabase, putOrAbort } from "./idbDatabase.js";
import {
  addressOfParts, addressParts, createAnnouncer, keyOfParts, partsOfKey, prefixRange, randomToken, recordKey, writeStamp,
} from "./idbRecords.js";

const DB_NAME = "build-cache";
// v3: the cache-first client's shapes, and the write-time index the lifetime
// rules sweep. A format change is a cold start by design — the records a
// previous version wrote are not this version's shapes, and one sync pass
// refills what the reader is looking at. v4: the task rename (#190) — a
// record keyed or shaped by the old names is dropped, not misread.
//
// Unsent drafts and UI state are not replicas and do not live here: they are
// in their own database (core/localUiStore.js), which this version never
// reaches. A browser that ran a build before that split still holds `ui-*`
// records in this store; an upgrade keeps them (see `upgrade`) until the UI
// store has taken them across.
// v5 adds durable pending-operation and tab-lease stores without resetting v4 replicas.
const DB_VERSION = 5;
const STORE = "records";
import {
  WRITE_OPERATION_STORE, WRITE_LEASE_STORE, activeOperationNamespaces, composeOperationRecords,
} from "./writeOperationProjection.js";

/** The index on each record's write time. It exists so "how old is what this
 *  workspace holds" can be answered from index keys alone: a key cursor
 *  yields (`at`, record key) pairs and never the record, and a workspace's
 *  records are where the megabytes are — file bodies, patches, a
 *  working-tree diff. Reading those to look at a timestamp would clone tens
 *  of megabytes onto the main thread and discard every byte. */
const AT_INDEX = "at";

/** Whether a record key is local UI state an older build wrote here: its
 *  kind, the third part, starts `ui-`. */
export const isUiRecordKey = (key) => (partsOfKey(key)[2] || "").startsWith("ui-");

const createReplicaStore = (db) => {
  const store = db.createObjectStore(STORE);
  store.createIndex(AT_INDEX, "at");
  return store;
};

/** Add durable stores independently from replica format resets. Rebuild an
 * obsolete replica format — keeping every `ui-*` record
 *  it held. Those are a user's unsent drafts, written by a build that kept
 *  them here, and possibly by an older tab still running one: they are read
 *  inside the upgrade's own transaction and put back into the rebuilt store,
 *  so the upgrade commits with them or not at all. */
function upgrade(request, event) {
  const db = request.result;
  for (const name of [WRITE_OPERATION_STORE, WRITE_LEASE_STORE]) {
    if (!db.objectStoreNames.contains(name)) {
      const store = db.createObjectStore(name);
      if (name === WRITE_OPERATION_STORE) {
        store.createIndex("namespace", "namespace");
        store.createIndex("overlayNamespace", "overlayNamespace");
      }
    }
  }
  if (event.oldVersion >= 4) return;
  if (!db.objectStoreNames.contains(STORE)) {
    createReplicaStore(db);
    return;
  }
  const previous = request.transaction.objectStore(STORE);
  const keys = previous.getAllKeys();
  keys.onsuccess = () => {
    const kept = keys.result.filter(isUiRecordKey).map((key) => ({ key, request: previous.get(key) }));
    const rebuild = () => {
      db.deleteObjectStore(STORE);
      const fresh = createReplicaStore(db);
      for (const { key, request: read } of kept) fresh.put(read.result, key);
    };
    if (!kept.length) return rebuild();
    kept[kept.length - 1].request.onsuccess = rebuild;
  };
}

export const CACHE_DIAGNOSTIC = "local-cache";

const database = createIdbDatabase({
  name: DB_NAME, version: DB_VERSION, store: STORE,
  transactionStores: [STORE, WRITE_OPERATION_STORE, WRITE_LEASE_STORE], upgrade, diagnostic: CACHE_DIAGNOSTIC, label: "local cache",
  // Pending writes are durable user intent: never delete an unfamiliar version.
  dropNewer: false,
});

/** For tests: a faster recovery schedule. Answers the one it replaced. */
export const setCacheRecoveryTiming = (next) => database.setRecoveryTiming(next);

/** What the cache is doing right now (see `health` in core/idbDatabase.js). */
export const cacheHealth = () => database.health();

const transact = database.transact;
const inStore = database.read;
const wroteStore = database.write;

/** The account's device list — the one record that is nobody's device. It
 *  comes from skriftapp rather than from a bridge, so it is addressed under no
 *  device and no entity. Named here because the presence read writes it and
 *  the boot paint reads it, and the two must agree on where it lives. */
export const DEVICES_ADDRESS = Object.freeze({ deviceId: "", entityId: "", kind: "devices" });

const { announce, subscribe } = createAnnouncer("build-cache");

/** Hear every write and eviction at or under `prefixAddress`, from this tab
 *  and from every other tab on this browser profile. The listener is handed
 *  the address that changed and reads what it wants; answers the way to stop
 *  listening. */
export const subscribeCache = subscribe;

/** Share monotonic admission evidence with open tabs even if storage refused
 * its record. This transient metadata never replaces a cached render value. */
export const announceCacheEvidence = (address, evidence) => announce(addressParts(address), true, evidence);

/** Read one record: `{ at, value }`, or undefined when it was never written,
 *  the cache is unavailable, or anything went wrong. */
export function readCanonicalCached(address) {
  return inStore("readonly", (store) => store.get(recordKey(address)));
}

export function readCached(address) {
  return readCachedMany([address]).then((records) => records[0]);
}

/** Shared transaction boundary for durable operations, layers and replicas. */
export const transactWriteOperations = (mode, run) => transact(mode, (records) => run({
  records,
  operations: records.transaction.objectStore(WRITE_OPERATION_STORE),
  leases: records.transaction.objectStore(WRITE_LEASE_STORE),
}));

export const announceWriteOperationCache = (addresses) => {
  for (const address of addresses) announce(addressParts(address));
};

/** Admission checks must distinguish a confirmed absent record from a failed
 * read. Keep the transaction's committed verdict alongside its result. */
export function readCachedOutcome(address) {
  return transact("readonly", (store) => store.get(recordKey(address)));
}

/** Read many records in one transaction — a view's first paint asks for
 *  everything it draws at once rather than a round trip per surface. Answers
 *  one slot per address, in the order asked, undefined where there is no
 *  record. */
export function readCanonicalCachedMany(addresses) {
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

/** Read base and accepted layers in one snapshot, including after reload. */
export async function readCachedMany(addresses) {
  if (!addresses.length) return [];
  let canonical = [], operations = [];
  await transactWriteOperations("readonly", (stores) => {
    readRecordsInStore(stores.records, addresses.map(recordKey), (records) => { canonical = records; });
    operations = [];
    for (const namespace of activeOperationNamespaces(addresses)) {
      const read = stores.operations.index("overlayNamespace").getAll(namespace);
      read.onsuccess = () => { operations.push(...read.result); };
    }
    return null;
  });
  return composeOperationRecords(addresses, canonical, operations);
}

/** Every feed write goes through this transaction. A local rewrite carries
 * surviving rows' observation times; a bridge board read explicitly replaces
 * them, less any row it names as superseded — one a push wrote after the read
 * was asked, whose own record is the newer word. An undo can merge into the
 * current value inside the same transaction. `readAddresses` adds canonical
 * records to that snapshot, handed to update as its third argument. */
function writeFeed(address, update, { observedFeedRows = false, supersededFeedRow, unchanged = [], readAddresses = [], readStateFrom = null } = {}) {
  const key = recordKey(address);
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    // Read and put in the same transaction: another tab can write between a
    // separate read and write, and its newer row observation must survive.
    const keys = [key, ...unchanged.map(({ address }) => recordKey(address)), ...readAddresses.map(recordKey)];
    readRecordsInStore(store, keys, ([previous, ...records]) => {
      try {
        if (readStateFrom && !isReadStateRecord(previous, readStateFrom)) return;
        if (!unchanged.every(({ written }, index) => isCachedWrite(records[index], written))) return;
        const next = update(previous?.value, previous, records.slice(unchanged.length));
        if (next == null) return;
        const { write, ...stamp } = writeStamp();
        const record = { ...stamp, write, ...readStateStamp(readStateFrom),
          value: feedWithObservations(next, previous, stamp, observedFeedRows, supersededFeedRow) };
        changed = putOrAbort(store, record, key);
      } catch (error) {
        abortForError(store, error);
      }
    });
    return null;
  }).then((committed) => {
    if (committed && changed) announce(partsOfKey(key));
    return Boolean(committed && changed);
  });
}

/** Read the records needed by one conditional write without leaving its
 * transaction. A writer cannot change a guard between these reads and put. */
function readRecordsInStore(store, keys, read) {
  const records = new Array(keys.length);
  let remaining = keys.length;
  keys.forEach((key, index) => {
    const request = store.get(key);
    request.onsuccess = () => {
      records[index] = request.result;
      if (--remaining === 0) read(records);
    };
  });
}

/** Write one record, stamped with when. A local UI writer may also stamp its
 * owner and edit sequence for page-exit journal ordering. */
export function writeCached(address, value, { source, sequence, observedFeedRows = false, publication } = {}) {
  if (address.kind === "feed") return writeFeed(address, () => value, { observedFeedRows });
  if (address.kind === "bridge-update") return writeBridgeUpdate(address, value);
  const key = recordKey(address);
  const record = { ...writeStamp(), value, ...(source ? { source, sequence } : {}), ...(publication ? { publication } : {}) };
  return wroteStore((store) => {
    store.put(record, key);
    return null;
  }).then((wrote) => {
    if (wrote) announce(partsOfKey(key));
  });
}

/** An absent record differs from an older record without a generation. */
export const cachedGeneration = (record) => record ? (record.generation || 0) : -1;
export const cacheAvailable = database.available;
const newCacheGeneration = randomToken;
const withBridgeGeneration = (address, record) => address.kind === "bridge-update"
  ? { ...record, generation: newCacheGeneration() } : record;

/** Capture a record before a conditional mutation. An absent record gets a
 * stamped null placeholder, so a write followed by deletion cannot look
 * unchanged. Readers continue to treat its null value as absent. */
export function captureCachedRecord(address) {
  const key = recordKey(address);
  let record;
  return wroteStore((store) => {
    record = undefined;
    const request = store.get(key);
    request.onsuccess = () => {
      try {
        if (request.result) {
          record = request.result;
          return;
        }
        record = { ...writeStamp(), value: null, generation: newCacheGeneration() };
        putOrAbort(store, record, key);
      } catch (error) {
        abortForError(store, error);
      }
    };
    return null;
  }).then((committed) => committed ? record : undefined);
}

/** Snapshot a bridge record before its RPC starts, fencing deletion too. */
export const captureCachedGeneration = (address) => captureCachedRecord(address).then(cachedGeneration);

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
          ...writeStamp(), value,
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
 * Null leaves the feed untouched. `unchanged` fences any other captured
 * records the update depends on, checked in this same transaction. */
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
        const record = withBridgeGeneration(address, { ...writeStamp(), value, source, sequence });
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

/** Joint merges waiting through a lost connection keep their order per key
 * in this tab. Each retry re-reads current records, including other tabs'
 * writes and plain writes, so merge callbacks must reconcile those values. */
const recoveringWrites = new Map();
function inRecoveryWriteOrder(keys, run) {
  const prior = Promise.all(keys.map((key) => recoveringWrites.get(key)));
  const running = prior.then(run);
  const settled = running.catch(() => {});
  for (const key of keys) recoveringWrites.set(key, settled);
  void settled.then(() => {
    for (const key of keys) if (recoveringWrites.get(key) === settled) recoveringWrites.delete(key);
  });
  return running;
}

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
    const next = merge((await readCanonicalCached(address))?.value);
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
export function mergeCachedAtomically(address, merge, { reportCommit = false } = {}) {
  return mergeRecordAtomically(address, (record) => merge(record?.value), reportCommit);
}

/** Which write a record is, as `readCached` answered it: to find the record
 * still that write later, inside a transaction (`isCachedWrite`). A record
 * stored before writes were named has none — and a page still running that
 * code can write another like it on the same tick — so it is never found
 * unchanged, and neither is an absent one. */
export const cachedWriteOf = (record) => record?.write;
export const isCachedWrite = (record, written) => typeof written === "string" && record?.write === written;

/** Which write a record is, for a guard held across a wire call outside any
 * transaction: its name, or for a record stored before writes were named its
 * stamp and order, and null for no record. Equal only for the same write, so
 * a record replaced on the same millisecond, or dropped, is not the one a
 * guard captured (#95). `at` is when; this is which. */
export const recordWriteOf = (record) => (record ? record.write ?? `${record.at}:${record.order}` : null);

/** The same merge, only while the record is still the write `written` names
 * (`cachedWriteOf`). Any writer since, in any tab, leaves the record alone:
 * for a verb's answer that must not land over what arrived after the verb was
 * sent. */
export function mergeCachedIfUnwritten(address, written, merge) {
  return mergeRecordAtomically(address, (record) => (isCachedWrite(record, written) ? merge(record?.value) : null));
}

// Only read-field repairs may share this lineage. Every ordinary writer makes
// a fresh record without it, so even an identical Detail/push write fences it.
const readStateBase = (record) => record?.readStateBase || cachedWriteOf(record);
const readStateStamp = (record) => record ? { readStateBase: readStateBase(record) } : {};
const isReadStateRecord = (current, before) => isCachedWrite(current, cachedWriteOf(before))
  || typeof current?.readStateBase === "string" && current.readStateBase === readStateBase(before);

/** Atomic read-field-only repair across competing repairs of the same original
 * write. `original` is false after a competing repair; callers then require
 * comparable, monotonic cursors. Any ordinary write ends the lineage. */
export function mergeCachedReadState(address, before, merge) {
  return mergeRecordAtomically(address, (record) => merge(record?.value, isCachedWrite(record, cachedWriteOf(before))), false, before);
}

/** Put records and let go of others in one readwrite transaction, only while
 * `guard.address` is still the write `guard.written` names (`recordWriteOf`,
 * so null is no record) — or always, with no guard. `drop` names the records
 * to let go of: those under `{ deviceId, entityId, kind }` whose sub starts
 * `subPrefix` and that `where(sub, value)` picks; they go before the puts, so
 * a put may name one of them. For a record and the records beside it that
 * must change together or not at all (a file's head and its pages, #270).
 * Answers whether it was written. */
export function writeCachedIfStill({ guard = null, puts = [], drop = null }) {
  const changed = [];
  return wroteStore((store) => {
    changed.length = 0;
    const put = () => {
      for (const { address, value } of puts) {
        const key = recordKey(address);
        if (!putOrAbort(store, withBridgeGeneration(address, { ...writeStamp(), value }), key)) return;
        changed.push(key);
      }
    };
    const dropThenPut = () => {
      if (!drop) return put();
      const kindPrefix = recordKey({ ...drop, sub: "" });
      const walk = store.openCursor(prefixRange(`${kindPrefix}${encodeURIComponent(drop.subPrefix)}`));
      walk.onsuccess = () => {
        const cursor = walk.result;
        if (!cursor) return put();
        const key = String(cursor.primaryKey);
        try {
          if (drop.where(decodeURIComponent(key.slice(kindPrefix.length)), cursor.value?.value)) {
            cursor.delete();
            changed.push(key);
          }
          cursor.continue();
        } catch (error) {
          abortForError(store, error);
        }
      };
    };
    if (!guard) {
      dropThenPut();
      return null;
    }
    const request = store.get(recordKey(guard.address));
    request.onsuccess = () => {
      if (recordWriteOf(request.result) === guard.written) dropThenPut();
    };
    return null;
  }).then((committed) => {
    if (!committed || !changed.length) return false;
    for (const key of new Set(changed)) announce(partsOfKey(key));
    return true;
  });
}

function mergeRecordAtomically(address, merge, reportCommit = false, readStateFrom = null) {
  const key = recordKey(address);
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    const request = store.get(key);
    request.onsuccess = () => {
      try {
        if (readStateFrom && !isReadStateRecord(request.result, readStateFrom)) return;
        const next = merge(request.result);
        if (next == null) return;
        const record = withBridgeGeneration(address, { ...writeStamp(), ...readStateStamp(readStateFrom), value: next });
        changed = putOrAbort(store, record, key);
      } catch (error) {
        abortForError(store, error);
      }
    };
    return null;
  }).then((committed) => {
    if (committed && changed) announce(partsOfKey(key));
    // Safety history needs to distinguish a committed no-op from a refusal.
    return Boolean(committed && (reportCommit || changed));
  });
}

/** Merge several records inside one IndexedDB readwrite transaction: `merge`
 * is handed their values, in the order asked, and answers the next value for
 * each — null leaves that one alone. For records that must move together, the
 * way a list and the note of what it was last read as must, so neither another
 * tab nor a failed write can land one without the other. */
export function mergeCachedTogether(addresses, merge) {
  const keys = addresses.map(recordKey);
  return inRecoveryWriteOrder(keys, () => mergeTogetherInStore(addresses, keys, merge, (record) => record?.value));
}

/** The same, handed each whole record — `{ at, value }`, or undefined — for a
 *  merge that must keep what the cache's stamp on a record says. */
export function mergeCachedRecordsTogether(addresses, merge) {
  const keys = addresses.map(recordKey);
  return inRecoveryWriteOrder(keys, () => mergeTogetherInStore(addresses, keys, merge, (record) => record));
}

function mergeTogetherInStore(addresses, keys, merge, handed) {
  let changed = [];
  return wroteStore((store) => {
    changed = [];
    const held = new Array(keys.length);
    let waiting = keys.length;
    const mergeAll = () => {
      try {
        const next = merge(held.map(handed));
        for (const [index, key] of keys.entries()) {
          if (next?.[index] == null) continue;
          const record = withBridgeGeneration(addresses[index], { ...writeStamp(), value: next[index] });
          if (!putOrAbort(store, record, key)) return;
          changed.push(key);
        }
      } catch (error) {
        abortForError(store, error);
      }
    };
    keys.forEach((key, index) => {
      const request = store.get(key);
      request.onsuccess = () => {
        held[index] = request.result;
        waiting -= 1;
        if (!waiting) mergeAll();
      };
    });
    return null;
  }, true).then((committed) => {
    if (committed) for (const key of changed) announce(partsOfKey(key));
    return Boolean(committed && changed.length);
  });
}

/** Raise a count every tab shares, in one transaction, and answer it: one more
 * than it held, and never under `floor`. Wait through a transient connection
 * loss; undefined means no cache exists to share, while a refused counter
 * write rejects instead of handing out an unshared number. Not announced. */
export function takeCachedCount(address, floor = 0) {
  const key = recordKey(address);
  return inRecoveryWriteOrder([key], () => takeCountInStore(key, floor));
}

async function takeCountInStore(key, floor) {
  let taken;
  const outcome = await transact("readwrite", (store) => {
    taken = undefined;
    const request = store.get(key);
    request.onsuccess = () => {
      const held = Number(request.result?.value) || 0;
      const next = Math.max(held + 1, floor);
      if (putOrAbort(store, { ...writeStamp(), value: next }, key)) taken = next;
    };
    return null;
  }, true);
  if (outcome.committed) return taken;
  if (outcome.unavailable) return undefined;
  throw outcome.error || new Error("Build could not update the shared cache counter.");
}

/** Drop every record one entity holds on one device — range deletes, which
 *  is why the entity sits second in the key. The `ui-*` records a build
 *  before the UI store left here are skipped until they are carried: every
 *  kind starting `ui-` sorts between the two ranges. */
export function evictEntity(deviceId, entityId, { still = null } = {}) {
  const prefix = `${encodeURIComponent(deviceId)}|${encodeURIComponent(entityId)}|`;
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    deleteWhileCurrent(store, prefix, still, () => {
      store.delete(IDBKeyRange.bound(prefix, `${prefix}ui-`, false, true));
      store.delete(IDBKeyRange.bound(`${prefix}ui.`, `${prefix}\uffff`, false, true));
      changed = true;
    });
    return null;
  }).then((wrote) => {
    if (wrote && changed) announce([encodeURIComponent(deviceId), encodeURIComponent(entityId)]);
  });
}

/** Transaction creation can precede an earlier writer's commit. Wait for a
 * request to execute, then check and delete in the same exclusive transaction.
 * No writer can interleave between that predicate and the deletes. Recovery
 * retries issue a fresh request and evaluate the predicate again. */
function deleteWhileCurrent(store, key, still, remove) {
  if (!still) return remove();
  const ready = store.getKey(key);
  ready.onsuccess = () => {
    if (still()) remove();
  };
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

/** Expire several prefixes together without cloning bodies or one callback
 * per stale account record. The existing time index and bounded primary-key
 * reads intersect inside one exclusive transaction, so a refreshed record
 * cannot be deleted using an age observed before its write. */
export async function expireCachedPrefixes(prefixAddresses, writtenBefore, { still = () => true, keep = () => false } = {}) {
  const prefixes = [...new Set(prefixAddresses.map((address) => `${keyOfParts(addressParts(address))}|`))];
  let result = { dropped: [], kept: [] };
  if (!prefixes.length) return result;
  const committed = await wroteStore((store) => {
    result = { dropped: [], kept: [] };
    readExpiryKeys(store, prefixes, writtenBefore, (stale, selected) => {
      try {
        if (still()) result = expireSelectedKeys(store, stale, selected, keep);
      } catch (error) {
        abortForError(store, error);
      }
    });
    return null;
  });
  if (!committed) return { dropped: [], kept: [] };
  for (const address of result.dropped) announce(addressParts(address));
  return result;
}

function readExpiryKeys(store, prefixes, writtenBefore, ready) {
  const stale = store.index(AT_INDEX).getAllKeys(IDBKeyRange.upperBound(writtenBefore, true));
  const selected = prefixes.map((prefix) => store.getAllKeys(prefixRange(prefix)));
  let waiting = selected.length + 1;
  const done = () => {
    if (--waiting === 0) ready(stale.result, selected.flatMap((request) => request.result));
  };
  for (const request of [stale, ...selected]) request.onsuccess = done;
}

function expireSelectedKeys(store, stale, selected, keep) {
  const remaining = new Set(selected);
  const dropped = [];
  for (const key of stale) {
    if (!remaining.has(key) || isUiRecordKey(key)) continue;
    const address = addressOfParts(partsOfKey(key));
    if (keep(address)) continue;
    store.delete(key);
    remaining.delete(key);
    dropped.push(address);
  }
  return { dropped, kept: [...remaining].map((key) => addressOfParts(partsOfKey(key))) };
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
 *  surface holding a record that has aged out hears it go. A `ui-*` record is
 *  never a replica's to drop: only its carry removes it. */
export function deleteCached(addresses, { still = null } = {}) {
  const keys = addresses.map(recordKey).filter((key) => !isUiRecordKey(key));
  if (!keys.length) return Promise.resolve();
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    deleteWhileCurrent(store, keys[0], still, () => {
      for (const key of keys) store.delete(key);
      changed = true;
    });
    return null;
  }).then((wrote) => {
    if (!wrote || !changed) return;
    for (const key of keys) announce(partsOfKey(key));
  });
}

/** Clear replica records, preserving durable operation intent. Announced
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

/** Every `ui-*` record a build before the UI store's split left here, with
 *  its address: `{ address, record }`. The key walk reads no replica body. */
export async function cachedUiRecords() {
  let held = [];
  await inStore("readonly", (store) => {
    held = [];
    const keys = store.getAllKeys();
    keys.onsuccess = () => {
      for (const key of keys.result.filter(isUiRecordKey)) {
        const entry = { address: addressOfParts(partsOfKey(key)), record: undefined };
        held.push(entry);
        const read = store.get(key);
        read.onsuccess = () => {
          entry.record = read.result;
        };
      }
    };
    return null;
  });
  return held;
}

/** Delete each `{ address, record }` (as `cachedUiRecords` answered it) that
 *  is still that write, in one transaction. One any tab has written since is
 *  left for whoever asks next. */
export function deleteCachedIfUnwritten(entries) {
  const deleted = [];
  return wroteStore((store) => {
    deleted.length = 0;
    for (const { address, record } of entries) {
      const key = recordKey(address);
      const current = store.get(key);
      current.onsuccess = () => {
        if (recordWriteOf(current.result) !== recordWriteOf(record)) return;
        store.delete(key);
        deleted.push(key);
      };
    }
    return null;
  }).then((wrote) => {
    if (wrote) for (const key of deleted) announce(partsOfKey(key));
  });
}
