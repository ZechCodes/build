// Device publication and its opaque recovery position commit together. Raw
// family rows exist only to join independent deltas into established caches.
import { cachedAddresses, DELETE_CACHED_RECORD, mergeCachedRecordsTogether, readCanonicalCached,
  readCanonicalCachedMany, recordWriteOf } from "./localCache.js";
import { captureTaskContentRead, taskContentFragment } from "./taskContentAuthority.js";
import { taskMutationEpoch, taskMutationsPending } from "./taskMutationFence.js";
import { syncCursorAddress } from "./scopedTaskSyncCache.js";
import { taskRecordValue } from "./scopedTaskSyncRecords.js";
import { summaryAddress } from "./taskSummaryCache.js";
import { taskAddress, tasksAddress } from "./trackerCache.js";
import { pushFence, pushedSince } from "./pushFence.js";
import { deviceOwnedProjection } from "./scopedDeviceFields.js";
import { deviceArchive, deviceConversationRecords, deviceDirectories, deviceFamilies, deviceFeed, deviceListedOwners, deviceOwners,
  devicePendingConversationOwners, deviceProjectRosters, deviceRecordAddress,
  deviceRecordId, deviceRoot, removedDeviceRecord } from "./scopedDeviceSyncRecords.js";

const addressKey = ({ deviceId, entityId, kind, sub = "" }) => JSON.stringify([deviceId, entityId, kind, sub]);
const distinct = (addresses) => [...new Map(addresses.map((address) => [addressKey(address), address])).values()];
const taskKinds = new Set(["tracker-summary", "tracker-task", "tracker-tasks"]);
const dataKinds = new Set(["sync-device-record", "projects", "workspaces", "device-projects", "project-settings", "row", "project-agents",
  "feed", "archive", "user-session", "tracker-summary-identity", ...taskKinds]);
const sourceRow = (record) => record.row || record.previousRow;
const taskPlace = (deviceId, record) => ({ deviceId, projectId: sourceRow(record)?.project_id });
const rawRows = (addresses, held) => new Map(addresses.flatMap((address, index) => address.kind === "sync-device-record" && held[index]?.value
  ? [[address.sub, held[index].value]] : []));
const isCurrentRevision = (previous, record, incarnation) => previous?.incarnation !== incarnation
  || BigInt(record.revision) > BigInt(previous.revision)
  || !previous.deleted && BigInt(record.revision) === BigInt(previous.revision);

function stageRecords(rows, records, incarnation) {
  const admitted = [];
  for (const record of records) {
    if (!deviceFamilies.has(record.kind)) continue;
    const key = deviceRecordId(record.kind, record.key), previous = rows.get(key);
    if (!isCurrentRevision(previous, record, incarnation)) continue;
    if (record.operation === "invalidate") continue;
    const deleted = removedDeviceRecord(record);
    const next = { ...record, incarnation, deleted, ...(deleted ? { previousRow: sourceRow(previous || {}) } : {}) };
    rows.set(key, next); admitted.push(next);
  }
  return admitted;
}

function snapshotDeletions(rows, records, response, snapshot) {
  if (!snapshot) return [];
  const present = new Set(records.map((record) => deviceRecordId(record.kind, record.key)));
  return [...rows.values()].filter((record) => !record.deleted && !present.has(deviceRecordId(record.kind, record.key))
    && (record.incarnation !== response.incarnation || BigInt(record.revision) <= BigInt(response.watermark)))
    .map((record) => ({ kind: record.kind, key: record.key, revision: response.watermark,
      operation: "delete", member: false, row: null }));
}

const knownProjects = (records) => [...new Set(records.map((record) => sourceRow(record)?.project_id).filter(Boolean))];

