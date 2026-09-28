// One IndexedDB database the SPA keeps open through whatever the browser does
// to it. The replica cache (core/localCache.js) and the local UI store
// (core/localUiStore.js) are each one of these: separate databases, separately
// versioned, so a schema change to the replica can never reach a draft.

import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";

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
// run out the database rests until the next wake — the page shown, the network
// back, or the rest simply over — and tries again. Nothing that asks while it
// is away is answered "nothing": a read waits for the database to come back,
// so a surface that painted keeps what it painted until a read really answers,
// and a merge never mistakes "unreadable" for "empty". A write waits too, and
// is made once the database is back.
//
// Only an error no reopen can fix stands the database down for the session: a
// private window refusing IndexedDB, a schema that is not ours.
// So does one that outlasts the weather: a store the browser can never open
// (Safari raises the same UnknownError for a corrupt one) fails every round,
// and a page someone is looking at does not wait on it for ever — nor does a
// page that has never opened it at all, whose boot paint is waiting.
// A write the database refuses on its own account (a value it cannot clone, one
// that does not fit in a full quota, or one that keeps failing while every
// other transaction commits) fails alone: the records already stored still
// read, and the pull that carried it asks again.

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
   *  reloaded — so past this the database stands down, and the page paints
   *  from the network as it would without one. */
  blockedTimeoutMs: 10_000,
  /** How long the database rests after the attempts run out, unless a wake
   *  ends it first. Short enough that storage coming back on a page nobody
   *  hides is noticed within seconds. */
  restMs: 10_000,
  /** How long the database may keep failing on a page someone is looking at
   *  before the failure counts as the database's own. Counted from the later
   *  of the outage starting and the page last being shown: a hidden page waits
   *  on, and a resume starts the count again. */
  giveUpAfterMs: 60_000,
});

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

const intentionalAborts = new WeakMap();

/** Record why our callback aborted. A storage call can fail because its
 *  connection closed (`connection`); a caller's update or merge error, or a
 *  value the store cannot hold, fails that write alone (`write`). */
export function abortForError(store, error, fault = "write") {
  intentionalAborts.set(store.transaction, { error, fault });
  store.transaction.abort();
}

export function putOrAbort(store, record, key) {
  try {
    store.put(record, key);
    return true;
  } catch (error) {
    abortForError(store, error, faultOf(error));
    return false;
  }
}

const MESSAGE_LIMIT = 200;

const errorFields = (error) => ({
  error: error?.name || (error ? "Error" : "none"),
  message: String(error?.message || "").slice(0, MESSAGE_LIMIT),
});

const visibility = () => globalThis.document?.visibilityState || "unknown";
const pageHidden = () => globalThis.document?.visibilityState === "hidden";

const release = (waiters) => {
  for (const resolve of waiters) resolve();
};

const sleep = (ms) => new Promise((resolve) => {
  const timer = setTimeout(resolve, ms);
  timer?.unref?.();
});

function namedError(name, message) {
  const error = new Error(message);
  error.name = name;
  return error;
}

const UNAVAILABLE = Object.freeze({ committed: false, unavailable: true });

/**
 * One database: `name` at `version`, one object `store`, built by `upgrade`
 * (handed the open request) whenever the version on disk is older.
 * `diagnostic` names it in the connection diagnostics ring and `label` on the
 * console.
 *
 * Answers `transact` (one transaction, see below), its `read`/`write`
 * shorthands, `health`, `available` and `setRecoveryTiming`.
 */
