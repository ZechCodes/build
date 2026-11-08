import { createWriteOperationCache } from "./writeOperationCache.js";
import { createWriteOperationWorker } from "./writeOperationWorker.js";
import { creationNeverSent, creationSendEvidenceMatches, creationTransportIntent, creationWritesSupported,
  isCreationWrite, nextCreationEvidence, normalizeWriteProgress, unsupportedCreationOutcome } from "./writeOperationProgress.js";
import {
  copyIntent, needsWriteStatus, terminalWrite, writeDependencies, writeRefusal, provenExpiredFailure, expiredOperationFailure,
  wireOperationId, wireRevision, writeSummaryFrame, writeSubmissionOrder, knownWriteSummary, operationFailure, WRITE_STATUS_LIMIT,
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
  let creationSendSequence = 0;
  let stopped = false;
  let detach = () => {};
  const worker = createWriteOperationWorker({ cache, tabId, run: statusPass, hasWork: runnableWork,
    random: options.random || Math.random, setTimeout: options.setTimeout || globalThis.setTimeout, clearTimeout: options.clearTimeout || globalThis.clearTimeout });
  cache.activateProjection?.();
  const unsubscribe = cache.subscribe(() => {void refreshSnapshot().catch(() => {});});
  const ready = refreshSnapshot();
  ready.catch(() => {});

  async function refreshSnapshot() {
    snapshot = await cache.list();
    for (const listener of [...listeners]) listener(snapshot);
    settleKnown();
    pruneRefreshes();
    if (!runnableWork()) worker.reset();
    worker.wake();
    return snapshot;
  }
  function settleKnown() {
    for (const row of snapshot) {
      const waiting = pending.get(row.operationId);
      if (!waiting) continue;
      if (row.unresolved) {
        pending.delete(row.operationId);
        waiting.reject(unresolvedError(row));
        continue;
      }
      if (!row.acknowledged) continue;
      pending.delete(row.operationId);
      if (row.state === "failed") waiting.reject(operationFailure(row.confirmation));
      else if (provenExpiredFailure(row)) waiting.reject(expiredOperationFailure(row));
      else waiting.resolve(row.ack?.result ?? row.confirmation?.result);
    }
  }
  function current(held) {
    if (stopped || !held || held !== binding) return false;
    if (held.active && !held.active()) return false;
    return openingMatches(held);
  }
  function openingMatches(held) {
    const authority = held.session.bridgeEpoch?.current();
    if (!authority) return true;
    return authority.bridge_epoch === held.greeting.bridge_epoch && authority.opening === held.opening;
  }
  function runnableWork() {
    return Boolean(binding) && current(binding)
      && (snapshot.some(needsWriteStatus) || snapshot.some(refreshRunnable) || checks.size > 0);
  }
  function entryCurrent(entry) {
    return refreshes.get(entry.row.operationId) === entry && current(entry.held);
  }
  function detachRefresh(entry) {
    entry.stopWatch?.(); entry.stopWatch = null; entry.watchGeneration = null; entry.recheck = false;
  }
  function pruneRefreshes() {
    for (const [id, entry] of refreshes) {
      const row = snapshot.find((value) => value.operationId === id);
      if (refreshRequired(row) && row.revision === entry.frame.revision) continue;
      detachRefresh(entry); refreshes.delete(id);
    }
  }
  function supports(method) {
    if (!binding || !current(binding)) return false;
    const { greeting } = binding;
    return greeting.capabilities?.includes("bridge.epoch") && greeting.ack_confirm_methods?.includes(method) === true
      && creationWritesSupported(greeting, method);
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
    if (old) return waitLocally(existingSubmit(old, request, input), input);
    const after = writeDependencies({ ...input, params: request.params }, await cache.list());
    if (after?.includes(operationId)) throw writeRefusal("A write cannot depend on itself", "invalid_dependency");
    const held = binding;
    const row = copyIntent({ accountId: options.accountId, deviceId: options.deviceId, principalId: options.principalId,
      operationId, request, method: request.method, after, owner: input.owner, generation: input.generation,
      draft: input.draft, baseRevision: input.baseRevision, overlay: input.overlay || [], reservedIds: input.reservedIds || [],
      acceptedEpoch: null, journalIncarnation: held.greeting.journal_incarnation, revision: 0, state: "queued", acknowledged: false, overlayActive: true, createdAt: now(),
      ...creationTransportIntent(request.method) });
    await installIntentEffects(row);
    await cache.put(row);
    await refreshSnapshot();
    const result = waitFor(operationId);
    worker.dispatch(leaseActive => send(row, held, leaseActive));
    return waitLocally(result, input);
  }
  /** Persist a complete ordered batch without waiting for wire acceptance.
   * Review comments may be queued before a greeting: their first authoritative
   * opening supplies the journal once, before any transport attempt. */
  async function enqueueBatch(inputs) {
    await ready;
    const rows = await cache.list();
    const batch = [];
    for (const input of inputs) {
      const row = await queuedBatchRow(input, [...rows, ...batch]);
      batch.push(row);
    }
    const fresh = batch.filter((row) => !rows.some((held) => held.operationId === row.operationId));
    if (fresh.length) await cache.putMany(fresh);
    await refreshSnapshot();
    if (binding && current(binding)) void worker.reconcile();
    return batch.map((row) => row.operationId);
  }

  async function queuedBatchRow(input, rows) {
    const request = copyIntent(input.request || { method: input.method, params: input.params || {} });
    const operationId = input.operationId || randomUUID();
    const previous = rows.find((row) => row.operationId === operationId);
    if (previous) {
      verifyRetryAfter(previous, input);
      if (JSON.stringify(previous.request) !== JSON.stringify(request)) throw writeRefusal("This operation ID already belongs to another intent", "operation_conflict");
      return previous;
    }
    if (!canQueue(request.method)) throw writeRefusal("This bridge does not support durable acceptance for this action");
    const after = batchDependencies(input, request, operationId, rows);
    const row = copyIntent({ accountId: options.accountId, deviceId: options.deviceId, principalId: options.principalId,
      operationId, request, method: request.method, after, owner: input.owner, draft: input.draft,
      overlay: input.overlay || [], reservedIds: input.reservedIds || [], acceptedEpoch: null,
      journalIncarnation: null, awaitPredecessorAck: true, revision: 0, state: "queued", acknowledged: false, overlayActive: true, createdAt: now() });
    await installIntentEffects(row);
    return row;
  }

  function batchDependencies(input, request, operationId, rows) {
    const after = writeDependencies({ ...input, params: request.params }, rows);
    if (after?.includes(operationId)) throw writeRefusal("A write cannot depend on itself", "invalid_dependency");
    return after;
  }

  function canQueue(method) {
    if (stopped) return false;
    if (binding && current(binding)) return supports(method);
    return method === "tasks.comment";
  }

  async function adoptQueuedJournal(row, held) {
    if (row.journalIncarnation !== null || row.acknowledged || row.state !== "queued") return row;
    if (!supports(row.request.method)) {
      await cache.update(row.operationId, (value) => ({ ...value, state: "failed", effect: "unapplied", overlayActive: false,
        error: { code: "write_unsupported", message: "This bridge cannot send the pending review comments. Update it, then try again." } }), { accept: () => current(held) });
      return null;
    }
    return cache.update(row.operationId, (value) => value?.journalIncarnation === null
      ? { ...value, journalIncarnation: held.greeting.journal_incarnation } : null, { accept: () => current(held) });
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
  async function existingSubmit(row, request, input) {
    verifyRetryAfter(row, input);
    if (row.unresolved) throw unresolvedError(row);
    if (JSON.stringify(row.request) !== JSON.stringify(request)) throw writeRefusal("This operation ID already belongs to another intent", "operation_conflict");
    if (row.state === "failed") throw operationFailure(row.confirmation);
    if (provenExpiredFailure(row)) throw expiredOperationFailure(row);
    if (row.acknowledged) return row.ack?.result ?? row.confirmation?.result;
    const result = waitFor(row.operationId);
    void worker.reconcile();
    return result;
  }
  function verifyRetryAfter(row, input) {
    if (input.after === undefined) return;
    const after = [...new Set(input.after)].sort();
    const saved = row.after === undefined ? undefined : [...new Set(row.after)].sort();
    if (JSON.stringify(after) !== JSON.stringify(saved)) throw writeRefusal("This operation ID has different dependencies", "operation_conflict");
  }
  function unresolvedError(row) {
    return Object.assign(new Error(row.error?.message || "The outcome of this change needs checking"), {
      error_code: row.state === "result_expired" ? "result_expired" : "outcome_unknown",
      uncertain: true, operation_id: row.operationId,
    });
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
  async function send(row, held, leaseActive = () => true) {
    if (!eligibleToSend(row, held)) return;
    sending.set(row.operationId, held);
    let sent = row;
    try {
      if (row.awaitPredecessorAck && !await prepareBatchSend(row, held)) return;
      if (isCreationWrite(row.request.method)) {
        sent = await prepareCreationTransport(row, held, leaseActive);
        if (!sent) return;
      }
      const frame = await raw(held, row.request.method, row.request.params, { write: { operation_id: row.operationId, after: row.after }, assertCurrent: () => assertLease(leaseActive) });
      if (current(held)) await handleFrame(frame, held);
    } catch (error) {
      await sendFailed(sent, error, held);
    } finally {
      if (sending.get(row.operationId) === held) sending.delete(row.operationId);
      worker.wake();
    }
  }
  async function prepareCreationTransport(row, held, leaseActive) {
    const creationSendToken = `${tabId}:${++creationSendSequence}`;
    const saved = await cache.update(row.operationId, value => {
      if (!value || terminalWrite(value) || value.acknowledged || value.unresolved) return null;
      if (value.journalIncarnation !== held.greeting.journal_incarnation) return null;
      return { ...value, transportAttempted: true, creationSendToken };
    }, { accept: () => current(held) && leaseActive() });
    const ready = saved?.creationSendToken === creationSendToken && !terminalWrite(saved) && !saved.acknowledged && !saved.unresolved && current(held) && leaseActive();
    return ready ? saved : null;
  }
  async function sendFailed(row, error, held) {
    if (stopped) return;
    if (current(held) && definitiveRefusal(error)) return refused(row, error, held);
    if (!isCreationWrite(row.request.method)) return;
    await cache.update(row.operationId, value => {
      if (!value || terminalWrite(value) || !creationSendEvidenceMatches(value, row)) return null;
      return { ...value, checking: true };
    }, { accept: () => !stopped });
    await refreshSnapshot();
  }
  function recordCreationEvidence(row, held) {
    if (!isCreationWrite(row.request.method)) return;
    return cache.update(row.operationId, value => {
      if (!value || terminalWrite(value) || value.journalIncarnation !== held.greeting.journal_incarnation) return null;
      return { ...value, creationEvidence: nextCreationEvidence(value), ...(value.checking ? { checking: false } : {}) };
    }, { accept: () => current(held) });
  }
  function eligibleToSend(row, held) {
    return current(held) && supports(row.request.method) && sending.get(row.operationId) !== held;
  }
  async function predecessorsAccepted(row) {
    for (const id of row.after || []) {
      const predecessor = await cache.get(id);
      if (!predecessor?.acknowledged || ["failed", "result_expired"].includes(predecessor.state) || predecessor.unresolved) return false;
    }
    return true;
  }
  async function prepareBatchSend(row, held) {
    if (!await predecessorsAccepted(row)) return false;
    const saved = await cache.update(row.operationId, (value) => {
      if (!value || terminalWrite(value) || value.unresolved || value.acknowledged) return null;
      return { ...value, transportAttempted: true };
    }, { accept: () => current(held) });
    return Boolean(saved?.transportAttempted && !terminalWrite(saved) && !saved.unresolved && !saved.acknowledged);
  }

  async function cancelUnsentBatch(ids) {
    for (const id of ids) await cache.update(id, (row) => {
      if (!row?.awaitPredecessorAck || row.transportAttempted || row.acknowledged) throw new Error("This write was already attempted; its result must be checked");
      return { ...row, state: "failed", effect: "unapplied", overlayActive: false, dismissed: true,
        error: { code: "batch_cancelled", message: "Removed from the pending review batch" } };
    });
    await refreshSnapshot();
  }

  function definitiveRefusal(error) {
    const local = ["scope_ended", "local_wait_ended"].includes(error.error_code);
    return !local && error.retryable === false && !error.uncertain;
  }
  async function refused(row, error, held) {
    const confirmation = { type: "writes.confirmed", operation_id: row.operationId, state: "failed", effect: "unapplied",
      error: { code: error.error_code, message: error.message, retryable: error.retryable }, revision: row.revision + 1 };
    await cache.confirm(row.operationId, { confirmation, revision: confirmation.revision, accept: () => current(held) });
    await refreshSnapshot();
  }
  function handleFrame(frame, held = binding) {
    const task = serial.then(() => routeFrame(frame, held));
    serial = task.catch(() => {});
    return task;
  }
  function routeFrame(frame, held) {
    if (frame?.type !== "writes.progress") return applyFrame(frame, held);
    if (!current(held) || frame.bridge_epoch !== held.greeting.bridge_epoch) return false;
    return recordProgress(frame, held);
  }
  async function applyFrame(frame, held) {
    if (!current(held) || frame?.bridge_epoch !== held.greeting.bridge_epoch) return false;
    if (!validPhase(frame)) return false;
    const id = wireOperationId(frame);
    const row = await cache.get(id);
    if (!eligibleFrame(row, frame, held)) return false;
    if (Object.hasOwn(frame, "result_ref")) {
      await quarantine(row, "result_reference", held, frame);
      worker.reset();
      await refreshSnapshot();
      return true;
    }
    if (frame.write?.phase === "ack") await accept(frame, row, held);
    else if (frame.type === "writes.confirmed") await confirm(frame, row, held);
    else return false;
    worker.reset();
    await refreshSnapshot();
    return true;
  }
  async function recordProgress(frame, held) {
    if (!current(held) || !held.greeting.capabilities?.includes("environments")) return false;
    const row = await cache.get(wireOperationId(frame));
    if (!row || terminalWrite(row) || row.journalIncarnation !== held.greeting.journal_incarnation) return false;
    if (!isCreationWrite(row.request.method) || !normalizeWriteProgress(frame.progress)) return false;
    await cache.recordProgress(row.operationId, frame.progress, {
      journalIncarnation: held.greeting.journal_incarnation, accept: () => current(held),
    });
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
    return advancesRevision(row, revision);
  }
  function advancesRevision(row, revision) {
    if (!Number.isSafeInteger(revision)) return false;
    if (row.unresolvedReason === "result_reference") return revision > row.revision;
    return revision > row.revision || (row.unresolvedReason && revision === row.revision);
  }
  async function normalizedEffects(normalize, frame, row) {
    try { return await normalize?.(frame, row) || {}; }
    catch { return {}; }
  }
  async function accept(frame, row, held) {
    await recordCreationEvidence(row, held);
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
      owners: row.owners || frame.published, published: frame.published || [],
      refreshPending: Boolean(frame.refresh?.length), accept: () => current(held) });
    if (!confirmedRefreshCurrent(stored, frame, held)) return;
    if (options.onRefresh) {
      if (refreshes.has(row.operationId)) detachRefresh(refreshes.get(row.operationId));
      const entry = { frame, row: stored, held, running: false };
      refreshes.set(row.operationId, entry);
      startRefresh(entry);
    }
  }
  const confirmedRefreshCurrent = (stored, frame, held) => current(held)
    && stored?.revision === frame.revision && stored.refreshPending;
  function confirmationNormalizer(frame, row) {
    if (options.normalizeConfirmation) return options.normalizeConfirmation;
    if (!row.acknowledged && frame.state === "succeeded") return options.normalizeAckEffects;
    return null;
  }
  function startRefresh(entry) {
    if (entry.running || !current(entry.held)) return;
    if (!refreshRunnable(entry.row)) { void watchBlockedRefresh(entry).catch(() => {}); return; }
    entry.running = true;
    const { frame, row, held } = entry;
    Promise.resolve().then(() => options.onRefresh(frame.refresh, frame, row, {
      active: () => current(held), call: (method, params, extra) => raw(held, method, params, extra),
      session: held.session, greeting: held.greeting,
    })).then((result) => finishRefresh(entry, result)).catch(() => {}).finally(() => {entry.running = false; worker.wake();});
  }
  async function finishRefresh(entry, result) {
    if (!entryCurrent(entry)) return;
    if (taskRefreshEntry(entry)) return finishTaskRefresh(entry, result);
    const { row, frame, held } = entry;
    const saved = await cache.update(row.operationId, value => {
      if (value?.revision !== frame.revision) return null;
      return { ...value, refreshPending: false };
    }, { accept: () => current(held) });
    if (!current(held) || saved?.refreshPending !== false) return;
    if (refreshes.get(row.operationId) === entry) refreshes.delete(row.operationId);
    await refreshSnapshot();
  }
  async function finishTaskRefresh(entry, result) {
    if (!entryCurrent(entry)) return;
    const saved = await cache.get(entry.row.operationId);
    if (!entryCurrent(entry) || saved?.revision !== entry.frame.revision || !taskResultMatches(result, saved)) return;
    entry.row = saved;
    if (saved.refreshPending === false && result?.complete === true) {
      detachRefresh(entry);
      if (refreshes.get(saved.operationId) === entry) refreshes.delete(saved.operationId);
      await refreshSnapshot();
      return;
    }
    // A resolved partial callback cannot complete or clear an obligation.
    await watchBlockedRefresh(entry);
    await refreshSnapshot();
  }
  function taskResultMatches(result, row) {
    if (!result?.refresh) return true;
    return result.refresh.revision === row.revision && result.refresh.generation === row.refreshState?.generation;
  }
  async function watchBlockedRefresh(entry) {
    const state = entry.row.refreshState;
    if (!entryCurrent(entry) || !state || refreshRunnable(entry.row)) return;
    if (entry.watchGeneration === state.generation || !cache.watchRefresh) return;
    detachRefresh(entry);
    entry.watchGeneration = state.generation;
    const stop = await cache.watchRefresh({ operationId: entry.row.operationId, generation: state.generation },
      () => { void reevaluateRefresh(entry).catch(() => {}); });
    if (!entryCurrent(entry) || entry.watchGeneration !== state.generation) stop();
    else entry.stopWatch = stop;
  }
  async function reevaluateRefresh(entry) {
    if (!entryCurrent(entry)) return;
    if (entry.checking) { entry.recheck = true; return; }
    entry.checking = true;
    try {
      await cache.reevaluateRefresh({ operationId: entry.row.operationId, accept: () => entryCurrent(entry) });
      if (!entryCurrent(entry)) return;
      const saved = await cache.get(entry.row.operationId);
      if (!entryCurrent(entry) || saved?.revision !== entry.frame.revision) return;
      entry.row = saved;
      await refreshSnapshot();
    } finally { entry.checking = false; scheduleRefreshRecheck(entry); }
  }
  function scheduleRefreshRecheck(entry) {
    if (!entry.recheck || !entryCurrent(entry)) return;
    entry.recheck = false;
    queueMicrotask(() => { void reevaluateRefresh(entry).catch(() => {}); });
  }
  function refreshRequired(row) {
    if (!row || row.refreshPending === false) return false;
    return Boolean(row.confirmation?.refresh?.length);
  }
  function taskRefreshEntry(entry) {
    return entry.frame.refresh?.some(({ kind }) => kind === "task")
      || entry.row.overlay?.some(({ address }) => address?.kind === "tracker-task");
  }
  function refreshRunnable(row) {
    if (!refreshRequired(row)) return false;
    if (!taskRefreshEntry({ frame: row.confirmation, row })) return true;
    const state = row.refreshState;
    if (!state || state.version !== 1 || state.revision !== row.revision || !Array.isArray(state.targets)) return true;
    return state.targets.some(({ phase }) => phase === "unread");
  }
  async function updateEvidence(row, held, fields) {
    const expectedRevision = row.revision;
    return cache.update(row.operationId, value => {
      if (!value || terminalWrite(value) || value.revision !== expectedRevision) return null;
      return { ...value, ...fields };
    }, { accept: () => current(held) });
  }
  function summaryRevision(summary, row) {
    const revision = wireRevision(summary);
    return Number.isSafeInteger(revision) ? Math.max(row.revision, revision) : row.revision;
  }
  async function quarantine(row, reason, held, evidence) {
    const fields = { state: "outcome_unknown", unresolved: true, unresolvedReason: reason };
    if (evidence) Object.assign(fields, { revision: summaryRevision(evidence, row), resultReference: evidence.result_ref });
    await updateEvidence(row, held, fields);
  }
  async function processSummary(summary, held, retries) {
    if (!knownWriteSummary(summary)) throw new Error("The bridge returned an unknown write state");
    const id = wireOperationId(summary);
    const row = await cache.get(id);
    if (!row || !current(held)) return;
    if (obsoleteSummary(summary, row, held)) return;
    await recordSummaryProgress(summary, row, held);
    if (summary.state === "not_found") {
      if (row.acknowledged) await quarantine(row, "acknowledged_missing", held);
      else retries.add(id);
      return;
    }
    if (Object.hasOwn(summary, "result_ref")) {
      await quarantine(row, "result_reference", held, summary);
      return;
    }
    if (summary.state === "result_expired") {
      await recordExpired(summary, row, held);
      return;
    }
    await handleFrame(writeSummaryFrame(summary, held.greeting.bridge_epoch), held);
    if (summary.state === "executing") await recordExecuting(summary, held);
  }
  async function recordSummaryProgress(summary, row, held) {
    if (!["accepted", "executing"].includes(summary.state)) return;
    await recordCreationEvidence(row, held);
    if (Object.hasOwn(summary, "progress")) return recordProgress(summary, held);
  }
  async function recordExpired(summary, row, held) {
    const unapplied = provenExpiredFailure(summary);
    await updateEvidence(row, held, {
      state: "result_expired", acknowledged: true, revision: summaryRevision(summary, row),
      outcome: summary.outcome, effect: summary.effect, expiredSummary: summary,
      overlayActive: unapplied ? false : row.overlayActive, unresolved: !unapplied,
      unresolvedReason: unapplied ? null : "result_expired",
    });
  }
  function obsoleteSummary(summary, row, held) {
    if (summary.state === "not_found" && !creationWritesSupported(held.greeting, row.request.method)) return true;
    const revision = wireRevision(summary);
    return Number.isSafeInteger(revision) && revision < row.revision;
  }
  async function recordExecuting(summary, held) {
    if (!Number.isSafeInteger(summary.revision)) return;
    await cache.update(summary.operation_id, value => {
      if (!value?.acknowledged || terminalWrite(value) || summary.revision <= value.revision) return null;
      return { ...value, state: "executing", revision: summary.revision };
    }, { accept: () => current(held) });
  }
  function assertLease(leaseActive) {
    if (!leaseActive()) throw writeRefusal("The write worker lease ended", "scope_ended");
  }
  async function readPages(method, ids, held, retries, leaseActive) {
    let cursor;
    let nextMethod = method;
    const seen = new Set();
    do {
      const result = await raw(held, nextMethod, { operation_ids: ids, ...(cursor ? { cursor } : {}) }, { assertCurrent: () => assertLease(leaseActive) });
      if (!current(held)) return;
      if (!await verifyJournal(result, ids, held)) return;
      for (const summary of result.operations || []) await processSummary(summary, held, retries);
      cursor = nextCursor(result.next_cursor, seen);
      if (nextMethod === "writes.subscribe") rememberCursor(held, result.cursor);
      nextMethod = "writes.status";
    } while (cursor && current(held) && leaseActive());
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
    if (result.journal_incarnation === held.greeting.journal_incarnation) return true;
    for (const id of ids) await quarantine(await cache.get(id), "journal_changed", held);
    return false;
  }
  async function statusBatch(rows, held, leaseActive, retries) {
    await readPages("writes.status", rows.map(row => row.operationId), held, retries, leaseActive);
  }
  async function replaceSubscription(held, leaseActive, retries) {
    if (!current(held) || !leaseActive()) return;
    const rows = (await cache.list()).filter(row => needsWriteStatus(row) && row.journalIncarnation === held.greeting.journal_incarnation);
    const ids = rows.sort(writeSubmissionOrder).slice(0, WRITE_STATUS_LIMIT).map(row => row.operationId);
    if (ids.length) await readPages("writes.subscribe", ids, held, retries, leaseActive);
  }
  async function dispatchRetries(retries, held) {
    for (const id of retries) {
      const row = await cache.get(id);
      if (row && !row.acknowledged && !terminalWrite(row) && !row.unresolved) {
        worker.dispatch(active => send(row, held, active));
      }
    }
  }
  async function restoreRefreshes(rows, held) {
    if (!options.onRefresh) return;
    for (const row of rows) {
      if (row.journalIncarnation !== held.greeting.journal_incarnation) continue;
      if (!refreshRequired(row)) continue;
      const stored = await restoredRefreshRow(row, held);
      if (!current(held) || !refreshRequired(stored)) continue;
      retainRefreshEntry(stored, held);
    }
  }
  async function restoredRefreshRow(row, held) {
    if (row.refreshPending === true) return row;
    return cache.update(row.operationId, value => refreshRequired(value) ? { ...value, refreshPending: true } : null,
      { accept: () => current(held) });
  }
  function retainRefreshEntry(row, held) {
    const previous = refreshes.get(row.operationId);
    if (previous?.frame.revision === row.revision && previous.held === held) { previous.row = row; return; }
    if (previous) detachRefresh(previous);
    refreshes.set(row.operationId, { frame: row.confirmation, row, held, running: false });
  }
  async function statusPass(leaseActive) {
    await ready;
    const held = binding;
    if (!held) return;
    const all = await cache.list();
    await restoreRefreshes(all, held);
    for (const entry of refreshes.values()) startRefresh(entry);
    const rows = all.filter(row => needsWriteStatus(row) || checks.has(row.operationId));
    checks.clear();
    const compatible = await compatibleRows(rows, held);
    const retries = new Set();
    for (let at = 0; at < compatible.length && current(held) && leaseActive(); at += WRITE_STATUS_LIMIT) await statusBatch(compatible.slice(at, at + WRITE_STATUS_LIMIT), held, leaseActive, retries);
    await replaceSubscription(held, leaseActive, retries);
    await dispatchRetries(retries, held);
    await refreshSnapshot();
  }
  async function compatibleRows(rows, held) {
    const compatible = [];
    for (const candidate of rows) {
      const row = candidate.journalIncarnation === null ? await adoptQueuedJournal(candidate, held) : candidate;
      if (!row) continue;
      if (!held.greeting.capabilities?.includes("bridge.epoch")) await quarantine(row, "bridge_incapable", held);
      else if (row.journalIncarnation !== held.greeting.journal_incarnation) await quarantine(row, "journal_changed", held);
      else if (!creationWritesSupported(held.greeting, row.request.method)) await retainCreationForGreeting(row, held, compatible);
      else compatible.push(row);
    }
    return compatible;
  }
  async function retainCreationForGreeting(row, held, compatible) {
    const recovered = await cache.update(row.operationId, value => {
      if (!value || terminalWrite(value) || value.journalIncarnation !== held.greeting.journal_incarnation) return null;
      return creationNeverSent(value) ? unsupportedCreationOutcome(value) : { ...value, checking: true };
    }, { accept: () => current(held) });
    if (recovered && !terminalWrite(recovered)) compatible.push(recovered);
  }

  function rebindRefreshes(held) {
    for (const [id, entry] of refreshes) {
      detachRefresh(entry);
      if (entry.row.journalIncarnation !== held.greeting.journal_incarnation) refreshes.delete(id);
      else refreshes.set(id, { ...entry, held, running: false, checking: false });
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
    submit, enqueueBatch, cancelUnsentBatch, bind, handleFrame, supports,
    reconcile: () => worker.reconcile(),
    async checkAgain(id) {
      if (id) {
        checks.add(id);
        const row = await cache.get(id);
        if (row?.confirmation?.refresh?.length) await checkRefreshAgain(row);
      }
      return worker.reconcile();
    },
    observePublication: input => cache.observePublication(input),
    get: id => cache.get(id), list: () => cache.list(), readOperations: () => snapshot,
    subscribe(listener) {listeners.add(listener); return () => listeners.delete(listener);},
    async dismiss(id) {await cache.update(id, row => ({ ...row, dismissed: true })); await refreshSnapshot();},
    async close() {
      stopped = true; binding = null; detach(); unsubscribe(); listeners.clear();
      for (const entry of refreshes.values()) detachRefresh(entry);
      refreshes.clear();
      cache.deactivateProjection?.(); await worker.close();
      for (const waiting of pending.values()) waiting.reject(writeRefusal("Local waiting ended; the write is retained", "local_wait_ended"));
      pending.clear();
    },
  };

  async function checkRefreshAgain(row) {
    if (!taskRefreshEntry({ frame: row.confirmation, row })) {
      await cache.update(row.operationId, value => ({ ...value, refreshPending: true }));
      return;
    }
    if (!cache.reevaluateRefresh) return;
    const held = binding;
    await cache.reevaluateRefresh({ operationId: row.operationId, accept: () => current(held) });
  }
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
