import { createWriteOperationCache } from "./writeOperationCache.js";
import { createWriteOperationWorker } from "./writeOperationWorker.js";
import {
  copyIntent, needsWriteStatus, terminalWrite, writeDependencies, writeRefusal,
  wireOperationId, wireRevision, writeSummaryFrame, operationFailure, WRITE_STATUS_LIMIT,
} from "./writeOperationIntent.js";

/** Per authenticated device, durable intent owns retries; sessions own transport.
 * bind() is called only for an authoritative current-opening greeting. */
export function createWriteOperationService(options) {
  const cache = options.cache || createWriteOperationCache(options);
  const randomUUID = options.randomUUID || (() => crypto.randomUUID());
  const tabId = options.tabId || randomUUID();
  const now = options.now || Date.now;
  const pending = new Map();
  const listeners = new Set();
  const sending = new Map();
  const checks = new Set();
  const refreshes = new Map();
  let snapshot = [];
  let binding = null;
  let serial = Promise.resolve();
  let stopped = false;
  let detach = () => {};
  const worker = createWriteOperationWorker({ cache, tabId, run: statusPass, hasWork: () => Boolean(binding) && (snapshot.some(needsWriteStatus) || checks.size > 0 || refreshes.size > 0),
    random: options.random || Math.random, setTimeout: options.setTimeout || globalThis.setTimeout, clearTimeout: options.clearTimeout || globalThis.clearTimeout });
  cache.activateProjection?.();
  const unsubscribe = cache.subscribe(() => {void refreshSnapshot().catch(() => {});});
  const ready = refreshSnapshot();
  ready.catch(() => {});

  async function refreshSnapshot() {
    snapshot = await cache.list();
    for (const listener of [...listeners]) listener(snapshot);
    settleKnown();
    worker.wake();
    return snapshot;
  }
  function settleKnown() {
    for (const row of snapshot) {
      const waiting = pending.get(row.operationId);
      if (!waiting || !row.acknowledged) continue;
      pending.delete(row.operationId);
      if (row.state === "failed") waiting.reject(operationFailure(row.confirmation));
      else waiting.resolve(row.ack?.result ?? row.confirmation?.result);
    }
  }
  function current(held) {
    if (stopped || held !== binding) return false;
    if (held?.active && !held.active()) return false;
    const authority = held?.session.bridgeEpoch?.current();
    if (!authority) return true;
    return authority.bridge_epoch === held.greeting.bridge_epoch && authority.opening === held.opening;
  }
  function supports(method) {
    if (!binding || !current(binding)) return false;
    const { greeting } = binding;
    return greeting.capabilities?.includes("bridge.epoch") && greeting.ack_confirm_methods?.includes(method) === true;
  }
  function raw(held, method, params, extra = {}) {
    const call = held.session.rawCall || held.session.callRaw || held.session.call;
    return call.call(held.session, method, params, { ...extra, assertCurrent: () => {
      if (!current(held)) throw writeRefusal("The write opening ended", "scope_ended");
      extra.assertCurrent?.();
    } });
  }
  async function submit(input) {
    await ready;
    const request = copyIntent(input.request || { method: input.method, params: input.params || {} });
    if (!supports(request.method)) throw writeRefusal("This bridge does not support durable acceptance for this action");
    const operationId = input.operationId || randomUUID();
    const old = await cache.get(operationId);
    if (old) return waitLocally(existingSubmit(old, request), input);
    const after = writeDependencies({ ...input, params: request.params }, await cache.list());
    if (after?.includes(operationId)) throw writeRefusal("A write cannot depend on itself", "invalid_dependency");
    const held = binding;
    const row = copyIntent({ accountId: options.accountId, deviceId: options.deviceId, principalId: options.principalId,
      operationId, request, method: request.method, after, owner: input.owner, generation: input.generation,
      draft: input.draft, baseRevision: input.baseRevision, overlay: input.overlay || [], reservedIds: input.reservedIds || [],
      acceptedEpoch: null, journalIncarnation: held.greeting.journal_incarnation, revision: 0, state: "queued", acknowledged: false, overlayActive: true, createdAt: now() });
    await installIntentEffects(row);
    await cache.put(row);
    await refreshSnapshot();
    const result = waitFor(operationId);
    worker.dispatch(() => {void send(row, held);});
    return waitLocally(result, input);
  }
  function waitLocally(result, input) {
    const signal = input.signal || input.options?.signal;
    if (!signal) return result;
    let abort;
    const cancelled = new Promise((_, reject) => {
      abort = () => reject(Object.assign(new Error("Local waiting ended; the write is retained"), {
        name: "AbortError", error_code: "local_wait_ended", uncertain: true,
      }));
      if (signal.aborted) abort();
      else signal.addEventListener("abort", abort, { once: true });
    });
    return Promise.race([result, cancelled]).finally(() => signal.removeEventListener("abort", abort));
  }
  async function installIntentEffects(row) {
    const effects = await options.normalizeIntentEffects?.(row);
    if (effects) Object.assign(row, effects);
  }
  async function existingSubmit(row, request) {
    if (JSON.stringify(row.request) !== JSON.stringify(request)) throw writeRefusal("This operation ID already belongs to another intent", "operation_conflict");
    if (row.state === "failed") throw operationFailure(row.confirmation);
    if (row.acknowledged) return row.ack?.result ?? row.confirmation?.result;
    const result = waitFor(row.operationId);
    void worker.reconcile();
    return result;
  }
  function waitFor(id) {
    const previous = pending.get(id);
    if (previous) return previous.promise;
    let resolve, reject;
    const promise = new Promise((yes, no) => {resolve = yes; reject = no;});
    pending.set(id, { resolve, reject, promise });
    settleKnown();
    return promise;
  }
  async function send(row, held) {
    if (!current(held) || !supports(row.request.method) || sending.get(row.operationId) === held) return;
    sending.set(row.operationId, held);
    try {
      const frame = await raw(held, row.request.method, row.request.params, { write: { operation_id: row.operationId, after: row.after } });
      if (current(held)) await handleFrame(frame, held);
    } catch (error) {
      if (current(held) && error.retryable === false && !error.uncertain) await refused(row, error);
    } finally {
      if (sending.get(row.operationId) === held) sending.delete(row.operationId);
      worker.wake();
    }
  }
  async function refused(row, error) {
    const confirmation = { type: "writes.confirmed", operation_id: row.operationId, state: "failed", effect: "unapplied",
      error: { code: error.error_code, message: error.message, retryable: error.retryable }, revision: row.revision + 1 };
    await cache.confirm(row.operationId, { confirmation, revision: confirmation.revision });
    await refreshSnapshot();
  }
  function handleFrame(frame, held = binding) {
    const task = serial.then(() => applyFrame(frame, held));
    serial = task.catch(() => {});
    return task;
  }
  async function applyFrame(frame, held) {
    if (!current(held) || frame?.bridge_epoch !== held.greeting.bridge_epoch) return false;
    if (!validPhase(frame)) return false;
    const id = wireOperationId(frame);
    const row = await cache.get(id);
    if (!eligibleFrame(row, frame, held)) return false;
    if (frame.write?.phase === "ack") await accept(frame, row, held);
    else if (frame.type === "writes.confirmed") await confirm(frame, row, held);
    else return false;
    worker.reset();
    await refreshSnapshot();
    return true;
  }
  function validPhase(frame) {
    if (frame.write?.phase === "ack") return frame.ok === true && frame.write.state === "accepted";
    if (frame.type !== "writes.confirmed") return false;
    if (frame.state === "succeeded") return true;
    return frame.state === "failed" && ["unapplied", "partial", "unknown"].includes(frame.effect);
  }
  function eligibleFrame(row, frame, held) {
    if (!row || !current(held) || terminalWrite(row)) return false;
    if (row.journalIncarnation !== held.greeting.journal_incarnation) return false;
    const revision = wireRevision(frame);
    return Number.isSafeInteger(revision) && (revision > row.revision || (row.unresolvedReason && revision === row.revision));
  }
  async function normalizedEffects(normalize, frame, row) {
    try { return await normalize?.(frame, row) || {}; }
    catch { return {}; }
  }
  async function accept(frame, row, held) {
    const effects = await normalizedEffects(options.normalizeAckEffects, frame, row);
    if (!current(held)) return;
    await cache.accept(row.operationId, { ...effects, ack: frame, owners: frame.write.owners, revision: wireRevision(frame), acceptedEpoch: frame.write.accepted_epoch,
      journalIncarnation: held.greeting.journal_incarnation, accept: () => current(held) });
  }
  async function confirm(frame, row, held) {
    const normalize = confirmationNormalizer(frame, row);
    const effects = await normalizedEffects(normalize, frame, row);
    if (!current(held)) return;
    const stored = await cache.confirm(row.operationId, { ...effects, confirmation: frame, revision: frame.revision,
      owners: row.owners || frame.published, published: frame.published || [], accept: () => current(held) });
    if (options.onRefresh && frame.refresh?.length) {
      const entry = { frame, row: stored, held, running: false };
      refreshes.set(row.operationId, entry);
      startRefresh(entry);
    }
  }
  function confirmationNormalizer(frame, row) {
    if (options.normalizeConfirmation) return options.normalizeConfirmation;
    if (!row.acknowledged && frame.state === "succeeded") return options.normalizeAckEffects;
    return null;
  }
  function startRefresh(entry) {
    if (entry.running || !current(entry.held)) return;
    entry.running = true;
    const { frame, row, held } = entry;
    Promise.resolve().then(() => options.onRefresh(frame.refresh, frame, row, {
      active: () => current(held), call: (method, params, extra) => raw(held, method, params, extra),
      session: held.session, greeting: held.greeting,
    })).then(() => {
      if (refreshes.get(row.operationId) === entry) refreshes.delete(row.operationId);
    }).catch(() => {}).finally(() => {entry.running = false; worker.wake();});
  }
  async function quarantine(row, reason) {
    await cache.update(row.operationId, value => ({ ...value, state: "outcome_unknown", unresolved: true, unresolvedReason: reason }));
  }
  async function processSummary(summary, held, retries) {
    const id = wireOperationId(summary);
    const row = await cache.get(id);
    if (!row || !current(held)) return;
    if (summary.state === "not_found") {
      if (row.acknowledged) await quarantine(row, "acknowledged_missing");
      else retries.add(id);
      return;
    }
    if (summary.state === "result_expired") {
      await cache.update(id, value => ({ ...value, state: "result_expired", acknowledged: true, unresolved: true }));
      return;
    }
    await handleFrame(writeSummaryFrame(summary, held.greeting.bridge_epoch), held);
  }
  async function readPages(method, ids, held, retries) {
    let cursor;
    const seen = new Set();
    do {
      const result = await raw(held, method, { operation_ids: ids, ...(cursor ? { cursor } : {}) });
      if (!current(held)) return;
      if (!await verifyJournal(result, ids, held)) return;
      for (const summary of result.operations || []) await processSummary(summary, held, retries);
      cursor = nextCursor(result.next_cursor, seen);
      if (method === "writes.subscribe") rememberCursor(held, result.cursor);
    } while (cursor && current(held));
  }
  function nextCursor(cursor, seen) {
    if (!cursor) return null;
    if (seen.has(cursor)) throw new Error("Write status repeated its pagination cursor");
    seen.add(cursor);
    return cursor;
  }
  function rememberCursor(held, cursor) {
    if (typeof cursor !== "string" || !/^\d+$/.test(cursor)) return;
    if (held.cursor === null || BigInt(cursor) > BigInt(held.cursor)) held.cursor = cursor;
  }
  async function verifyJournal(result, ids, held) {
    if (!result.journal_incarnation || result.journal_incarnation === held.greeting.journal_incarnation) return true;
    for (const id of ids) await quarantine(await cache.get(id), "journal_changed");
    return false;
  }
  async function statusBatch(rows, held) {
    const ids = rows.map(row => row.operationId);
    const retries = new Set();
    await readPages("writes.status", ids, held, retries);
    await readPages("writes.subscribe", ids, held, retries);
    for (const id of retries) {
      const row = await cache.get(id);
      if (row && !row.acknowledged && !terminalWrite(row) && !row.unresolved) void send(row, held);
    }
  }
  function restoreRefreshes(rows, held) {
    if (!options.onRefresh) return;
    for (const row of rows) {
      if (row.journalIncarnation !== held.greeting.journal_incarnation) continue;
      if (!row.overlayActive || !row.confirmation?.refresh?.length || refreshes.has(row.operationId)) continue;
      refreshes.set(row.operationId, { frame: row.confirmation, row, held, running: false });
    }
  }
  async function statusPass() {
    await ready;
    const held = binding;
    if (!held) return;
    const all = await cache.list();
    restoreRefreshes(all, held);
    for (const entry of refreshes.values()) startRefresh(entry);
    const rows = all.filter(row => needsWriteStatus(row) || checks.has(row.operationId));
    checks.clear();
    const compatible = [];
    for (const row of rows) {
      if (!held.greeting.capabilities?.includes("bridge.epoch")) await quarantine(row, "bridge_incapable");
      else if (row.journalIncarnation !== held.greeting.journal_incarnation) await quarantine(row, "journal_changed");
      else compatible.push(row);
    }
    for (let at = 0; at < compatible.length && current(held); at += WRITE_STATUS_LIMIT) await statusBatch(compatible.slice(at, at + WRITE_STATUS_LIMIT), held);
    await refreshSnapshot();
  }
  function rebindRefreshes(held) {
    for (const [id, entry] of refreshes) {
      if (entry.row.journalIncarnation !== held.greeting.journal_incarnation) refreshes.delete(id);
      else refreshes.set(id, { ...entry, held, running: false });
    }
  }
  async function bind({ session, greeting, opening, active }) {
    detach();
    binding = { session, greeting, active, opening: opening ?? session.bridgeEpoch?.current()?.opening, cursor: null };
    const held = binding;
    rebindRefreshes(held);
    detach = session.onWriteFrame?.(frame => {void handleFrame(frame, held).catch(() => {});}) || (() => {});
    await worker.restart();
    await worker.reconcile();
  }
  return {
    submit, bind, handleFrame, supports,
    reconcile: () => worker.reconcile(),
    checkAgain(id) {if (id) checks.add(id); return worker.reconcile();},
    observePublication: input => cache.observePublication(input),
    get: id => cache.get(id), list: () => cache.list(), readOperations: () => snapshot,
    subscribe(listener) {listeners.add(listener); return () => listeners.delete(listener);},
    async dismiss(id) {await cache.update(id, row => ({ ...row, dismissed: true })); await refreshSnapshot();},
    async close() {
      stopped = true; binding = null; detach(); unsubscribe(); listeners.clear();
      cache.deactivateProjection?.(); await worker.close();
      for (const waiting of pending.values()) waiting.reject(writeRefusal("Local waiting ended; the write is retained", "local_wait_ended"));
      pending.clear();
    },
  };
}

const services = new Map();
export function getWriteOperationService(options) {
  const key = JSON.stringify([options.accountId, options.deviceId, options.principalId]);
  if (!services.has(key)) services.set(key, createWriteOperationService(options));
  return services.get(key);
}
export async function releaseWriteOperationServices() {
  const held = [...services.values()]; services.clear();
  await Promise.all(held.map(service => service.close()));
}
