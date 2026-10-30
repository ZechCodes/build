// Durable operation intent and accepted overlays share the replica transaction
// boundary, but live in stores that replica resets and lifetime sweeps never clear.
import { abortForError, putOrAbort } from "./idbDatabase.js";
import { createAnnouncer, randomToken, recordKey, writeStamp } from "./idbRecords.js";
import { announceWriteOperationCache, recordWriteOf, subscribeCache, transactWriteOperations } from "./localCache.js";
import {
  activateOperationProjection, clearOperationProjections, deactivateOperationProjection, layerPublished,
  leaseKey, operationKey, operationNamespace, validPublicationFence,
} from "./writeOperationProjection.js";

import { publicationReadAddresses, publicationGuardMatches, publicationIsActive, preparePublication,
  taskPublicationEvidenceAddresses } from "./writeOperationPublication.js";
import { TRACKER_TASK_KIND, taskAddress } from "./trackerCache.js";
import { threadCacheAddress } from "./conversationCache.js";

const notifications = createAnnouncer("build-write-operations");
const terminalStates = new Set(["succeeded", "failed"]);
const isTerminal = (record) => terminalStates.has(record?.state);
const notificationAddress = (scope, operationId) => ({
  deviceId: scope.accountId, entityId: scope.deviceId, kind: scope.principalId, ...(operationId ? { sub: operationId } : {}),
});

function validateScope(scope) {
  if (![scope.accountId, scope.deviceId, scope.principalId].every((part) => typeof part === "string" && part.length)) {
    throw new TypeError("Write operations require account, device and principal identity");
  }
}

function validateRecord(record) {
  if (typeof record?.operationId !== "string" || !record.operationId) throw new TypeError("Missing operation identity");
  if (!Number.isSafeInteger(record.revision) || record.revision < 0) throw new TypeError("Invalid operation revision");
  validateFences(record.owners || []);
  validateFences(record.published || []);
  validateFences(record.confirmation?.published || []);
}

function validateFences(fences) {
  if (!Array.isArray(fences) || !fences.every(validPublicationFence)) throw new TypeError("Invalid generation-scoped publication fence");
}

function canAdvance(previous, next) {
  if (!previous) return true;
  if (next.revision < previous.revision) return false;
  return !(isTerminal(previous) && next.state !== previous.state);
}

async function strictTransaction(mode, run) {
  const outcome = await transactWriteOperations(mode, (stores) => {
    try { return run(stores); }
    catch (error) { abortForError(stores.operations, error); return null; }
  });
  if (!outcome.committed) throw outcome.error || new Error("Write-operation storage is unavailable");
  return outcome;
}

function readRecords(store, keys, done) {
  const held = new Map();
  if (!keys.length) return done(held);
  let remaining = keys.length;
  for (const key of keys) {
    const request = store.get(key);
    request.onsuccess = () => {
      held.set(key, request.result);
      if (--remaining === 0) done(held);
    };
  }
}

function writeCanonical(records, entries, accept, done) {
  readRecords(records, publicationReadAddresses(entries).map(recordKey), (held) => {
    try {
      if (!accept()) return done([], []);
      const written = [], outcomes = [];
      for (const entry of entries) {
        validateFences(entry.publication || []);
        if (!publicationGuardMatches(entry, held)) {
          outcomes.push({ address: entry.address, payloadWritten: false, receiptPublished: false, reason: "superseded" });
          continue;
        }
        const active = () => publicationIsActive(entry, accept);
        const result = preparePublication(entry, held, active);
        if (!putPublication(records, result.writes, held, written, active)) return;
        outcomes.push({ address: entry.address,
          payloadWritten: result.writes.some(({ address }) => recordKey(address) === recordKey(entry.address)),
          receiptPublished: Boolean(result.receiptPublished), historyWritten: Boolean(result.historyWritten), reason: result.reason });
      }
      if (!guardPublicationWrite(records, accept)) return;
      done(written, outcomes);
    } catch (error) { abortForError(records, error); }
  });
}

function guardPublicationWrite(store, accept) {
  if (accept()) return true;
  abortForError(store, new Error("The canonical publication opening ended"));
  return false;
}

