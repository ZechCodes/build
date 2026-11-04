// A scope's journal cursor shares the publication transaction with all its
// task replicas and memberships. Received data never acknowledges itself.
import { cachedAddresses, DELETE_CACHED_RECORD, readCanonicalCached } from "./localCache.js";
import { captureTaskContentRead, mergeTaskContentRecordsTogether } from "./taskContentAuthority.js";
import { taskMutationEpoch, taskMutationsPending } from "./taskMutationFence.js";
import { summaryAddress, summaryQueryAddress } from "./taskSummaryCache.js";
import { projectColumnsAddress, withProjectColumns } from "./projectColumnsCache.js";
import { taskAddress, tasksAddress, tasksQueryAddress } from "./trackerCache.js";
import { currentRevision, isMembership, matchingMembership, membershipValue, removedDetail,
  revisionAddress, revisionValue, scopeFilters, snapshotMembers, taskRecordValue } from "./scopedTaskSyncRecords.js";

// A project eviction removes its recovery position in the same range delete
// as its replicas. Retaining that position after dropping content would make
// a no-change replay leave a reopened view permanently empty.
export const syncCursorAddress = (deviceId, scope, projectId = scope.project_id) => ({
  deviceId, entityId: projectId, kind: "sync-cursor", sub: JSON.stringify(scope),
});
const keyOf = (address) => JSON.stringify([address.deviceId, address.entityId, address.kind, address.sub || ""]);
const distinct = (addresses) => [...new Map(addresses.map((address) => [keyOf(address), address])).values()];
const tokenOf = (cursor) => cursor?.cursor ?? null;
const generations = ["storeGeneration", "scopeGeneration", "retirementEpoch"];

function acceptsCapture(capture, context, active, deviceId) {
  return active() && capture?.mutationEpoch === taskMutationEpoch(deviceId)
    && capture.pending === 0 && taskMutationsPending(deviceId) === 0
    && generations.every((key) => capture.contentRead?.ticket?.[key]
      && capture.contentRead.ticket[key] === context[key]);
}

const snapshotDemand = (previous, records, snapshot) => !snapshot
  && (previous?.snapshotRequired === true || records.some((record) => record.operation === "invalidate"));

function cursorValue(response, previous, bridgeIdentity, snapshot, records) {
  const head = snapshot ? response.watermark : response.head;
  return { incarnation: response.incarnation, cursor: response.cursor, head,
    appliedHead: response.continuation ? previous?.appliedHead ?? null : head,
    snapshotRequired: snapshotDemand(previous, records, snapshot),
    bridgeIdentity, bridge_epoch: response.bridge_epoch };
}

function primaryMemberships(place, scope, queryParams) {
  if (scope.kind !== "project_tasks") return [];
  const filters = scopeFilters(scope);
  return [summaryQueryAddress(place, filters), tasksQueryAddress(place.deviceId, place.projectId, queryParams ||
    { project_id: place.projectId, ...scope.filters, ...(scope.order && scope.order !== "number" ? { order: scope.order } : {}) }),
    ...(Object.keys(filters).length === 0 ? [tasksAddress(place.deviceId, place.projectId)] : [])];
}

function publicationAddresses(place, scope, cursor, existing, records, queryParams) {
  return distinct([cursor, projectColumnsAddress(place.deviceId, place.projectId), ...primaryMemberships(place, scope, queryParams),
    ...existing.filter(isMembership),
    ...records.flatMap((record) => [summaryAddress(place, record.key), taskAddress(place.deviceId, place.projectId, record.key),
      revisionAddress(place, record.key)]),
    ...(scope.kind === "task" ? [taskAddress(place.deviceId, place.projectId, scope.task_id)] : [])]);
}

function recordMap(addresses, held) {
  return new Map(addresses.map((address, index) => [keyOf(address), held[index]]));
}

function admittedRecords(records, get, place, incarnation) {
  const newest = new Map();
  for (const record of records) {
    const prior = newest.get(record.key) || get(revisionAddress(place, record.key))?.value;
    if (currentRevision(prior, record, incarnation)) newest.set(record.key, { ...record, ...revisionValue(record, incarnation) });
  }
  return [...newest.values()];
}

