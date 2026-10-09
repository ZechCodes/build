// Read-only RPC coordination shared by every surface in this tab.
//
// A cache warmer and the surface a reader opens can ask for the same payload
// through different repository instances. The request key names that payload;
// while it is pending with active readers, callers share its promise. Foreground
// reads start at once and hold background reads until the foreground is quiet.
// Background reads then give rendering one idle turn before they use the wire.

const IDLE_DEADLINE_MS = 2000;

/**
 * The request-envelope fields a read of this priority rides with (wire spec
 * step 1.4). Background is stamped so the bridge's dispatcher keeps a cache
 * warm-up behind the focused surface's reads; foreground stamps nothing,
 * because absence is what every bridge — this one and the one that predates
 * the field — reads as foreground.
 */
export function requestPriorityFields(priority) {
  return priority === "background" ? { priority: "background" } : {};
}

const pending = new Map(); // request key -> { promise, promote, active, join }
const foregroundWaiters = new Set();
let activeForeground = 0;
const identities = new WeakMap();
let nextIdentity = 0;

function identityOf(value) {
  if ((typeof value !== "object" || value === null) && typeof value !== "function") {
    return `${typeof value}:${String(value)}`;
  }
  let identity = identities.get(value);
  if (!identity) {
    identity = `object:${++nextIdentity}`;
    identities.set(value, identity);
  }
  return identity;
}

/** Names one RPC answer without allowing equal ids from replacement app
 * sessions, repositories, or RPC transports to accidentally share a read. */
export function rpcReadKey({ deviceId, requestScope, repository, call, method, params }) {
  return JSON.stringify([
    deviceId || null,
    identityOf(requestScope),
    identityOf(repository),
    identityOf(call),
    method,
    params,
  ]);
}

function idleTurn() {
  return new Promise((resolve) => {
    if (typeof requestIdleCallback === "function") requestIdleCallback(resolve, { timeout: IDLE_DEADLINE_MS });
    else setTimeout(resolve, 0);
  });
}

function foregroundQuiet() {
  if (!activeForeground) return Promise.resolve();
  return new Promise((resolve) => foregroundWaiters.add(resolve));
}

function finishForeground() {
  activeForeground -= 1;
  if (activeForeground) return;
  for (const resolve of foregroundWaiters) resolve();
  foregroundWaiters.clear();
}

async function startInBackground(start, alreadyStarted) {
  // A foreground read can begin after the idle callback was requested. Check
  // again after yielding so background traffic never starts alongside it. A
  // foreground caller can also promote this operation while either wait is in
  // progress; the guarded start then makes the abandoned turn harmless.
  do {
    await foregroundQuiet();
    if (alreadyStarted()) return;
    await idleTurn();
    if (alreadyStarted()) return;
  } while (activeForeground);
  start(false);
}

/** A queued loader belongs to the reader that supplied it. Another reader may
 *  still need the shared request after its original reader has stood down. */
function loadActiveReader(readers, envelope) {
  const reader = readers.find((candidate) => candidate.active());
  return reader ? reader.load(envelope) : null;
}

function newRead(priority, reader) {
  const readers = [reader];
  let started = false;
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });

  const start = (asForeground) => {
    if (started) return;
    started = true;
    if (asForeground) activeForeground += 1;
    // A read promoted to foreground crosses the wire as one: the envelope is
    // stamped where the load starts, not where it was queued.
    Promise.resolve()
      .then(() => loadActiveReader(readers, requestPriorityFields(asForeground ? "foreground" : priority)))
      .then(resolve, reject)
      .finally(() => {
        if (asForeground) finishForeground();
      });
  };

  if (priority === "foreground") start(true);
  else void startInBackground(start, () => started);
  return {
    promise,
    promote: () => start(true),
    active: () => readers.some((candidate) => candidate.active()),
    join: (candidate) => readers.push(candidate),
  };
}

function validateRead(key, priority, load) {
  if (key === undefined || key === null) throw new TypeError("coordinatedRead requires a key");
  if (priority !== "foreground" && priority !== "background") {
    throw new TypeError(`unknown read priority: ${priority}`);
  }
  if (typeof load !== "function") throw new TypeError("coordinatedRead requires a load function");
}

/**
 * Share one read-only operation by `key` across all callers in this tab.
 *
 * The caller owns the key and must include every field that changes the
 * answer, including the device/session, method, and params. A foreground read
 * starts immediately. A background read waits for active foreground reads and
 * one browser idle turn. `load` is handed the request-envelope fields its
 * priority rides with, to pass to the session call it makes. Each caller's
 * optional `active` predicate names its lifetime. At dispatch the request uses
 * an active caller's loader, or settles with null if every caller has stopped.
 * A caller replaces a pending request whose readers are all inactive rather
 * than waiting on their abandoned work. Settled reads are forgotten, whether
 * they fulfilled or rejected, so a later call can refresh or retry.
 */
export function coordinatedRead({ key, priority = "foreground", load, active = () => true }) {
  validateRead(key, priority, load);
  const reader = { load, active };
  const existing = pending.get(key);
  if (existing?.active()) {
    existing.join(reader);
    if (priority === "foreground") existing.promote();
    return existing.promise;
  }

  const read = newRead(priority, reader);
  const entry = { ...read };
  entry.promise = read.promise.finally(() => {
    if (pending.get(key) === entry) pending.delete(key);
  });
  pending.set(key, entry);
  return entry.promise;
}