function putPublication(records, entries, held, written, accept) {
  for (const { address, value, publication = [], notify = true } of entries) {
    if (!guardPublicationWrite(records, accept)) return false;
    const record = { ...writeStamp(), value, publication };
    const key = recordKey(address);
    if (!putOrAbort(records, record, key)) return false;
    if (!guardPublicationWrite(records, accept)) return false;
    held.set(key, record);
    if (notify) written.push(address);
  }
  return true;
}

function effectsAddresses(...rows) {
  const addresses = rows.flatMap((row) => (row?.overlay || []).map(({ address }) => address));
  return [...new Map(addresses.map((address) => [recordKey(address), address])).values()];
}

function confirmationState(previous, input) {
  const confirmation = input.confirmation;
  const unapplied = confirmation.state === "failed" && confirmation.effect === "unapplied";
  const unresolved = confirmation.state !== "succeeded" && !unapplied;
  return {
    ...previous, ...input, confirmation, acknowledged: true,
    state: confirmation.state, effect: confirmation.effect, error: confirmation.error,
    overlayActive: Boolean((previous.overlayActive || input.overlay?.length) && !unapplied), unresolved,
  };
}

/** Create one authenticated namespace. Rendering activation is explicit so
 * disposing an account cannot expose another principal's accepted data. */