function targetAddresses(deviceId, records, previous, existing) {
  const rows = [...previous, ...records];
  const roots = ["projects", "workspaces", "device-projects", "feed", "archive", "user-session"].map((kind) => deviceRoot(deviceId, kind));
  const owners = rows.map(sourceRow).map((row) => row?.owner_id).filter(Boolean);
  const projects = knownProjects(rows);
  const taskProjects = knownProjects(rows.filter((record) => record.kind === "inbox_task"));
  return distinct([...roots, ...existing.filter((address) => dataKinds.has(address.kind) && !taskKinds.has(address.kind)),
    ...rows.map((record) => deviceRecordAddress(deviceId, record.kind, record.key)),
    ...owners.flatMap((owner) => [deviceRoot(deviceId, "row", owner), deviceRoot(deviceId, "project-agents", owner)]),
    ...rows.filter((record) => record.kind === "agent" && sourceRow(record)).map((record) => {
      const agent = sourceRow(record);
      return deviceRoot(deviceId, "thread", agent.owner_id, agent.conversation_id || agent.id);
    }),
    ...projects.map((project) => deviceRoot(deviceId, "project-settings", project)),
    ...taskProjects.map((project) => tasksAddress(deviceId, project)),
    ...rows.filter((record) => record.kind === "inbox_task" && sourceRow(record)?.project_id).flatMap((record) => {
      const place = taskPlace(deviceId, record);
      return [summaryAddress(place, record.key), taskAddress(deviceId, place.projectId, record.key)];
    }),
    ...projects.flatMap((project) => rows.filter((record) => record.kind === "identity").map((record) =>
      deviceRoot(deviceId, "tracker-summary-identity", project, record.key))),
  ]);
}

function projectionWriter(addresses, held) {
  const heldByKey = new Map(addresses.map((address, index) => [addressKey(address), held[index]])), next = new Map();
  const get = (address) => heldByKey.get(addressKey(address));
  return { get, next, set: (address, value) => next.set(addressKey(address), value),
    values: () => addresses.map((address) => next.get(addressKey(address)) ?? null) };
}

function writeDirectories(rows, writer, deviceId) {
  const held = (kind, owner = "") => writer.get(deviceRoot(deviceId, kind, owner))?.value;
  const directories = deviceDirectories(rows, held, deviceId);
  for (const kind of ["projects", "workspaces"]) writer.set(deviceRoot(deviceId, kind), directories[kind]);
  writer.set(deviceRoot(deviceId, "device-projects"), directories.projects);
  for (const project of directories.projects) writer.set(deviceRoot(deviceId, "project-settings", project.id),
    { ...held("project-settings", project.id), ...project });
  for (const record of [...rows.values()].filter((record) => record.kind === "project" && record.deleted)) {
    writer.set(deviceRoot(deviceId, "project-settings", record.key), DELETE_CACHED_RECORD);
  }
  const fullOwners = deviceOwners(rows, held, deviceId);
  const rosters = deviceProjectRosters(rows, fullOwners, held);
  const owners = deviceListedOwners(fullOwners, rosters);
  const visible = writeOwners(rows, owners, rosters, held, writer, deviceId);
  removeUnclassifiedOwners(rows, fullOwners, rosters, writer, deviceId);
  removeDeletedOwners(rows, owners, rosters, writer, deviceId);
  writer.set(deviceRoot(deviceId, "feed"), deviceFeed(rows, visible, directories, held, deviceId));
}

function ownerDigests(rows) {
  const agents = [...rows.values()].filter((record) => record.kind === "agent" && !record.deleted);
  const groups = new Map();
  for (const record of [...agents, ...deviceConversationRecords(rows)]) {
    const owner = record.row.owner_id;
    if (!groups.has(owner)) groups.set(owner, []);
    groups.get(owner).push(record);
  }
  return groups;
}

const cachedFeedOwner = (held, owner) => held("row", owner)
  || held("feed")?.items?.find((row) => (row.entity_id || row.run_id) === owner);

function writeOwners(rows, owners, rosters, held, writer, deviceId) {
  const digests = ownerDigests(rows), visible = new Map();
  const pending = devicePendingConversationOwners(rows);
  for (const [owner, row] of owners) {
    if (pending.has(owner) || !currentThreads(rows, digests.get(owner) || [], writer, deviceId)) {
      // An unrelated metadata page must advance before a later agent-reset
      // page. Keep this owner's current paint until its raw generation catches up.
      const current = cachedFeedOwner(held, owner);
      if (current) visible.set(owner, current);
      continue;
    }
    visible.set(owner, row);
    writer.set(deviceRoot(deviceId, "row", owner), row);
    if (rosters.has(owner)) writer.set(deviceRoot(deviceId, "project-agents", owner),
      { ...held("project-agents", owner), ...rosters.get(owner) });
  }
  return visible;
}

