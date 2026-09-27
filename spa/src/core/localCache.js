// The local cache: one IndexedDB store holding what each surface last saw.
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
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";

const DB_NAME = "build-cache";
// v3: the cache-first client's shapes, and the write-time index the lifetime
// rules sweep. A format change is a cold start by design — the records a
// previous version wrote are not this version's shapes, and one sync pass
// refills what the reader is looking at. v4: the task rename (#190) — a
// record keyed or shaped by the old names is dropped, not misread.
const DB_VERSION = 4;
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
const randomToken = () => globalThis.crypto?.randomUUID?.() ||
  `${Date.now()}-${Math.random()}-${Math.random()}`;
// `order` is a clock, and two tabs can read the same tick. `write` names the
// write itself — this page's own id and its count of writes — so a reader
// that captured it can tell whether any writer, in any tab, has put the
// record since. Every record this module writes carries both.
const WRITER = randomToken();
let writesMade = 0;
const writeStamp = () => ({ at: Date.now(), order: nextWriteOrder(), write: `${WRITER}:${(writesMade += 1)}` });

/** The index on each record's write time. It exists so "how old is what this
 *  workspace holds" can be answered from index keys alone: a key cursor
 *  yields (`at`, record key) pairs and never the record, and a workspace's
 *  records are where the megabytes are — file bodies, patches, a
 *  working-tree diff. Reading those to look at a timestamp would clone tens
 *  of megabytes onto the main thread and discard every byte. */
const AT_INDEX = "at";

// ─── Staying open ────────────────────────────────────────────────────────────
//
// iOS suspends a backgrounded page, and WebKit drops its IndexedDB connection
// while it sleeps. On resume the open transactions abort, and for a moment the
// next open or transaction fails with `UnknownError: Connection to Indexed
// Database server lost`. That is weather, not a verdict: the records are still
// on disk. A cache that stood down for the session on it answered "nothing" to
// every surface until a reload, and every surface painted blank (#169).
//
// So a lost connection is reopened on a backoff that waits for the page to be
// shown rather than spending its attempts on a sleeping one. When the attempts
// run out the cache rests until the next wake — the page shown, the network
// back, or the rest simply over — and tries again. Nothing that asks while it
// is away is answered "nothing": a read waits for the cache to come back, so a
// surface that painted keeps what it painted until a read really answers, and
// a merge never mistakes "unreadable" for "empty". A write waits too, and is
// made once the database is back.
//
// Only an error no reopen can fix stands the cache down for the session: a
// private window refusing IndexedDB, a schema that is not ours.
// So does one that outlasts the weather: a store the browser can never open
// (Safari raises the same UnknownError for a corrupt one) fails every round,
// and a page someone is looking at does not wait on it for ever — nor does a
// page that has never opened it at all, whose boot paint is waiting.
// A write the database refuses on its own account (a value it cannot clone, one
// that does not fit in a full quota, or one that keeps failing while every
// other transaction commits) fails alone: the records already stored still
// read, and the pull that carried it asks again.

/** Set when the cache met an error no reopen can fix; it answers nothing for
 *  the rest of the session. */
let disabled = false;

let dbPromise = null;
/** Whether the database has opened at all in this page. Before it has, a
 *  round of failed opens is not a resume's weather but a store that cannot be
 *  opened — and the boot paint is waiting on it. */
let everOpened = false;
const intentionalAborts = new WeakMap();