export function createWriteOperationCache(scope) {
  validateScope(scope);
  const namespace = operationNamespace(scope);
  const projectionOwner = randomToken();
  const belongs = (record) => record.namespace === namespace;
  const refreshAttempts = new WeakMap();
  const confirmationTransitions = new WeakSet();

  async function get(operationId) {
    const outcome = await strictTransaction("readonly", ({ operations }) => operations.get(operationKey(scope, operationId)));
    return outcome.result;
  }

  async function list() {
    const outcome = await strictTransaction("readonly", ({ operations }) => operations.index("namespace").getAll(namespace));
    return outcome.result.filter(belongs).sort((a, b) => a.localOrder - b.localOrder);
  }

  function announce(operationId, addresses = []) {
    notifications.announce(Object.values(notificationAddress(scope, operationId)).map(encodeURIComponent));
    announceWriteOperationCache(addresses);
  }

  function normalizeRecord(record, previous, owned) {
    const stamp = writeStamp();
    const next = {
      revision: 0, state: "queued", createdAt: stamp.at, localOrder: stamp.order,
      ...previous, ...record, ...scope, namespace,
    };
    delete next.refreshState;
    if (owned && !confirmationTransitions.has(record)) Object.assign(next, owned);
    validateRecord(next);
    return next;
  }

  async function update(operationId, updater, { canonical = [], accept = () => true, migrateProvisional = false } = {}) {
    let result, changed, addresses;
    await strictTransaction("readwrite", (stores) => {
      result = undefined;
      changed = false;
      const read = stores.operations.get(operationKey(scope, operationId));
      read.onsuccess = () => {
        try {
          const previous = read.result;
          result = previous;
          if (!accept()) return;
          const owned = protectedRefresh(previous);
          const candidate = updater(previous);
          if (candidate == null) return;
          const next = normalizeRecord(candidate, previous, owned);
          if (next.operationId !== operationId) throw new TypeError("Operation identity cannot change");
          if (!canAdvance(previous, next)) return;
          addresses = effectsAddresses(previous, next);
          writeCanonical(stores.records, canonical, accept, () => {
            readRecords(stores.records, addresses.map(recordKey), (records) => {
              if (!guardPublicationWrite(stores.records, accept)) return;
              if (migrateProvisional) migrateProvisionalThreadItems(stores.records, next, records);
              if (mayRetireLayer(next, records)) next.overlayActive = false;
              if (!next.refreshState) indexOverlay(next);
              if (!guardPublicationWrite(stores.operations, accept)) return;
              changed = putOrAbort(stores.operations, next, operationKey(scope, operationId));
              if (changed && !guardPublicationWrite(stores.operations, accept)) return;
              result = next;
            });
          });
        } catch (error) { abortForError(stores.operations, error); }
      };
      return null;
    });
    if (changed) announce(operationId, [...addresses, ...canonical.map(({ address }) => address)]);
    return result;
  }

  const put = (record) => update(record.operationId, (previous) => {
    if (!previous) return record;
    if (intentIdentity(previous) !== intentIdentity(record)) throw new Error("operation_conflict: the saved intent differs");
    return null;
  }, { migrateProvisional: true });
  function accept(operationId, input) {
    const { accept: authority = () => true, ...evidence } = input;
    return update(operationId, (previous) => {
      if (!previous) throw new Error("Cannot accept an unpersisted operation");
      if (previous.acknowledged && previous.revision >= evidence.revision) return null;
      return { ...previous, ...evidence, owners: evidence.owners || evidence.ack?.write?.owners || previous.owners,
        state: "accepted", acknowledged: true, overlayActive: true, unresolved: false };
    }, { accept: authority });
  }
  function confirm(operationId, input) {
    const { canonical = [], accept: authority = () => true, ...evidence } = input;
    return update(operationId, (previous) => {
      if (!previous) throw new Error("Cannot confirm an unpersisted operation");
      if (previous.confirmation && previous.revision >= evidence.revision) return null;
      const next = confirmationState(previous, evidence);
      confirmationTransitions.add(next);
      return next;
    }, { canonical, accept: authority });
  }

  async function observePublication({ canonical = [], accept = () => true, report = false, refresh, failure } = {}) {
    if (refresh) return observeRefreshPublication({ canonical, accept, report, refresh, failure });
    const changed = [];
    let written = [], outcomes = [];
    await strictTransaction("readwrite", (stores) => {
      changed.length = 0;
      written = [];
      outcomes = [];
      if (!accept()) return null;
      writeCanonical(stores.records, canonical, accept, (addresses, results) => {
        written = addresses;
        outcomes = results;
        if (!guardPublicationWrite(stores.records, accept)) return;
        // Task payload availability cannot retire this or another operation's
        // layer from an inherited publication vector.
        if (canonical.some(({ address }) => address.kind === TRACKER_TASK_KIND)) return;
        const read = stores.operations.index("overlayNamespace").getAll(namespace);
        read.onsuccess = () => retirePublished(stores, read.result, accept, changed);
      });
      return null;
    });
    for (const row of changed) announce(row.operationId, effectsAddresses(row));
    announceWriteOperationCache(written);
    return report ? { written, outcomes } : written;
  }

  function readOperation(stores, operationId, done) {
    const read = stores.operations.get(operationKey(scope, operationId));
    read.onsuccess = () => {
      try { done(read.result); }
      catch (error) { abortForError(stores.operations, error); }
    };
  }

  async function openRefresh({ operationId, revision, targets, accept = () => true }) {
    const addresses = canonicalRefreshTargets(targets);
    let result, permit, changed;
    await strictTransaction("readwrite", (stores) => {
      result = { state: "inactive" }; permit = undefined; changed = false;
      if (!accept()) return null;
      readOperation(stores, operationId, (row) => {
        if (row && !belongs(row)) { result = { state: "unbound" }; return; }
        const state = refreshOpeningState(row, revision, addresses, scope);
        result = { state };
        if (state !== "ready" || !guardPublicationWrite(stores.operations, accept)) return;
        const opened = openedRefresh(row, addresses);
        if (!opened.unread.length) { result = { state: "blocked" }; return; }
        const next = { ...row, refreshPending: true, refreshState: opened.state };
        if (!putGuardedOperation(stores.operations, next, scope, accept)) return;
        changed = true;
        permit = refreshPermit(next, opened.unread, namespace, accept);
        result = { state: "ready" };
      });
      return null;
    });
    if (permit) {
      const attempt = Object.freeze({});
      refreshAttempts.set(attempt, permit);
      result.attempt = attempt;
    }
    if (changed) announce(operationId);
    return result;
  }

  async function observeRefreshPublication({ canonical, accept, report, refresh, failure }) {
    if (failure && !["invalid-task-response", "unsupported"].includes(failure.reason)) throw new TypeError("Invalid refresh refusal");
    const permit = refreshAttempts.get(refresh);
    let result, changed;
    const active = () => Boolean(permit && permit.accept() && accept()
      && canonical.every((entry) => publicationIsActive(entry, accept)));
    await strictTransaction("readwrite", (stores) => {
      result = { written: [], outcomes: [], refresh: refreshOutcome(permit, "inactive") }; changed = false;
      if (!active()) return null;
      readOperation(stores, permit.operationId, (row) => {
        if (!currentRefreshAttempt(row, permit, canonical, failure)) {
          result.refresh = refreshOutcome(permit, "superseded"); return;
        }
        readRecords(stores.records, ownedPublicationAddresses(canonical, row).map(recordKey), (held) => {
          try {
            if (!guardPublicationWrite(stores.records, active)) return;
            const plan = ownedPublicationPlan(canonical, held, active);
            const next = parkedRefresh(row, permit, plan, failure);
            if (!queuePublicationPlan(stores.records, plan, active)) return;
            if (!putGuardedOperation(stores.operations, next, scope, active)) return;
            result = { written: plan.written, outcomes: plan.outcomes, refresh: refreshOutcome(permit, refreshPhase(next)) };
            changed = true;
          } catch (error) { abortForError(stores.records, error); }
        });
      });
      return null;
    });
    if (changed) announce(permit.operationId, result.written);
    return report ? result : result.written;
  }

  async function reevaluateRefresh({ operationId, accept = () => true }) {
    let result = { state: "inactive" };
    await strictTransaction("readonly", (stores) => {
      result = { state: "inactive" };
      if (!accept()) return null;
      readOperation(stores, operationId, (row) => {
        if (!row) { result = { state: "unbound" }; return; }
        readRecords(stores.records, blockedEvidenceAddresses(row).map(recordKey), () => {
          if (!accept()) return;
          // A changed cache hint is not positive watch authority. Genuine
          // proof rearming/ReceiptRef finalization is a separate dependency.
          result = { state: refreshPhase(row), revision: row.revision, generation: row.refreshState?.generation };
        });
      });
      return null;
    });
    return result;
  }

  async function watchRefresh({ operationId, generation }, listener) {
    const row = await get(operationId);
    if (row?.refreshState?.generation !== generation) return () => {};
    let active = true, queued = false;
    const hint = () => {
      if (!active || queued) return;
      queued = true;
      queueMicrotask(() => { queued = false; if (active) listener(); });
    };
    const stops = blockedEvidenceAddresses(row).map((address) => subscribeCache(address, hint));
    hint(); // Close the read/subscribe race without a timer or another RPC.
    return () => { active = false; for (const stop of stops) stop(); };
  }

  function retirePublished(stores, held, accept, changed) {
    const rows = held.filter((row) => belongs(row) && row.overlayActive && !row.refreshState && !pendingTaskRefresh(row));
    const keys = [...new Set(effectsAddresses(...rows).map(recordKey))];
    readRecords(stores.records, keys, (records) => {
      if (!guardPublicationWrite(stores.records, accept)) return;
      for (const row of rows) {
        if (!layerPublished(row, records)) continue;
        const next = { ...row, overlayActive: false };
        indexOverlay(next);
        if (!guardPublicationWrite(stores.operations, accept)) return;
        if (!putOrAbort(stores.operations, next, operationKey(scope, row.operationId))) return;
        if (!guardPublicationWrite(stores.operations, accept)) return;
        changed.push(row);
      }
    });
  }

  async function changeLease(tabId, ttlMs, action) {
    if (typeof tabId !== "string" || !tabId) throw new TypeError("Missing lease holder identity");
    if (!Number.isFinite(ttlMs) || ttlMs <= 0) throw new TypeError("Invalid lease duration");
    let changed = false;
    await strictTransaction("readwrite", ({ leases }) => {
      changed = false;
      const read = leases.get(leaseKey(scope));
      read.onsuccess = () => {
        const current = read.result;
        const now = Date.now();
        if (!leaseAllowed(current, tabId, now, action)) return;
        if (action === "release") leases.delete(leaseKey(scope));
        else putOrAbort(leases, { tabId, expiresAt: now + ttlMs }, leaseKey(scope));
        changed = true;
      };
      return null;
    });
    if (changed) announce(null);
    return changed;
  }

  return {
    get, list, put, update, accept, confirm, observePublication, openRefresh, reevaluateRefresh, watchRefresh,
    acquireLease: (tabId, ttlMs = 15000) => changeLease(tabId, ttlMs, "acquire"),
    renewLease: (tabId, ttlMs = 15000) => changeLease(tabId, ttlMs, "renew"),
    releaseLease: (tabId) => changeLease(tabId, 1, "release"),
    subscribe: (listener) => notifications.subscribe(notificationAddress(scope), (address) => listener(address.sub || null)),
    activateProjection: () => activateOperationProjection(scope, projectionOwner),
    deactivateProjection: () => deactivateOperationProjection(scope, projectionOwner),
  };
}