function removeUnclassifiedOwners(rows, owners, rosters, writer, deviceId) {
  for (const record of rows.values()) {
    const owner = sourceRow(record)?.owner_id;
    if (!owner) continue;
    if (!owners.has(owner)) writer.set(deviceRoot(deviceId, "row", owner), DELETE_CACHED_RECORD);
    if (!rosters.has(owner)) writer.set(deviceRoot(deviceId, "project-agents", owner), DELETE_CACHED_RECORD);
  }
}

function removeDeletedOwners(rows, owners, rosters, writer, deviceId) {
  const removed = [...rows.values()].filter((record) => ["agent", "owner_session"].includes(record.kind) && record.deleted)
    .map((record) => record.previousRow?.owner_id || (record.kind === "owner_session" ? record.key : null)).filter(Boolean);
  for (const owner of removed) if (!owners.has(owner)) {
    writer.set(deviceRoot(deviceId, "row", owner), DELETE_CACHED_RECORD);
    if (!rosters.has(owner)) writer.set(deviceRoot(deviceId, "project-agents", owner), DELETE_CACHED_RECORD);
  }
}

function writeAuxiliary(rows, selected, writer, deviceId, projects) {
  if (selected.some((record) => record.kind === "archived_worktree")) writer.set(deviceRoot(deviceId, "archive"),
    deviceArchive(rows, writer.get(deviceRoot(deviceId, "archive"))?.value));
  writeSession(selected, writer, deviceId);
  writeIdentities(rows, selected, writer, deviceId, projects);
}

function writeSession(selected, writer, deviceId) {
  for (const record of selected.filter((record) => record.kind === "user_session")) {
    const address = deviceRoot(deviceId, "user-session"), held = writer.get(address)?.value;
    if (record.deleted) writer.set(address, DELETE_CACHED_RECORD);
    else if (!held || record.row.last_activity_ms >= held.last_activity_ms) writer.set(address, { ...held, ...record.row });
  }
}

function writeIdentities(rows, selected, writer, deviceId, projects) {
  for (const record of selected.filter((record) => record.kind === "identity")) {
    for (const [projectId, project] of projects) {
      const address = deviceRoot(deviceId, "tracker-summary-identity", projectId, record.key);
      const held = writer.get(address)?.value;
      if ((held?.read_order || 0) <= project.read.readOrder) writer.set(address,
        record.deleted ? DELETE_CACHED_RECORD : { identity: hydratedIdentity(rows, record, held?.identity), read_order: project.read.readOrder });
    }
  }
}

function hydratedIdentity(rows, record, held) {
  const agent = rows.get(deviceRecordId("agent", record.key))?.row;
  // SQL identities deliberately omit runtime availability and registry links.
  // Retain observed availability and hydrate the link from this paired agent.
  return { ...held, ...record.row, workspace_id: agent?.workspace_id ?? held?.workspace_id ?? record.row.workspace_id };
}

function acceptsPublication(held, options) {
  return options.accept() && currentCapture(options.captured, options.deviceId, options.active)
    && captureStillHeld(options.captured, options.addresses, held)
    && (held[0]?.value?.cursor ?? null) === (options.expectedCursor ?? null);
}