const DEFAULT_TIMING = Object.freeze({
  /** The wait before each attempt to reach the database: the first at once,
   *  the last several seconds on, about eight in all — long enough to outlast
   *  WebKit reconnecting to its storage process after a resume. */
  reopenDelaysMs: Object.freeze([0, 50, 150, 400, 1000, 2000, 4000]),
  /** How long an open may stay pending before it counts as a failed attempt.
   *  This bounds what one attempt costs, and little more: a hung open holds
   *  the origin's connection queue, so the opens retried after it queue behind
   *  it, and a hang the browser never answers spends the round (about
   *  forty seconds) and then rests. An upgrade blocked by another tab is
   *  waited on for `blockedTimeoutMs` instead. */
  openTimeoutMs: 5000,
  /** How long an upgrade waits for another tab to close its older connection.
   *  A tab on this build closes at once when told to; one on a build before
   *  #169 has nothing that listens, and holds on until it is closed or
   *  reloaded — so past this the cache stands down, and the page paints from
   *  the network as it would without one. */
  blockedTimeoutMs: 10_000,
  /** How long the cache rests after the attempts run out, unless a wake ends
   *  it first. Short enough that storage coming back on a page nobody hides
   *  is noticed within seconds. */
  restMs: 10_000,
  /** How long the cache may keep failing on a page someone is looking at
   *  before the failure counts as the database's own. Counted from the later
   *  of the outage starting and the page last being shown: a hidden page waits
   *  on, and a resume starts the count again. */
  giveUpAfterMs: 60_000,
});
let timing = DEFAULT_TIMING;

/** For tests: a faster schedule. Answers the one it replaced. */
export function setCacheRecoveryTiming(next) {
  const was = timing;
  timing = next ? { ...DEFAULT_TIMING, ...next } : DEFAULT_TIMING;
  return was;
}

/** Errors a reopen can outlive: the connection closed under a transaction, the
 *  browser aborted it, or the storage process went away and is coming back. */
const TRANSIENT_ERRORS = new Set(["AbortError", "InvalidStateError", "TransactionInactiveError", "UnknownError", "TimeoutError"]);
/** Errors that belong to one write rather than to the database. A full quota
 *  refuses what does not fit; everything already stored still reads. */
const WRITE_ERRORS = new Set(["DataCloneError", "DataError", "QuotaExceededError"]);

/** Whose fault a failure was: the connection's (reopen and retry), the
 *  write's (fail it alone), or the database's (stand down). An open that
 *  fails with InvalidStateError is a private window refusing storage, not a
 *  closing connection. */
function faultOf(error, during = "transaction") {
  if (!error) return "connection"; // aborted with no reason: the browser took the connection
  if (during === "open" && error.name === "InvalidStateError") return "database";
  if (TRANSIENT_ERRORS.has(error.name)) return "connection";
  if (during !== "open" && WRITE_ERRORS.has(error.name)) return "write";
  return "database";
}

// ─── What happened to it ─────────────────────────────────────────────────────
//
// Every loss, rest, recovery and stand-down is recorded in the connection
// diagnostics ring (Settings → Diagnostics, and `buildConnectionDiagnostics()`)
// and on the console, with the page's visibility and how long ago it was last
// shown — the facts that say whether a blank screen followed a resume.
// `cacheHealth` answers the state right now, for the line under the dump.

export const CACHE_DIAGNOSTIC = "local-cache";
const MESSAGE_LIMIT = 200;

/** When the page was last shown after being hidden, or restored from the
 *  back-forward cache. */
let lastShownAt = null;
/** The outage being ridden out — from the first failure to the next commit —
 *  or null while the database answers. */
let outage = null;
let restingUntil = 0;
let restTimer = null;
let stoodDown = null;
let lastRecovery = null;
/** The last write the database refused on its own account, or null. */
let lastRefused = null;
/** Whether the database has answered anything yet in this page. */
let answered = false;
/** When the open now pending was blocked by another tab, or null. */
let blockedSince = null;

const errorFields = (error) => ({
  error: error?.name || (error ? "Error" : "none"),
  message: String(error?.message || "").slice(0, MESSAGE_LIMIT),
});

const visibility = () => globalThis.document?.visibilityState || "unknown";

function cacheEvent(event, detail = {}) {
  const entry = {
    ...detail,
    visibility: visibility(),
    sinceShownMs: lastShownAt === null ? null : Date.now() - lastShownAt,
  };
  recordConnectionDiagnostic(CACHE_DIAGNOSTIC, event, entry);
  (event === "cache-stood-down" ? console.warn : console.info)(`local cache: ${event}`, entry);
}

