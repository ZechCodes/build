// Read-only RPC coordination shared by every surface in this tab.
//
// A cache warmer and the surface a reader opens can ask for the same payload
// through different repository instances. The request key names that payload;
// while it is pending, every caller receives the same promise. Foreground
// reads start at once and hold background reads until the foreground is quiet.
// Background reads then give rendering one idle turn before they use the wire.

const IDLE_DEADLINE_MS = 2000;

const pending = new Map(); // request key -> { promise, promote }
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

function newRead(priority, load) {
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
    Promise.resolve()
      .then(load)
      .then(resolve, reject)
      .finally(() => {
        if (asForeground) finishForeground();
      });
  };

  if (priority === "foreground") start(true);
  else void startInBackground(start, () => started);
  return { promise, promote: () => start(true) };
}

/**
 * Share one read-only operation by `key` across all callers in this tab.
 *
 * The caller owns the key and must include every field that changes the
 * answer, including the device/session, method, and params. A foreground read
 * starts immediately. A background read waits for active foreground reads and
 * one browser idle turn. Settled reads are forgotten, whether they fulfilled
 * or rejected, so a later call can refresh or retry.
 */
export function coordinatedRead({ key, priority = "foreground", load }) {
  if (key === undefined || key === null) throw new TypeError("coordinatedRead requires a key");
  if (priority !== "foreground" && priority !== "background") {
    throw new TypeError(`unknown read priority: ${priority}`);
  }
  if (typeof load !== "function") throw new TypeError("coordinatedRead requires a load function");

  const existing = pending.get(key);
  if (existing) {
    if (priority === "foreground") existing.promote();
    return existing.promise;
  }

  const read = newRead(priority, load);
  const entry = { ...read };
  entry.promise = read.promise.finally(() => {
    if (pending.get(key) === entry) pending.delete(key);
  });
  pending.set(key, entry);
  return entry.promise;
}