function applyPublication(all, options) {
  const { addresses, projects, fragments, deviceId, records, response, snapshot, bridgeIdentity, cursor } = options;
  const held = all.slice(0, addresses.length);
  if (!acceptsPublication(held, options)) return null;
  let offset = addresses.length;
  for (const project of fragments) {
    project.context = project.fragment.context(all.slice(offset, offset + project.fragment.addresses.length));
    offset += project.fragment.addresses.length;
  }
  if (!currentProjects(projects)) return null;
  const rows = rawRows(addresses, held);
  const completeRecords = [...records, ...snapshotDeletions(rows, records, response, snapshot)];
  const selected = stageRecords(rows, completeRecords, response.incarnation);
  const writer = projectionWriter(addresses, held);
  if (!currentThreads(rows, selected, writer, deviceId)) return null;
  for (const record of selected) writer.set(deviceRecordAddress(deviceId, record.kind, record.key), record);
  if (selected.length) {
    writeDirectories(rows, writer, deviceId);
    writeTasks(selected, writer, deviceId, projects);
    writeAuxiliary(rows, selected, writer, deviceId, projects);
  }
  writer.set(cursor, cursorValue(response, held[0]?.value, bridgeIdentity, snapshot, records));
  return [...writer.values(), ...fragments.flatMap(({ fragment }) => fragment.addresses.map(() => null))];
}

function writeTasks(selected, writer, deviceId, projects) {
  for (const record of selected.filter((record) => record.kind === "inbox_task")) {
    const place = taskPlace(deviceId, record), project = projects.get(place.projectId);
    if (!project) continue; // A null registry link cannot address any SPA project cache.
    const summary = summaryAddress(place, record.key), held = writer.get(summary);
    // inbox_task membership ends on unwatch too; it cannot delete the task's
    // ordinary content or evict it from the independently maintained whole list.
    const incoming = record.deleted ? { ...held?.value?.task, ...sourceRow(record), watched: false } : record.row;
    if (!incoming?.id) continue;
    const taskRecord = { ...record, operation: "upsert", kind: "task", row: incoming };
    for (const address of [summary, taskAddress(deviceId, place.projectId, record.key)]) {
      writer.set(address, taskRecordValue(writer.get(address), taskRecord,
        { place, address, capture: { contentRead: project.read }, context: project.context }));
    }
    updateTaskList(writer, deviceId, place.projectId, record.key, summary);
  }
}

function updateTaskList(writer, deviceId, projectId, id, summary) {
  const address = tasksAddress(deviceId, projectId), held = writer.get(address)?.value || {};
  const current = writer.next.get(addressKey(address)) || held;
  const task = writer.next.get(addressKey(summary))?.task;
  if (!task) return;
  const tasks = new Map((current.tasks || []).map((task) => [task.id, task]));
  tasks.set(id, task);
  writer.set(address, { ...current, tasks: [...tasks.values()] });
}

function currentCapture(capture, deviceId, active) {
  return active() && capture?.mutationEpoch === taskMutationEpoch(deviceId)
    && capture.pending === 0 && taskMutationsPending(deviceId) === 0;
}

function captureStillHeld(capture, addresses, held) {
  return addresses.every((address, index) => !pushedSince(address, capture.fence)
    && (!dataKinds.has(address.kind) || capturedValueCurrent(capture, address, held[index])));
}

function capturedValueCurrent(capture, address, held) {
  const key = addressKey(address), written = capture.writes.get(key) ?? null;
  if (written === recordWriteOf(held)) return true;
  return written !== null && capture.projections.has(key)
    && capture.projections.get(key) === deviceOwnedProjection(address.kind, held?.value);
}

function captureProjections(addresses, held) {
  return new Map(addresses.flatMap((address, index) => {
    const projection = deviceOwnedProjection(address.kind, held[index]?.value);
    return held[index] && projection !== undefined ? [[addressKey(address), projection]] : [];
  }));
}

function currentProjects(projects) {
  return [...projects.values()].every(({ read, context }) => ["storeGeneration", "scopeGeneration", "retirementEpoch"]
    .every((key) => read.ticket?.[key] && read.ticket[key] === context[key]));
}

function digestNotRetired(held, incoming) {
  if (!held || !incoming.thread_id) return true;
  return !held.retired_thread_ids?.includes(incoming.thread_id)
    && BigInt(held.thread_generation_revision || 0) <= BigInt(incoming.thread_generation_revision || 0);
}