function leaseAllowed(current, tabId, now, action) {
  const owned = current?.tabId === tabId;
  if (action === "release") return owned;
  if (action === "renew") return owned && current.expiresAt > now;
  return owned || !current || current.expiresAt <= now;
}

/** Account reset is the sole application action that destroys durable intent. */
export async function wipePendingOperations() {
  await strictTransaction("readwrite", ({ operations, leases }) => {
    operations.clear();
    leases.clear();
    return null;
  });
  clearOperationProjections();
  notifications.announce([]);
  announceWriteOperationCache([{}]);
}

function sortedJson(value) {
  if (Array.isArray(value)) return value.map(sortedJson);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sortedJson(value[key])]));
}

const intentIdentity = (record) => JSON.stringify(sortedJson({ request: record.request, after: record.after }));


function indexOverlay(record) {
  record.overlayActive = Boolean(record.overlayActive && record.overlay?.length);
  if (record.overlayActive) record.overlayNamespace = record.namespace;
  else delete record.overlayNamespace;
}


function migrateProvisionalThreadItems(store, operation, held) {
  if (operation.request?.method !== "thread.post") return;
  for (const address of effectsAddresses(operation).filter((address) => address.kind === "thread")) {
    const key = recordKey(address);
    const record = held.get(key);
    const items = record?.value?.items;
    if (!Array.isArray(items)) continue;
    const kept = items.filter((item) => !(item?.data?.provisional === true && item.data.operation_id === operation.operationId));
    if (kept.length === items.length) continue;
    const migrated = { ...record, ...writeStamp(), value: { ...record.value, items: kept } };
    if (!putOrAbort(store, migrated, key)) return;
    held.set(key, migrated);
  }
}

