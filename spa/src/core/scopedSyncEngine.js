// Scope-independent durable recovery. Tokens stay opaque; only server heads
// are compared, and publication owns the cache/cursor transaction boundary.
export const SYNC_HEAD_INTERVAL_MS = 30_000;
const RETRY_DELAYS_MS = [1000, 2000, 5000, 10_000, 30_000];
// Registry reconciliation can last for a clone. It is ordinary cache weather,
// never a displayed failure, and must not extend a captured connection period.
const reconciling = (error) => (error?.code || error?.error_code) === "unavailable"
  && error?.retryable === true && error?.message === "sync_reconciling";
const MAX_RESTARTS = 3;
const MAX_SNAPSHOT_RECORDS = 100_000;
const MAX_SNAPSHOT_PAGES = 1000;
const identity = (scope) => JSON.stringify(scope);
const pageParams = (page) => page ? { page } : {};
const cursorParams = (held) => held?.cursor ? { cursor: held.cursor } : {};
const cursorOf = (held) => held?.cursor ?? null;
const caughtUp = (held, answer) => Boolean(held?.cursor && !held.snapshotRequired
  && held.incarnation === answer.incarnation && held.appliedHead === answer.head);
const needsSnapshot = (entry, held) => entry.fresh || held?.snapshotRequired;
function publishedSnapshot(entry, first, committed) {
  if (!committed) return null;
  entry.fresh = false;
  return { cursor: first.cursor };
}