/** What the cache is doing right now: `unused` (nothing asked of it yet),
 *  `ready`, `recovering` (reopening after a
 *  lost connection), `resting` (between rounds of reopening), `blocked`
 *  (waiting for another tab to close an older connection), `stood-down` (for
 *  the session), or `absent` (this browser has no IndexedDB). With when it
 *  started and the error behind it. */
export function cacheHealth() {
  if (typeof indexedDB === "undefined") return { state: "absent" };
  if (disabled) return { state: "stood-down", ...stoodDown };
  if (blockedSince) return { state: "blocked", since: blockedSince };
  if (!outage) return answered ? { state: "ready", lastRecovery, lastRefused } : { state: "unused" };
  return {
    state: Date.now() < restingUntil ? "resting" : "recovering",
    since: outage.since,
    reason: outage.reason,
    attempts: outage.attempts,
    ...errorFields(outage.error),
  };
}

function connectionLost(reason, error) {
  if (!outage) {
    outage = { since: Date.now(), reason, error, attempts: 0 };
    cacheEvent("cache-connection-lost", { reason, ...errorFields(error) });
  }
  outage.error = error || outage.error;
  outage.attempts += 1;
}

/** A transaction committed: whatever outage there was is over. */
function recovered() {
  if (!outage) return;
  const { since, attempts } = outage;
  outage = null;
  lastRecovery = { at: Date.now(), afterMs: Date.now() - since, attempts };
  cacheEvent("cache-recovered", { afterMs: lastRecovery.afterMs, attempts });
}

/** The attempts to open ran out: stop trying until a wake, or until the rest
 *  is over. Whatever asks meanwhile waits for it. */
function rest() {
  if (!outage) connectionLost("unanswered");
  restingUntil = Date.now() + timing.restMs;
  cacheEvent("cache-resting", { attempts: outage.attempts, forMs: timing.restMs, ...errorFields(outage.error) });
  clearTimeout(restTimer);
  restTimer = setTimeout(() => wake("rested"), timing.restMs);
  restTimer?.unref?.();
}

function standDown(reason, error) {
  if (!disabled) {
    stoodDown = { at: Date.now(), reason, ...errorFields(error) };
    cacheEvent("cache-stood-down", { reason, ...errorFields(error) });
  }
  disabled = true;
  dbPromise = null;
  outage = null;
  wake("stood-down"); // nobody waits on a cache that is not coming back
}

// ─── Waiting it out ──────────────────────────────────────────────────────────

let watchingPage = false;
let shownWaiters = [];
let wakeWaiters = [];

const pageHidden = () => globalThis.document?.visibilityState === "hidden";

const release = (waiters) => {
  for (const resolve of waiters) resolve();
};

/** Settles when the page is visible — at once if it already is. A retry spent
 *  on a suspended page fails for the reason it was waiting out. */
const whenShown = () => (pageHidden() ? new Promise((resolve) => shownWaiters.push(resolve)) : Promise.resolve());

const sleep = (ms) => new Promise((resolve) => {
  const timer = setTimeout(resolve, ms);
  timer?.unref?.();
});

/** The wait before a retry: its delay, and then the page being shown. */
const pause = (ms) => sleep(ms).then(whenShown);

/** Settles on the next wake, or after `ms` without one. */
const untilWake = (ms) => Promise.race([new Promise((resolve) => wakeWaiters.push(resolve)), sleep(ms)]);

/** A wake: the page was shown or restored, the network came back, or a rest
 *  ran out. Ends a rest, and lets everything waiting on one try again now. */
function wake(reason) {
  if (reason === "visible" || reason === "pageshow") {
    lastShownAt = Date.now();
    // A resumed page reopens at the front of the backoff, not after the delay
    // the outage had reached while it slept.
    if (outage) outage.attempts = 0;
    const shown = shownWaiters;
    shownWaiters = [];
    release(shown);
  }
  restingUntil = 0;
  clearTimeout(restTimer);
  const waiting = wakeWaiters;
  wakeWaiters = [];
  release(waiting);
}