function protectedRefresh(row) {
  if (!row?.refreshState) return null;
  return structuredClone({ revision: row.revision, confirmation: row.confirmation,
    refreshState: row.refreshState, refreshPending: row.refreshPending,
    overlay: row.overlay, overlayActive: row.overlayActive, overlayNamespace: row.overlayNamespace });
}

function pendingTaskRefresh(row) {
  return row.refreshPending !== false && row.confirmation?.refresh?.some(({ kind }) => kind === "task");
}

function mayRetireLayer(row, held) {
  return !row.refreshState && !pendingTaskRefresh(row) && layerPublished(row, held);
}

function canonicalRefreshTargets(targets) {
  if (!Array.isArray(targets)) throw new TypeError("Missing exact refresh targets");
  const addresses = targets.map(({ deviceId, entityId, kind, sub, threadId }) => ({ deviceId, entityId, kind, sub,
    ...(threadId ? { threadId } : {}) }));
  if (!addresses.every(validRefreshAddress)) throw new TypeError("Invalid canonical refresh target");
  const byKey = new Map(addresses.map((address) => [recordKey(address), address]));
  return [...byKey.keys()].sort().map((key) => byKey.get(key));
}

const supportedRefreshKind = (kind) => [TRACKER_TASK_KIND, "thread"].includes(kind);
const validRefreshAddress = (address) => supportedRefreshKind(address.kind)
  && [address.deviceId, address.entityId, address.sub].every((part) => typeof part === "string" && part.length > 0);
const sameRefreshAddress = (left, right) => Boolean(left && right && recordKey(left) === recordKey(right)
  && (left.threadId || "") === (right.threadId || ""));

function explicitRefreshProject(row) {
  return row.owner?.projectId || row.request?.params?.project_id || row.confirmation?.result?.task?.project_id;
}

