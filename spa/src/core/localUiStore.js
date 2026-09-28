// The local UI store: unsent drafts, folds, menus and the other `ui-*` state a
// view keeps on this browser (core/localUiState.js is its only writer).
//
// It is its own IndexedDB database, versioned apart from the replica cache
// (core/localCache.js). The replica is disposable — a schema bump rebuilds it,
// the lifetime sweeps and evictions trim it, and an older tab that meets a
// newer replica version deletes the whole replica database — and none of that
// can reach this one. Nothing here is a copy of anything the bridge holds, so
// nothing here may be dropped to be refilled.
//
// Builds before this store kept `ui-*` records in the replica store. They are
// carried here, each only while it is newer than what this store holds, and
// then removed from the replica store: once at the first use in a page, and
// again whenever another tab — one still running such a build — announces a
// `ui-*` write there. The replica upgrade keeps them until they are carried.

import { createIdbDatabase, putOrAbort } from "./idbDatabase.js";
import { createAnnouncer, partsOfKey, recordKey, writeStamp } from "./idbRecords.js";
import { cachedUiRecords, deleteCachedIfUnwritten, subscribeCache } from "./localCache.js";

const DB_NAME = "build-ui";
// v1: one store of records keyed as the replica keys them. A later format
// migrates what is here; it never starts this store cold.
const DB_VERSION = 1;
const STORE = "records";

export const UI_STORE_DIAGNOSTIC = "local-ui-store";

function upgrade(request) {
  const db = request.result;
  if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE);
}

const database = createIdbDatabase({
  name: DB_NAME, version: DB_VERSION, store: STORE, upgrade, diagnostic: UI_STORE_DIAGNOSTIC, label: "local UI store",
});
const { announce, subscribe } = createAnnouncer("build-ui");

/** For tests: a faster recovery schedule. Answers the one it replaced. */
export const setUiStoreRecoveryTiming = (next) => database.setRecoveryTiming(next);

// ─── Carrying the replica store's `ui-*` records across ──────────────────────

let adopting = Promise.resolve(0);
let firstAdoption = null;
/** Raised by every wipe. A carry pass that read the replica store before a
 *  wipe holds the last account's records, and writes none of them after. */
let wipes = 0;

/** How long a read waits for the page's first carry pass. A mount that read
 *  before a carried draft arrived would paint an empty composer, and the
 *  first keystroke typed into it is newer than the draft it replaces. The
 *  wait is bounded because the replica may be away (a resume, a stand-down):
 *  past it the read goes ahead, and a draft carried later is announced. */
const FIRST_ADOPTION_WAIT_MS = 1500;

/** Whether `candidate` is newer than `current`: the same writer by its edit
 *  sequence, any other by the time it was written, ties left alone. */
const isNewer = (candidate, current) => !current || (current.source && current.source === candidate.source
  ? (Number(current.sequence) || 0) < (Number(candidate.sequence) || 0)
  : (Number(current.at) || 0) < (Number(candidate.at) || 0));

/** Put each carried record that is newer than what is here, in one
 *  transaction, unless a wipe came after `wipedAt`. Answers the keys it put,
 *  or null when nothing committed. */
function putNewer(entries, wipedAt) {
  const put = [];
  return database.write((store) => {
    put.length = 0;
    // A wipe's transaction is created when it is asked for, so one asked for
    // before this callback runs clears after nothing this pass puts.
    if (wipes !== wipedAt) return null;
    for (const { address, record } of entries) {
      const key = recordKey(address);
      const current = store.get(key);
      current.onsuccess = () => {
        if (!isNewer(record, current.result)) return;
        if (putOrAbort(store, record, key)) put.push(key);
      };
    }
    return null;
  }, true).then((committed) => (committed ? put : null));
}

async function adoptOnce() {
  const wipedAt = wipes;
  const entries = (await cachedUiRecords()).filter((entry) => entry.record);
  if (!entries.length || wipes !== wipedAt) return 0;
  const put = await putNewer(entries, wipedAt);
  // Nothing leaves the replica store until this store has committed it.
  if (!put) return 0;
  for (const key of put) announce(partsOfKey(key));
  await deleteCachedIfUnwritten(entries);
  return put.length;
}

/** Carry every `ui-*` record the replica store holds into this store, where
 *  it is newer, and out of the replica store. One pass at a time. Answers how
 *  many it put here. */
export function adoptCachedUiRecords() {
  const next = adopting.then(adoptOnce, adoptOnce);
  adopting = next.catch(() => 0);
  return next;
}

/** Start carrying: now, and on every `ui-*` write another tab announces on
 *  the replica's channel. Answers the page's first pass, which reads wait on
 *  (bounded); a later pass is announced, and the view watching that address
 *  repaints from here. */
function watchReplica() {
  if (firstAdoption) return firstAdoption;
  subscribeCache({}, (address) => {
    if (address.kind?.startsWith("ui-")) void adoptCachedUiRecords();
  });
  const adopted = adoptCachedUiRecords().catch(() => 0);
  firstAdoption = Promise.race([adopted, new Promise((resolve) => {
    const timer = setTimeout(resolve, FIRST_ADOPTION_WAIT_MS);
    timer?.unref?.();
  })]);
  return firstAdoption;
}

// ─── Records ─────────────────────────────────────────────────────────────────

/** Read one record: `{ at, value, … }`, or undefined. */
export async function readUiRecord(address) {
  await watchReplica();
  return database.read("readonly", (store) => store.get(recordKey(address)));
}

/** Write one record, stamped with when — and, for a UI writer, its owner and
 *  edit sequence, which order the page-exit journal's replay. */
export function writeUiRecord(address, value, { source, sequence } = {}) {
  watchReplica();
  const key = recordKey(address);
  const record = { ...writeStamp(), value, ...(source ? { source, sequence } : {}) };
  return database.write((store) => {
    store.put(record, key);
    return null;
  }).then((wrote) => {
    if (wrote) announce(partsOfKey(key));
  });
}

/** Replay one page-exit edit only if it is still the newest edit. The get
 * and conditional put share a readwrite transaction, so another tab cannot
 * insert a newer record between the comparison and the write. A same-owner
 * in-flight write made just before page exit is ordered by edit sequence;
 * other writers are ordered by timestamp, with ties left untouched. */
export function writeUiRecordIfNewer(address, value, { at, source, sequence }) {
  if (!Number.isFinite(at) || !source || !Number.isFinite(sequence)) return Promise.resolve(false);
  watchReplica();
  const key = recordKey(address);
  let applied = false;
  return database.write((store) => {
    applied = false;
    const current = store.get(key);
    current.onsuccess = () => {
      if (!isNewer({ at, source, sequence }, current.result)) return;
      applied = putOrAbort(store, { ...writeStamp(), value, source, sequence }, key);
    };
    return null;
  }).then((committed) => {
    if (committed && applied) announce(partsOfKey(key));
    return Boolean(committed && applied);
  });
}

/** Hear every write at or under `prefixAddress`, from this tab and the
 *  others. Answers the way to stop listening. */
export function subscribeUiRecords(prefixAddress, listener) {
  watchReplica();
  return subscribe(prefixAddress, listener);
}

/** Drop every record — for the account reset that precedes the next account,
 *  never for a replica's lifetime. A carry pass already under way writes
 *  nothing after it. */
export function wipeUiRecords() {
  wipes += 1;
  return database.write((store) => {
    store.clear();
    return null;
  }).then((wrote) => {
    if (wrote) announce([]);
  });
}