function watchPage() {
  if (watchingPage) return;
  watchingPage = true;
  const doc = globalThis.document;
  doc?.addEventListener?.("visibilitychange", () => {
    if (doc.visibilityState !== "hidden") wake("visible");
  });
  globalThis.addEventListener?.("pageshow", () => wake("pageshow"));
  globalThis.addEventListener?.("online", () => wake("online"));
}

// ─── Opening ─────────────────────────────────────────────────────────────────

function invalidateDb(db, promise) {
  if (dbPromise !== promise) return false;
  dbPromise = null;
  try { db.close(); } catch { /* already closed by the browser */ }
  return true;
}

function namedError(name, message) {
  const error = new Error(message);
  error.name = name;
  return error;
}

function upgrade(request) {
  const db = request.result;
  if (db.objectStoreNames.contains(STORE)) db.deleteObjectStore(STORE);
  db.createObjectStore(STORE).createIndex(AT_INDEX, "at");
}

/** One `indexedDB.open`: the database, or the error it failed with — and
 *  `blocked` when another tab held it past the wait. A request that answers
 *  after it was given up on closes what it opened. */
function openOnce() {
  return new Promise((resolve) => {
    let settled = false;
    let blocked = false;
    let timer = null;
    const giveUpAfter = (ms, outcome) => {
      clearTimeout(timer);
      timer = setTimeout(() => settle(outcome), ms);
      timer?.unref?.();
    };
    const settle = (outcome) => {
      if (settled) return outcome.db?.close();
      settled = true;
      clearTimeout(timer);
      if (blocked) blockedSince = null;
      resolve(outcome);
    };
    let request;
    try {
      request = indexedDB.open(DB_NAME, DB_VERSION);
    } catch (error) {
      return settle({ error });
    }
    giveUpAfter(timing.openTimeoutMs, { error: namedError("TimeoutError", "opening the cache database did not answer") });
    request.onupgradeneeded = () => upgrade(request);
    request.onsuccess = () => settle({ db: request.result });
    request.onerror = (event) => {
      event?.preventDefault?.();
      settle({ error: request.error });
    };
    // Another tab holds an older version open. It is told to close (see
    // `onversionchange` below); this open goes through the moment it does,
    // unless that tab never listens.
    request.onblocked = () => {
      if (blocked || settled) return;
      blocked = true;
      blockedSince = Date.now();
      cacheEvent("cache-open-blocked", {});
      giveUpAfter(timing.blockedTimeoutMs, {
        blocked: true,
        error: namedError("BlockedError", "another tab kept an older version of the database open"),
      });
    };
  });
}

/** How long the cache has been failing in front of someone: since the outage
 *  began or the page was last shown, whichever is later. None while hidden. */
function visibleFailingMs() {
  if (!outage || pageHidden()) return 0;
  return Date.now() - Math.max(outage.since, lastShownAt ?? 0);
}

/** A round of attempts ran out. On a page that has never opened the database,
 *  or one that has watched it fail past the ceiling, the failure is the
 *  database's and the cache stands down, answering everything waiting on it.
 *  True when it did. */
function outlasted() {
  if (pageHidden()) return false;
  if (!everOpened || visibleFailingMs() >= timing.giveUpAfterMs) standDown("persistent", outage?.error);
  return disabled;
}

/** Reach the database through a lost connection: every attempt on the
 *  backoff, resting when they run out. A reopen during an outage starts as far
 *  along the backoff as the outage already is. The backoff lives here and not
 *  in the operations: everything that wants the database waits on this one
 *  opening, and goes through it in the order it asked — so a write retried
 *  after a failed transaction still lands before one asked for after it. */