export function createIdbDatabase({ name, version, store: storeName, upgrade, diagnostic, label }) {
  /** Set when the database met an error no reopen can fix; it answers nothing
   *  for the rest of the session. */
  let disabled = false;
  let dbPromise = null;
  /** Whether the database has opened at all in this page. Before it has, a
   *  round of failed opens is not a resume's weather but a store that cannot
   *  be opened — and the boot paint is waiting on it. */
  let everOpened = false;
  let timing = DEFAULT_TIMING;

  // ─── What happened to it ───────────────────────────────────────────────────
  //
  // Every loss, rest, recovery and stand-down is recorded in the connection
  // diagnostics ring (Settings → Diagnostics, and `buildConnectionDiagnostics()`)
  // and on the console, with the page's visibility and how long ago it was
  // last shown — the facts that say whether a blank screen followed a resume.
  // `health` answers the state right now, for the line under the dump.

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

  function cacheEvent(event, detail = {}) {
    const entry = {
      ...detail,
      visibility: visibility(),
      sinceShownMs: lastShownAt === null ? null : Date.now() - lastShownAt,
    };
    recordConnectionDiagnostic(diagnostic, event, entry);
    (event === "cache-stood-down" ? console.warn : console.info)(`${label}: ${event}`, entry);
  }

  /** What the database is doing right now: `unused` (nothing asked of it
   *  yet), `ready`, `recovering` (reopening after a lost connection),
   *  `resting` (between rounds of reopening), `blocked` (waiting for another
   *  tab to close an older connection), `stood-down` (for the session), or
   *  `absent` (this browser has no IndexedDB). With when it started and the
   *  error behind it. */
  function health() {
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

  /** For tests: a faster schedule. Answers the one it replaced. */
  function setRecoveryTiming(next) {
    const was = timing;
    timing = next ? { ...DEFAULT_TIMING, ...next } : DEFAULT_TIMING;
    return was;
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

  /** The attempts to open ran out: stop trying until a wake, or until the
   *  rest is over. Whatever asks meanwhile waits for it. */
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
    wake("stood-down"); // nobody waits on a database that is not coming back
  }

  // ─── Waiting it out ────────────────────────────────────────────────────────

  let watchingPage = false;
  let shownWaiters = [];
  let wakeWaiters = [];

  /** Settles when the page is visible — at once if it already is. A retry
   *  spent on a suspended page fails for the reason it was waiting out. */
  const whenShown = () => (pageHidden() ? new Promise((resolve) => shownWaiters.push(resolve)) : Promise.resolve());

  /** The wait before a retry: its delay, and then the page being shown. */
  const pause = (ms) => sleep(ms).then(whenShown);

  /** Settles on the next wake, or after `ms` without one. */
  const untilWake = (ms) => Promise.race([new Promise((resolve) => wakeWaiters.push(resolve)), sleep(ms)]);

  /** A wake: the page was shown or restored, the network came back, or a rest
   *  ran out. Ends a rest, and lets everything waiting on one try again now. */
  function wake(reason) {
    if (reason === "visible" || reason === "pageshow") {
      lastShownAt = Date.now();
      // A resumed page reopens at the front of the backoff, not after the
      // delay the outage had reached while it slept.
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

  // ─── Opening ───────────────────────────────────────────────────────────────

  function invalidateDb(db, promise) {
    if (dbPromise !== promise) return false;
    dbPromise = null;
    try { db.close(); } catch { /* already closed by the browser */ }
    return true;
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
        request = indexedDB.open(name, version);
      } catch (error) {
        return settle({ error });
      }
      giveUpAfter(timing.openTimeoutMs, { error: namedError("TimeoutError", `opening ${label} did not answer`) });
      request.onupgradeneeded = (event) => upgrade(request, event);
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

  /** Delete the database: `{}` once it is gone, or the error, or `blocked`
   *  when another tab held it past the wait. */
  function deleteDatabase() {
    return new Promise((resolve) => {
      let request;
      try {
        request = indexedDB.deleteDatabase(name);
      } catch (error) {
        return resolve({ error });
      }
      const timer = setTimeout(() => resolve({
        blocked: true,
        error: namedError("BlockedError", "another tab kept the newer database open"),
      }), timing.blockedTimeoutMs);
      timer?.unref?.();
      request.onsuccess = () => {
        clearTimeout(timer);
        resolve({});
      };
      request.onerror = (event) => {
        event?.preventDefault?.();
        clearTimeout(timer);
        resolve({ error: request.error });
      };
    });
  }

  /** Open this build's version. A database a newer build left behind — this
   *  build was rolled back to — answers VersionError, which no reopen fixes:
   *  it is dropped and starts cold, rather than standing down on every page
   *  load until someone clears the site's data. */
  async function openThisVersion() {
    const opened = await openOnce();
    if (opened.error?.name !== "VersionError") return opened;
    cacheEvent("cache-dropped-newer", {});
    const dropped = await deleteDatabase();
    return dropped.error ? dropped : openOnce();
  }

  /** How long the database has been failing in front of someone: since the
   *  outage began or the page was last shown, whichever is later. None while
   *  hidden. */
  function visibleFailingMs() {
    if (!outage || pageHidden()) return 0;
    return Date.now() - Math.max(outage.since, lastShownAt ?? 0);
  }

  /** A round of attempts ran out. On a page that has never opened the
   *  database, or one that has watched it fail past the ceiling, the failure
   *  is the database's and it stands down, answering everything waiting on
   *  it. True when it did. */
  function outlasted() {
    if (pageHidden()) return false;
    if (!everOpened || visibleFailingMs() >= timing.giveUpAfterMs) standDown("persistent", outage?.error);
    return disabled;
  }

  /** Reach the database through a lost connection: every attempt on the
   *  backoff, resting when they run out. A reopen during an outage starts as
   *  far along the backoff as the outage already is. The backoff lives here
   *  and not in the operations: everything that wants the database waits on
   *  this one opening, and goes through it in the order it asked — so a write
   *  retried after a failed transaction still lands before one asked for
   *  after it. */
  async function reachDb() {
    const delays = timing.reopenDelaysMs;
    const along = outage ? outage.attempts : 0;
    for (let attempt = 0; attempt < delays.length; attempt += 1) {
      const step = attempt + along;
      if (step > 0) await pause(delays[Math.min(step, delays.length - 1)]);
      if (disabled) return null;
      const { db, error, blocked } = await openThisVersion();
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

  // ─── Transactions ──────────────────────────────────────────────────────────

  /** One attempt: the transaction's outcome, and whose fault it was if it
   *  did not commit. */
  function attemptTransaction(db, mode, run) {
    return new Promise((resolve) => {
      let request;
      try {
        const transaction = db.transaction(storeName, mode);
        request = run(transaction.objectStore(storeName));
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
   *  IndexedDB, or stood down). While it rests this waits for the wake. */
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

  /** One transaction, one operation, resolved when the transaction settles
   *  with what it did: whether it committed, and the result of the request
   *  `run` returned. `run` gets the store and returns an IDBRequest (or null
   *  for delete-ranges, where the transaction's own completion is the answer).
   *
   *  A read is never answered "nothing" because the database was away: one
   *  whose attempts all failed waits for the next wake and asks again, until
   *  it is answered or the database stands down — which a failure that
   *  outlasts the weather makes it do (`outlasted`). Most writes fail alone
   *  after a round; only callers whose result cannot be skipped opt into
   *  waiting as reads do. */
  async function transact(mode, run, waitForRecovery = mode === "readonly") {
    for (;;) {
      const outcome = await attemptOperation(mode, run);
      if (!outcome.exhausted || !waitForRecovery) return outcome;
      await untilWake(timing.restMs);
    }
  }

  return {
    transact,
    /** A read: what was read, or undefined when there was nothing to read
     *  from. */
    read: (mode, run) => transact(mode, run).then((done) => done.result),
    /** A write: whether the store actually changed. Only a transaction that
     *  committed is announced — a private window that refuses IndexedDB, or a
     *  database that is away, would otherwise send every subscriber to
     *  re-read a record that was never written and blank a surface that was
     *  painting the right thing a frame earlier. */
    write: (run, waitForRecovery = false) =>
      transact("readwrite", run, waitForRecovery).then((done) => done.committed),
    health,
    available: async () => Boolean(await openDb()),
    setRecoveryTiming,
  };
}
