// Scope-independent durable recovery. Tokens stay opaque; only server heads
// are compared, and publication owns the cache/cursor transaction boundary.
export const SYNC_HEAD_INTERVAL_MS = 30_000;
const MAX_RESTARTS = 3;
const MAX_SNAPSHOT_RECORDS = 100_000;
const MAX_SNAPSHOT_PAGES = 1000;
const identity = (scope) => JSON.stringify(scope);
const pageParams = (page) => page ? { page } : {};
const cursorParams = (held) => held?.cursor ? { cursor: held.cursor } : {};
const caughtUp = (held, answer) => Boolean(held?.cursor && !held.snapshotRequired
  && held.incarnation === answer.incarnation && held.appliedHead === answer.head);

export function createScopedSyncEngine({ request, storeFor, active = () => true, epoch = () => null }) {
  const scopes = new Map();
  let disposed = false;
  const current = () => !disposed && active();
  const timer = setInterval(() => { void checkHeads(); }, SYNC_HEAD_INTERVAL_MS);
  timer.unref?.();

  function track(scope) {
    const key = identity(scope);
    if (!scopes.has(key)) scopes.set(key, { scope, store: storeFor(scope), running: null, trailing: null });
  }
  const stands = (entry) => current() && scopes.get(identity(entry.scope)) === entry;

  async function read(entry, method, params) {
    if (!stands(entry)) return null;
    const capturedEpoch = epoch();
    const answer = await request(method, { scope: entry.scope, ...params });
    if (!stands(entry) || epoch() !== capturedEpoch) return null;
    if (capturedEpoch && answer?.bridge_epoch !== capturedEpoch) return null;
    return answer;
  }

  async function reset(entry, answer, capture, expectedCursor) {
    if (!stands(entry)) return false;
    if (answer.reason === "scope_removed") {
      await entry.store.publish({ response: answer, records: [], capture, expectedCursor, removed: true });
      return false;
    }
    return entry.store.reset({ capture, expectedCursor });
  }

  async function snapshot(entry, capture, expectedCursor) {
    let page, first, pages = 0;
    const records = [];
    do {
      const answer = await read(entry, "sync.snapshot", pageParams(page));
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
    if (!stands(entry)) return null;
    const committed = await entry.store.publish({ response: first, records, capture, expectedCursor, snapshot: true });
    return committed ? { cursor: first.cursor } : null;
  }

  async function changes(entry, cursor) {
    let continuation;
    do {
      const capture = await entry.store.capture();
      const answer = await read(entry, "sync.changes", { cursor, ...(continuation ? { continuation } : {}) });
      if (!answer) return false;
      if (answer.status === "resync_required") return { resync: answer, capture, cursor };
      if (answer.status !== "ok" || !stands(entry)) return false;
      if (!await entry.store.publish({ response: answer, records: answer.changes, capture, expectedCursor: cursor })) return false;
      if (answer.continuation && answer.cursor === cursor) return false;
      cursor = answer.cursor;
      continuation = answer.continuation;
    } while (continuation);
    return true;
  }

  async function recover(entry) {
    for (let attempt = 0; attempt < MAX_RESTARTS && stands(entry); attempt += 1) {
      const held = await entry.store.readCursor();
      const result = await recoverOnce(entry, held);
      if (!result?.resync) return result;
      if (!await reset(entry, result.resync, result.capture, result.cursor)) return false;
    }
    return false;
  }

  async function recoverOnce(entry, held) {
    if (held?.cursor && !held.snapshotRequired) return changes(entry, held.cursor);
    const capture = await entry.store.capture();
    const cursor = held?.cursor ?? null;
    const result = await snapshot(entry, capture, cursor);
    if (!result) return false;
    return result.resync ? { ...result, capture, cursor } : changes(entry, result.cursor);
  }

  function run(entry, operation) {
    if (!stands(entry)) return Promise.resolve(false);
    if (entry.running) return entry.running;
    const running = Promise.resolve().then(operation).catch(() => false);
    entry.running = running;
    void running.finally(() => { if (entry.running === running) entry.running = null; });
    return running;
  }
  const catchUp = (scope) => {
    const entry = scopes.get(identity(scope));
    return entry ? run(entry, () => recover(entry)) : Promise.resolve(false);
  };

  async function check(entry) {
    const held = await entry.store.readCursor();
    const capture = await entry.store.capture();
    const answer = await read(entry, "sync.head", cursorParams(held));
    if (!answer) return false;
    if (answer.status === "resync_required") {
      if (!await reset(entry, answer, capture, held?.cursor ?? null)) return false;
      return recover(entry);
    }
    if (answer.status !== "ok") return false;
    return caughtUp(held, answer) ? true : recover(entry);
  }
  const checkHeads = () => Promise.all([...scopes.values()].map((entry) => run(entry, () => check(entry))));

  function wakeOne(entry) {
    if (!entry.running) return catchUp(entry.scope);
    entry.trailing ||= entry.running.then(() => {
      entry.trailing = null;
      return catchUp(entry.scope);
    });
    return entry.trailing;
  }
  const wake = (predicate = () => true) => Promise.all([...scopes.values()]
    .filter((entry) => predicate(entry.scope)).map(wakeOne));
  return {
    track, catchUp, checkHeads, wake, foreground: () => wake(),
    untrack: (scope) => scopes.delete(identity(scope)),
    dispose() { disposed = true; clearInterval(timer); scopes.clear(); },
  };
}

const sameSnapshot = (first, answer) => first.incarnation === answer.incarnation
  && first.watermark === answer.watermark && first.cursor === answer.cursor
  && first.bridge_epoch === answer.bridge_epoch;
const snapshotExceeded = (records, answer, page, pages) => records.length > MAX_SNAPSHOT_RECORDS
  || Boolean(page && answer.next_page === page) || Boolean(answer.next_page && pages >= MAX_SNAPSHOT_PAGES);