async function reachDb() {
  const delays = timing.reopenDelaysMs;
  const along = outage ? outage.attempts : 0;
  for (let attempt = 0; attempt < delays.length; attempt += 1) {
    const step = attempt + along;
    if (step > 0) await pause(delays[Math.min(step, delays.length - 1)]);
    if (disabled) return null;
    const { db, error, blocked } = await openOnce();
    if (db) return db;
    if (blocked || faultOf(error, "open") !== "connection") {
      standDown(blocked ? "blocked" : "open-failed", error);
      return null;
    }
    connectionLost("open-failed", error);
  }
  if (!outlasted()) rest();
  return null;
}

/** Keep a fresh connection honest: note when the browser takes it, and step
 *  aside for a newer version opening in another tab. */
function watchConnection(db, opening) {
  db.onclose = () => {
    if (dbPromise === opening) dbPromise = null;
    cacheEvent("cache-connection-closed", {});
  };
  db.onversionchange = () => {
    invalidateDb(db, opening);
    cacheEvent("cache-yielded", { reason: "a newer version is opening" });
  };
}

function openDb() {
  if (disabled || typeof indexedDB === "undefined") return Promise.resolve(null);
  if (dbPromise) return dbPromise;
  watchPage();
  if (Date.now() < restingUntil) return Promise.resolve(null);
  const opening = reachDb().then((db) => {
    if (!db || disabled || dbPromise !== opening) {
      db?.close();
      if (dbPromise === opening) dbPromise = null;
      return null;
    }
    everOpened = true;
    watchConnection(db, opening);
    return db;
  });
  dbPromise = opening;
  return opening;
}

// ─── Transactions ────────────────────────────────────────────────────────────

/** Record why our callback aborted. A storage call can fail because its
 *  connection closed (`connection`); a caller's update or merge error, or a
 *  value the store cannot hold, fails that write alone (`write`). */
function abortForError(store, error, fault = "write") {
  intentionalAborts.set(store.transaction, { error, fault });
  store.transaction.abort();
}

function putOrAbort(store, record, key) {
  try {
    store.put(record, key);
    return true;
  } catch (error) {
    abortForError(store, error, faultOf(error));
    return false;
  }
}

/** One attempt: the transaction's outcome, and whose fault it was if it
 *  did not commit. */
function attemptTransaction(db, mode, run) {
  return new Promise((resolve) => {
    let request;
    try {
      const transaction = db.transaction(STORE, mode);
      request = run(transaction.objectStore(STORE));
      transaction.onabort = () => {
        const marked = intentionalAborts.get(transaction);
        const error = marked ? marked.error : transaction.error;
        resolve({ committed: false, error, fault: marked ? marked.fault : faultOf(error) });
      };
      transaction.oncomplete = () =>
        resolve({ committed: true, result: request ? request.result : undefined });
    } catch (error) {
      resolve({ committed: false, error, fault: faultOf(error) });
    }
  });
}

const UNAVAILABLE = Object.freeze({ committed: false, unavailable: true });

/** What an attempt's outcome settles, or null when the connection failed it
 *  and the attempt is worth making again. */
function settled(outcome) {
  if (outcome.committed || outcome.fault === "write") answered = true;
  if (outcome.committed) {
    recovered();
    return outcome;
  }
  if (outcome.fault === "write") {
    lastRefused = { at: Date.now(), ...errorFields(outcome.error) };
    cacheEvent("cache-write-refused", errorFields(outcome.error));
    return outcome;
  }
  if (outcome.fault === "database") {
    standDown("transaction-failed", outcome.error);
    return { ...outcome, unavailable: true };
  }
  return null;
}

/** The database, once it can be had: null only when it never will be (no
 *  IndexedDB, or stood down). While the cache rests this waits for the wake. */
async function reachableDb() {
  for (;;) {
    const opening = dbPromise || openDb();
    const db = await opening;
    if (db) return { db, opening };
    if (disabled || typeof indexedDB === "undefined") return null;
    await untilWake(timing.restMs);
  }
}