const cachedOwnerAgent = (writer, deviceId, kind, agent) => writer.get(deviceRoot(deviceId, kind, agent.owner_id))
  ?.value?.agents?.find((held) => held.id === agent.id);

function currentThreads(rows, selected, writer, deviceId) {
  return selected.filter((record) => !record.deleted && ["agent", "inbox_conversation"].includes(record.kind)).every((record) => {
    const incoming = record.row;
    const agent = rows.get(deviceRecordId("agent", incoming.agent_id || incoming.id))?.row;
    if (!agent) return true;
    const thread = writer.get(deviceRoot(deviceId, "thread", agent.owner_id, agent.conversation_id || agent.id))?.value;
    return [thread, cachedOwnerAgent(writer, deviceId, "row", agent), cachedOwnerAgent(writer, deviceId, "project-agents", agent)]
      .every((held) => digestNotRetired(held, incoming));
  });
}

function cursorValue(response, previous, bridgeIdentity, snapshot, records) {
  const head = snapshot ? response.watermark : response.head;
  return { incarnation: response.incarnation, cursor: response.cursor, head,
    appliedHead: response.continuation ? previous?.appliedHead ?? null : head,
    bridgeIdentity, bridge_epoch: response.bridge_epoch,
    snapshotRequired: !snapshot && (previous?.snapshotRequired === true || records.some((record) => record.operation === "invalidate")) };
}

export function createScopedDeviceSyncCache({ deviceId, scope, bridgeIdentity, active = () => true }) {
  const cursor = syncCursorAddress(deviceId, scope, "");
  async function capture() {
    const mutationEpoch = taskMutationEpoch(deviceId), pending = taskMutationsPending(deviceId), fence = pushFence();
    const addresses = (await cachedAddresses({ deviceId })).filter((address) => dataKinds.has(address.kind));
    const held = await readCanonicalCachedMany(addresses), rows = [...rawRows(addresses, held).values()];
    const projects = new Map();
    for (const projectId of knownProjects(rows)) projects.set(projectId,
      await captureTaskContentRead({ deviceId, projectId }, { revalidate: "summary" }));
    return { mutationEpoch, pending, fence, projects, projections: captureProjections(addresses, held),
      writes: new Map(addresses.map((address, index) => [addressKey(address), recordWriteOf(held[index])])) };
  }
  async function publish({ records = [], capture: captured, expectedCursor, response, snapshot = false, accept = () => true }) {
    const existing = await cachedAddresses({ deviceId });
    const rawAddresses = existing.filter((address) => address.kind === "sync-device-record");
    const previous = [...rawRows(rawAddresses, await readCanonicalCachedMany(rawAddresses)).values()];
    const addresses = distinct([cursor, ...targetAddresses(deviceId, records, previous, existing)]);
    const projectIds = knownProjects([...previous, ...records]), projects = new Map();
    for (const projectId of projectIds) {
      const read = captured.projects.get(projectId) || await captureTaskContentRead({ deviceId, projectId }, { revalidate: "summary" });
      const ids = [...previous, ...records].filter((record) => record.kind === "inbox_task" && sourceRow(record)?.project_id === projectId).map((record) => record.key);
      projects.set(projectId, { read, fragment: taskContentFragment({ deviceId, projectId }, { watchTaskIds: ids }) });
    }
    const fragments = [...projects.values()], allAddresses = [...addresses, ...fragments.flatMap(({ fragment }) => fragment.addresses)];
    return mergeCachedRecordsTogether(allAddresses, (all) => applyPublication(all, { addresses, projects, fragments,
      deviceId, records, response, snapshot, bridgeIdentity, cursor, accept, captured, active, expectedCursor }));
  }
  async function reset({ capture: captured, expectedCursor, accept = () => true } = {}) {
    return mergeCachedRecordsTogether([cursor], ([held]) => accept() && currentCapture(captured, deviceId, active)
      && (held?.value?.cursor ?? null) === (expectedCursor ?? null) ? [DELETE_CACHED_RECORD] : null);
  }
  return { capture, publish, reset, readCursor: async () => (await readCanonicalCached(cursor))?.value };
}