function expectedRefreshProject(row) {
  const explicit = explicitRefreshProject(row);
  if (explicit) return explicit;
  const projects = [...new Set([...(row.owners || []), ...(row.confirmation?.published || [])]
    .filter(({ kind }) => kind === "project").map(({ id }) => id))];
  return projects.length === 1 ? projects[0] : null;
}

function namedRefreshTargetsMatch(row, targets, scope) {
  const expected = expectedRefreshTargets(row, scope);
  return Boolean(expected && targets.length === expected.length
    && targets.every((address, index) => sameRefreshAddress(address, expected[index])));
}

function expectedRefreshTargets(row, scope) {
  const named = (row.confirmation.refresh || []).map((item) => namedRefreshAddress(row, item, scope));
  if (named.some((address) => !address)) return null;
  const overlay = effectsAddresses(row).filter((address) => address.deviceId === scope.deviceId && supportedRefreshKind(address.kind));
  if (!overlay.every((address) => mappedRefreshAddress(row, address, scope))) return null;
  return canonicalRefreshTargets([...named, ...overlay]);
}

function namedRefreshAddress(row, item, scope) {
  if (!item?.id) return null;
  if (item.kind === "task") {
    const project = expectedRefreshProject(row);
    return project ? taskAddress(scope.deviceId, project, item.id) : null;
  }
  if (item.kind !== "thread") return null;
  const params = row.request?.params || {};
  return [params.thread_id, params.conversation_id, params.agent_id].includes(item.id) ? persistedThreadAddress(row, scope) : null;
}

function persistedThreadAddress(row, scope) {
  const params = row.request?.params || {};
  if (!params.entity_id || !(params.conversation_id || params.agent_id)) return null;
  return threadCacheAddress({ deviceId: scope.deviceId, entityId: params.entity_id,
    conversationId: params.conversation_id, agentId: params.agent_id, threadId: params.thread_id });
}

function mappedRefreshAddress(row, address, scope) {
  if (!validRefreshAddress(address)) return false;
  if (address.kind === "thread") return sameRefreshAddress(address, persistedThreadAddress(row, scope));
  return address.entityId === expectedRefreshProject(row);
}

function validRefreshState(row, targets) {
  const state = row.refreshState;
  if (!state || state.version !== 1 || state.revision !== row.revision || !state.generation) return false;
  if (JSON.stringify(state.required) !== JSON.stringify(row.confirmation.published || [])) return false;
  return Array.isArray(state.targets) && state.targets.length === targets.length && state.targets.every((target, index) =>
    target.key === recordKey(targets[index]) && sameRefreshAddress(target.address, targets[index]));
}

function refreshOpeningState(row, revision, targets, scope) {
  if (!row || !row.confirmation) return "unbound";
  if (row.revision !== revision || row.confirmation.revision !== revision) return "superseded";
  if (!namedRefreshTargetsMatch(row, targets, scope)) return "unbound";
  if (row.refreshPending === false) return "complete";
  if (validRefreshState(row, targets)) return refreshPhase(row);
  return "ready";
}

function openedRefresh(row, targets) {
  const state = validRefreshState(row, targets) ? row.refreshState : {
    version: 1, revision: row.revision, generation: randomToken(), required: row.confirmation.published || [],
    targets: targets.map((address) => ({ key: recordKey(address), address, phase: "unread", attemptId: null, blocked: null, receipt: null })),
  };
  const slots = state.targets.map((target) => target.phase === "unread" ? { ...target, attemptId: randomToken() } : target);
  return { state: { ...state, targets: slots }, unread: slots.filter(({ phase }) => phase === "unread") };
}

function refreshPermit(row, unread, namespace, accept) {
  return { namespace, operationId: row.operationId, revision: row.revision, generation: row.refreshState.generation,
    required: structuredClone(row.refreshState.required), targets: new Map(unread.map((target) => [target.key, target.attemptId])), accept };
}

function refreshPhase(row) {
  if (row.refreshPending === false) return "complete";
  const targets = row.refreshState?.targets;
  return !targets || targets.some(({ phase }) => phase === "unread") ? "ready" : "blocked";
}

function refreshOutcome(permit, state) {
  return { operationId: permit?.operationId, revision: permit?.revision, generation: permit?.generation, state };
}

