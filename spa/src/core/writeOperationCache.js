// Durable operation intent and accepted overlays share the replica transaction
// boundary, but live in stores that replica resets and lifetime sweeps never clear.
import { abortForError, putOrAbort } from "./idbDatabase.js";
import { createAnnouncer, randomToken, recordKey, writeStamp } from "./idbRecords.js";
import { announceWriteOperationCache, transactWriteOperations } from "./localCache.js";
import {
  activateOperationProjection, clearOperationProjections, deactivateOperationProjection, layerPublished,
  leaseKey, operationKey, operationNamespace, validPublicationFence,
} from "./writeOperationProjection.js";

import { publicationReadAddresses, publicationGuardMatches, preparePublicationWrites } from "./writeOperationPublication.js";

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
      if (!accept()) return done([]);
      const written = [];
      for (const entry of entries) {
        validateFences(entry.publication || []);
        if (!publicationGuardMatches(entry, held)) continue;
        if (!putPublication(records, preparePublicationWrites(entry, held), held, written)) return;
      }
      done(written);
    } catch (error) { abortForError(records, error); }
  });
}

function putPublication(records, entries, held, written) {
  for (const { address, value, publication = [], notify = true } of entries) {
    const record = { ...writeStamp(), value, publication };
    const key = recordKey(address);
    if (!putOrAbort(records, record, key)) return false;
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

  function normalizeRecord(record, previous) {
    const stamp = writeStamp();
    const next = {
      revision: 0, state: "queued", createdAt: stamp.at, localOrder: stamp.order,
      ...previous, ...record, ...scope, namespace,
    };
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
          const candidate = updater(previous);
          if (candidate == null) return;
          const next = normalizeRecord(candidate, previous);
          if (next.operationId !== operationId) throw new TypeError("Operation identity cannot change");
          if (!canAdvance(previous, next)) return;
          addresses = effectsAddresses(previous, next);
          writeCanonical(stores.records, canonical, accept, () => {
            readRecords(stores.records, addresses.map(recordKey), (records) => {
              if (!accept()) return;
              if (migrateProvisional) migrateProvisionalThreadItems(stores.records, next, records);
              if (layerPublished(next, records)) next.overlayActive = false;
              indexOverlay(next);
              changed = putOrAbort(stores.operations, next, operationKey(scope, operationId));
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
      return confirmationState(previous, evidence);
    }, { canonical, accept: authority });
  }

  async function observePublication({ canonical = [], accept = () => true } = {}) {
    const changed = [];
    let written = [];
    await strictTransaction("readwrite", (stores) => {
      changed.length = 0;
      written = [];
      if (!accept()) return null;
      writeCanonical(stores.records, canonical, accept, (addresses) => {
        written = addresses;
        if (!accept()) return;
        const read = stores.operations.index("overlayNamespace").getAll(namespace);
        read.onsuccess = () => retirePublished(stores, read.result, accept, changed);
      });
      return null;
    });
    for (const row of changed) announce(row.operationId, effectsAddresses(row));
    announceWriteOperationCache(written);
    return written;
  }

  function retirePublished(stores, held, accept, changed) {
    const rows = held.filter((row) => belongs(row) && row.overlayActive);
    const keys = [...new Set(effectsAddresses(...rows).map(recordKey))];
    readRecords(stores.records, keys, (records) => {
      if (!accept()) return;
      for (const row of rows) {
        if (!layerPublished(row, records)) continue;
        const next = { ...row, overlayActive: false };
        indexOverlay(next);
        if (putOrAbort(stores.operations, next, operationKey(scope, row.operationId))) changed.push(row);
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
    get, list, put, update, accept, confirm, observePublication,
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