function applyRows(addresses, held, records, options) {
  const get = (address) => held.get(keyOf(address));
  const selected = admittedRecords(records, get, options.place, options.response.incarnation);
  const next = new Map(), taskOf = (id) => next.get(keyOf(summaryAddress(options.place, id)))?.task
    || get(summaryAddress(options.place, id))?.value?.task;
  for (const record of selected) {
    next.set(keyOf(revisionAddress(options.place, record.key)), revisionValue(record, options.response.incarnation));
    for (const address of [summaryAddress(options.place, record.key), taskAddress(options.place.deviceId, options.place.projectId, record.key)]) {
      next.set(keyOf(address), taskRecordValue(get(address), record, { ...options, address }));
    }
  }
  const revisionOf = (id) => next.get(keyOf(revisionAddress(options.place, id)))
    || get(revisionAddress(options.place, id))?.value;
  const members = options.snapshot ? snapshotMembers(records, revisionOf, options.response.incarnation) : selected;
  for (const address of addresses.filter(isMembership)) {
    const matches = matchingMembership(address, options.scope);
    const membership = membershipValue(get(address), members,
      { ...options, address, matches, taskOf, snapshot: options.snapshot && matches, removed: options.removed && matches });
    const columns = get(projectColumnsAddress(options.place.deviceId, options.place.projectId))?.value;
    next.set(keyOf(address), withProjectColumns(membership, columns));
  }
  return next;
}

function reducePublication(addresses, records, options, held, context) {
  const previous = held[0]?.value;
  if (!acceptsCapture(options.capture, context, options.active, options.place.deviceId)
    || tokenOf(previous) !== (options.expectedCursor ?? null)) return null;
  const cached = recordMap(addresses, held);
  const next = applyRows(addresses, cached, records, { ...options, context });
  if (options.removed && options.scope.kind === "task") {
    const detail = taskAddress(options.place.deviceId, options.place.projectId, options.scope.task_id);
    next.set(keyOf(detail), removedDetail(cached.get(keyOf(detail))));
  }
  const values = addresses.map((address) => next.get(keyOf(address)) ?? null);
  values[0] = options.removed ? DELETE_CACHED_RECORD
    : cursorValue(options.response, previous, options.bridgeIdentity, options.snapshot, records);
  return { values, retire: options.removed };
}

export function createScopedTaskSyncCache({ deviceId, scope, projectId, bridgeIdentity, queryParams, active = () => true }) {
  const place = { deviceId, projectId: projectId || scope.project_id };
  const cursor = syncCursorAddress(deviceId, scope, place.projectId);
  const readCursor = async () => (await readCanonicalCached(cursor))?.value;
  const capture = async () => {
    const mutationEpoch = taskMutationEpoch(deviceId), pending = taskMutationsPending(deviceId);
    const contentRead = await captureTaskContentRead(place, { revalidate: "summary" });
    return { contentRead, mutationEpoch, pending };
  };
  const publish = async ({ records = [], ...publication }) => {
    const existing = await cachedAddresses({ deviceId, entityId: place.projectId });
    const addresses = publicationAddresses(place, scope, cursor, existing, records, queryParams);
    const options = { ...publication, place, scope, bridgeIdentity, active };
    return mergeTaskContentRecordsTogether(place, addresses,
      (held, context) => reducePublication(addresses, records, options, held, context),
      { watchTaskIds: records.map((record) => record.key) });
  };
  const reset = async ({ capture: captured, expectedCursor } = {}) => {
    let admitted = false, committed = false;
    await mergeTaskContentRecordsTogether(place, [cursor], ([held], context) => {
      admitted = acceptsCapture(captured, context, active, deviceId) && tokenOf(held?.value) === (expectedCursor ?? null);
      return admitted ? { values: [DELETE_CACHED_RECORD] } : null;
    }, { onCommit: () => { committed = admitted; } });
    return committed;
  };
  return { readCursor, capture, publish, reset };
}