function currentRefreshAttempt(row, permit, entries, failure) {
  if (!refreshIdentityMatches(row, permit)) return false;
  const state = row.refreshState;
  const keys = entries.map(({ address }) => recordKey(address));
  if (failure) keys.push(failure.targetKey);
  return keys.length > 0 && new Set(keys).size === keys.length && keys.every((key) => {
    const target = state.targets.find((slot) => slot.key === key);
    return permit.targets.has(key) && target?.phase === "unread" && target.attemptId === permit.targets.get(key);
  });
}

function refreshIdentityMatches(row, permit) {
  return Boolean(row && row.namespace === permit.namespace && row.revision === permit.revision && row.refreshPending !== false
    && row.refreshState?.generation === permit.generation && row.refreshState.revision === permit.revision
    && JSON.stringify(row.refreshState.required) === JSON.stringify(permit.required));
}

function ownedPublicationAddresses(entries, row) {
  const addresses = [...publicationReadAddresses(entries),
    ...row.refreshState.targets.flatMap(({ address }) => refreshEvidenceAddresses(address)), ...blockedEvidenceAddresses(row)];
  return [...new Map(addresses.map((address) => [recordKey(address), address])).values()];
}

function ownedPublicationPlan(entries, held, accept) {
  const plan = { held: new Map(held), records: [], outcomes: [], written: [] };
  for (const entry of entries) {
    validateFences(entry.publication || []);
    const selected = publicationGuardMatches(entry, plan.held) ? preparePublication(entry, plan.held, accept)
      : { writes: [], reason: "superseded" };
    for (const write of selected.writes) {
      const record = { ...writeStamp(), value: write.value, publication: write.publication || [] };
      plan.records.push({ ...write, record });
      plan.held.set(recordKey(write.address), record);
      if (write.notify !== false) plan.written.push(write.address);
    }
    plan.outcomes.push({ address: entry.address,
      payloadWritten: selected.writes.some(({ address }) => recordKey(address) === recordKey(entry.address)),
      historyWritten: Boolean(selected.historyWritten), receiptPublished: Boolean(selected.receiptPublished), reason: selected.reason });
  }
  return plan;
}

function parkedRefresh(row, permit, plan, failure) {
  const outcomes = new Map(plan.outcomes.map((outcome) => [recordKey(outcome.address), outcome]));
  const targets = row.refreshState.targets.map((target) => {
    const outcome = outcomes.get(target.key);
    const refused = failure?.targetKey === target.key;
    if (!outcome && !refused) return target;
    const reason = refused ? failure.reason : outcome.reason || "receipt-reference-unavailable";
    return { ...target, phase: "blocked", receipt: null,
      blocked: { reason, seen: { records: evidenceReferences(target.address, plan.held) } } };
  });
  // Genuine ReceiptRef creation/revalidation/finalization is deliberately not
  // implemented in this negative parked path. Pending and layers stay intact.
  return { ...row, refreshState: { ...row.refreshState, generation: permit.generation, targets } };
}

function evidenceReferences(address, held) {
  return refreshEvidenceAddresses(address).map((target) => ({ address: target, written: recordWriteOf(held.get(recordKey(target))) }));
}

const refreshEvidenceAddresses = (address) => address.kind === TRACKER_TASK_KIND ? taskPublicationEvidenceAddresses(address) : [address];

function blockedEvidenceAddresses(row) {
  const addresses = (row.refreshState?.targets || []).flatMap((target) => target.blocked?.seen?.records || []).map(({ address }) => address);
  return [...new Map(addresses.map((address) => [recordKey(address), address])).values()];
}

function queuePublicationPlan(records, plan, accept) {
  for (const { address, record } of plan.records) {
    if (!guardPublicationWrite(records, accept)) return false;
    if (!putOrAbort(records, record, recordKey(address))) return false;
    if (!guardPublicationWrite(records, accept)) return false;
  }
  return guardPublicationWrite(records, accept);
}

function putGuardedOperation(operations, row, scope, accept) {
  if (!guardPublicationWrite(operations, accept)) return false;
  if (!putOrAbort(operations, row, operationKey(scope, row.operationId))) return false;
  return guardPublicationWrite(operations, accept);
}