export function createScopedSyncEngine({ request, storeFor, active = () => true, epoch = () => null, captureRequest, onError = () => {} }) {
  const scopes = new Map();
  const opening = captureRequest || epochOpenings(epoch);
  let disposed = false;
  const current = () => !disposed && active();
  const timer = setInterval(() => { void checkHeads(); }, SYNC_HEAD_INTERVAL_MS);
  timer.unref?.();

  function track(scope) {
    const key = identity(scope);
    if (scopes.has(key)) return;
    scopes.set(key, { scope, store: storeFor(scope), running: null, trailing: null, fresh: true,
      opening: null, validateCursor: false, lastCursor: null, retry: null, attempts: 0, failures: 0 });
  }
  const stands = (entry) => current() && scopes.get(identity(entry.scope)) === entry;
  // A scope can outlive its transport; each operation belongs to exactly
  // one accepted session/hello opening through its publication transaction.
  const accepts = (entry, token) => stands(entry) && entry.opening === token && token.current();

  function takeOpening(entry) {
    const token = opening();
    if (!token?.current()) return null;
    if (entry.opening !== token) {
      entry.validateCursor = !entry.fresh;
      entry.opening = token;
    }
    return token;
  }

  async function read(entry, token, method, params) {
    if (!accepts(entry, token)) return null;
    const answer = await request(method, { scope: entry.scope, ...params });
    if (!accepts(entry, token)) return null;
    if (token.epoch && answer?.bridge_epoch !== token.epoch) return null;
    return answer;
  }

  async function publish(entry, token, batch) {
    const accept = () => accepts(entry, token);
    if (!accept()) return false;
    const committed = await entry.store.publish({ ...batch, accept });
    if (!committed) return false;
    if (entry.opening === token) entry.lastCursor = batch.removed ? null : batch.response.cursor;
    return accept();
  }

  async function reset(entry, token, answer, capture, expectedCursor) {
    const accept = () => accepts(entry, token);
    if (!accept()) return false;
    if (answer.reason === "scope_removed") {
      await publish(entry, token, { response: answer, records: [], capture, expectedCursor, removed: true });
      return false;
    }
    const committed = await entry.store.reset({ capture, expectedCursor, accept });
    if (!committed) return false;
    if (entry.opening === token) entry.lastCursor = null;
    return accept();
  }

  async function snapshot(entry, token, capture, expectedCursor) {
    let page, first, pages = 0;
    const records = [];
    do {
      const answer = await read(entry, token, "sync.snapshot", pageParams(page));
      pages += 1;
      if (!answer) return null;
      if (answer.status === "resync_required") return { resync: answer };
      if (answer.status !== "ok") return null;
      first ||= answer;
      if (!sameSnapshot(first, answer)) return null;
      records.push(...answer.records);
      if (snapshotExceeded(records, answer, page, pages)) return null;
      page = answer.next_page;
    } while (page);
    const committed = await publish(entry, token, { response: first, records, capture, expectedCursor, snapshot: true });
    return publishedSnapshot(entry, first, committed);
  }

  async function changes(entry, token, origin) {
    let cursor = origin, continuation;
    do {
      const capture = await entry.store.capture();
      const answer = await read(entry, token, "sync.changes", { cursor: origin, ...(continuation ? { continuation } : {}) });
      if (!answer) return false;
      if (answer.status === "resync_required") return { resync: answer, capture, cursor };
      if (answer.status !== "ok") return false;
      if (!await publish(entry, token, { response: answer, records: answer.changes, capture,
        expectedCursor: cursor })) return false;
      if (answer.continuation && answer.cursor === cursor) return false;
      cursor = answer.cursor;
      continuation = answer.continuation;
    } while (continuation);
    return true;
  }

  async function recover(entry, token) {
    for (let attempt = 0; attempt < MAX_RESTARTS && accepts(entry, token); attempt += 1) {
      const held = await entry.store.readCursor();
      if (!accepts(entry, token)) return false;
      validatePosition(entry, held);
      const result = await recoverOnce(entry, token, held);
      if (!result?.resync) return result;
      if (!await reset(entry, token, result.resync, result.capture, result.cursor)) return false;
    }
    scheduleRetry(entry, token);
    return false;
  }

  async function recoverOnce(entry, token, held) {
    if (held?.cursor && !needsSnapshot(entry, held)) return changes(entry, token, held.cursor);
    const capture = await entry.store.capture();
    const cursor = cursorOf(held);
    const result = await snapshot(entry, token, capture, cursor);
    if (!result) return false;
    return result.resync ? { ...result, capture, cursor } : changes(entry, token, result.cursor);
  }

  function scheduleRetry(entry, token) {
    if (!accepts(entry, token) || entry.retry) return;
    const delay = RETRY_DELAYS_MS[Math.min(entry.attempts++, RETRY_DELAYS_MS.length - 1)];
    entry.retry = setTimeout(() => {
      entry.retry = null;
      if (accepts(entry, token)) void catchUp(entry.scope);
    }, delay);
    entry.retry.unref?.();
  }

  function failed(entry, token, error) {
    if (!accepts(entry, token)) return false;
    entry.failures = reconciling(error) ? 0 : entry.failures + 1;
    if (entry.failures === 3) onError(error);
    scheduleRetry(entry, token);
    return false;
  }

  async function perform(entry, token, operation) {
    try {
      const result = await operation(token);
      if (result === true && accepts(entry, token)) {
        clearTimeout(entry.retry); entry.retry = null; entry.attempts = 0; entry.failures = 0;
      }
      return result;
    } catch (error) { return failed(entry, token, error); }
  }

  function run(entry, operation) {
    if (!stands(entry)) return Promise.resolve(false);
    const token = takeOpening(entry);
    if (!token) return Promise.resolve(false);
    if (entry.running?.token === token) return entry.running.promise;
    clearTimeout(entry.retry); entry.retry = null;
    const running = { token, promise: Promise.resolve().then(() => perform(entry, token, operation)) };
    entry.running = running;
    void running.promise.finally(() => { if (entry.running === running) entry.running = null; });
    return running.promise;
  }
  const catchUp = (scope) => {
    const entry = scopes.get(identity(scope));
    return entry ? run(entry, (token) => recover(entry, token)) : Promise.resolve(false);
  };

  async function check(entry, token) {
    if (entry.fresh || entry.validateCursor) return recover(entry, token);
    const held = await entry.store.readCursor();
    const capture = await entry.store.capture();
    const answer = await read(entry, token, "sync.head", cursorParams(held));
    if (!answer) return false;
    if (answer.status === "resync_required") {
      if (!await reset(entry, token, answer, capture, cursorOf(held))) return false;
      return recover(entry, token);
    }
    if (answer.status !== "ok") return false;
    return caughtUp(held, answer) && !needsSnapshot(entry, held) ? true : recover(entry, token);
  }
  const checkHeads = () => Promise.all([...scopes.values()].map((entry) => run(entry, (token) => check(entry, token))));

  function wakeOne(entry) {
    const token = takeOpening(entry);
    if (!token || entry.running?.token !== token) return catchUp(entry.scope);
    if (entry.trailing?.token !== token) {
      const trailing = { token, promise: entry.running.promise.then(() => {
        if (entry.trailing === trailing) entry.trailing = null;
        return accepts(entry, token) ? catchUp(entry.scope) : false;
      }) };
      entry.trailing = trailing;
    }
    return entry.trailing.promise;
  }
  const wake = (predicate = () => true) => Promise.all([...scopes.values()]
    .filter((entry) => predicate(entry.scope)).map(wakeOne));
  return {
    track, catchUp, checkHeads, wake, foreground: () => wake(),
    untrack: (scope) => { clearTimeout(scopes.get(identity(scope))?.retry); scopes.delete(identity(scope)); },
    dispose() {
      disposed = true; clearInterval(timer);
      for (const entry of scopes.values()) clearTimeout(entry.retry);
      scopes.clear();
    },
  };
}

const sameSnapshot = (first, answer) => first.incarnation === answer.incarnation
  && first.watermark === answer.watermark && first.cursor === answer.cursor
  && first.bridge_epoch === answer.bridge_epoch;
const snapshotExceeded = (records, answer, page, pages) => records.length > MAX_SNAPSHOT_RECORDS
  || Boolean(page && answer.next_page === page) || Boolean(answer.next_page && pages >= MAX_SNAPSHOT_PAGES);

function validatePosition(entry, held) {
  if (!entry.validateCursor) return;
  // Reconnect continuity needs this engine's committed position. Within an
  // unchanged opening, competing writers can still retry from shared CAS.
  if (cursorOf(held) !== entry.lastCursor) entry.fresh = true;
  entry.validateCursor = false;
}

function epochOpenings(epoch) {
  let token;
  return () => {
    const captured = epoch();
    if (!token || token.epoch !== captured) token = { epoch: captured, current: () => epoch() === captured };
    return token;
  };
}