/** One operation's attempts on a connection that keeps failing it. Waiting
 *  for the database to come back spends none of them, and there is no pause
 *  between them: the reopen carries the backoff (see `reachDb`). */
async function attemptOperation(mode, run) {
  const delays = timing.reopenDelaysMs;
  let outcome = UNAVAILABLE;
  for (let attempt = 0; attempt < delays.length; attempt += 1) {
    const reached = await reachableDb();
    if (!reached) return UNAVAILABLE;
    outcome = await attemptTransaction(reached.db, mode, run);
    const done = settled(outcome);
    if (done) return done;
    connectionLost("transaction-failed", outcome.error);
    invalidateDb(reached.db, reached.opening);
  }
  if (outlasted()) return UNAVAILABLE;
  // Said once an outage: a read waiting it out runs out again every round.
  if (!outage.saidExhausted) cacheEvent("cache-operation-failed", { mode, attempts: delays.length, ...errorFields(outcome.error) });
  outage.saidExhausted = true;
  return { ...outcome, unavailable: true, exhausted: true };
}

/** One transaction, one operation, resolved when the transaction settles with
 *  what it did: whether it committed, and the result of the request `run`
 *  returned. `run` gets the store and returns an IDBRequest (or null for
 *  delete-ranges, where the transaction's own completion is the answer).
 *
 *  A read is never answered "nothing" because the database was away: one
 *  whose attempts all failed waits for the next wake and asks again, until it
 *  is answered or the cache stands down — which a failure that outlasts the
 *  weather makes it do (`outlasted`). Most writes fail alone after a round;
 *  only callers whose result cannot be skipped opt into waiting as reads do. */
async function transact(mode, run, waitForRecovery = mode === "readonly") {
  for (;;) {
    const outcome = await attemptOperation(mode, run);
    if (!outcome.exhausted || !waitForRecovery) return outcome;
    await untilWake(timing.restMs);
  }
}

/** A read: what was read, or undefined when there was nothing to read from. */
const inStore = (mode, run) => transact(mode, run).then((done) => done.result);

/** A write: whether the store actually changed. Only a transaction that
 *  committed is announced — a private window that refuses IndexedDB, or a
 *  database that is away, would otherwise send every subscriber to re-read a
 *  record that was never written and blank a surface that was painting the
 *  right thing a frame earlier. */
const wroteStore = (run, waitForRecovery = false) =>
  transact("readwrite", run, waitForRecovery).then((done) => done.committed);

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
function writeFeed(address, update, { observedFeedRows = false, supersededFeedRow, unchanged = [] } = {}) {
  const key = recordKey(address);
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    // Read and put in the same transaction: another tab can write between a
    // separate read and write, and its newer row observation must survive.
    readRecordsInStore(store, [key, ...unchanged.map(({ address }) => recordKey(address))], ([previous, ...guards]) => {
      try {
        if (!unchanged.every(({ written }, index) => isCachedWrite(guards[index], written))) return;
        const next = update(previous?.value, previous);
        if (next == null) return;
        const { write, ...stamp } = writeStamp();
        const record = { ...stamp, write, value: feedWithObservations(next, previous, stamp, observedFeedRows, supersededFeedRow) };
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
export function writeCached(address, value, { source, sequence, observedFeedRows = false } = {}) {
  if (address.kind === "feed") return writeFeed(address, () => value, { observedFeedRows });
  if (address.kind === "bridge-update") return writeBridgeUpdate(address, value);
  const key = recordKey(address);
  const record = { ...writeStamp(), value, ...(source ? { source, sequence } : {}) };
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
  return mergeRecordAtomically(address, (record) => merge(record?.value));
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

function mergeRecordAtomically(address, merge) {
  const key = recordKey(address);
  let changed = false;
  return wroteStore((store) => {
    changed = false;
    const request = store.get(key);
    request.onsuccess = () => {
      try {
        const next = merge(request.result);
        if (next == null) return;
        const record = withBridgeGeneration(address, { ...writeStamp(), value: next });
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
