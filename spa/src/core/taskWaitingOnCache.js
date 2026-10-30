// Successful task actions update every replica already used by task views.
// Lists and their per-row read fences move in the same cache transaction.
import { cachedAddresses, mergeCachedAtomically, mergeCachedRecordsTogether, readCached, recordWriteOf } from "./localCache.js";
import { nextTaskRead, readsAddress, withWritten } from "./taskReadOrder.js";
import {
  TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND,
  taskAddress, tasksAddress, preserveTaskFloors, preserveTaskReviewSummary,
} from "./trackerCache.js";
import { filterTasks, sortTasks } from "./trackerFilters.js";
import { assigneeKey } from "./trackerModel.js";
import { preserveTaskActionMarks, summaryTaskMutation, withTaskWatchLineage } from "./taskSummaryCache.js";

import { observeTaskContent, reconcileTaskContent, withTaskContentComponents } from "./taskContentAuthority.js";

const listKinds = new Set([TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND]);
const newer = (held, incoming) => Date.parse(held?.updated_at) > Date.parse(incoming?.updated_at);

function actionTask(held, incoming, read, projection, options) {
  const selected = reconcileTaskContent(held, observeTaskContent(incoming, { contentRead: options.contentRead, coverage: "full" }), options);
  const ordinary = preserveTaskReviewSummary(held, incoming, { readOrder: read, keepHeld: newer(held, incoming),
    allowMissing: !newer(held, incoming), baseRow: selected.task || held });
  const task = withTaskWatchLineage({ ...ordinary, read_through: projection.read_through,
    dismissed_through: projection.dismissed_through }, projection);
  return { task, acceptedBody: selected.acceptedBody };
}

function detailWithResponse(held, answer, read, projection, options) {
  const before = held?.task;
  const selected = actionTask(before, answer.task, read, projection, options);
  if (!selected.task?.id || !held && typeof selected.task.body !== "string") return null;
  const timeline = selected.acceptedBody && Object.hasOwn(answer, "timeline") ? answer.timeline : held?.timeline;
  const detail = preserveTaskFloors(held, { ...held, task: selected.task, timeline: timeline || [] });
  const marks = actionDetailMarks(before, detail.task, selected.task);
  return { ...detail, task: withTaskContentComponents(marks, selected.task) };
}

function actionDetailMarks(held, detail, task) {
  const raw = { ...detail };
  for (const field of ["read_through", "watch_started_after", "dismissed_through"]) {
    if (Object.hasOwn(task, field)) raw[field] = task[field];
    else delete raw[field];
  }
  return preserveTaskActionMarks(held, raw, { watchAuthority: true });
}

function queryMatches(address, task) {
  if (address.kind === TRACKER_TASKS_KIND) return true;
  let params;
  try { params = JSON.parse(address.sub || "{}"); }
  catch { return false; }
  const filters = { ...params, assignee: typeof params.assignee === "object" ? assigneeKey(params.assignee) : params.assignee };
  return filterTasks([task], filters).length > 0;
}

function listWithResponse(address, held, task, read, projection, options) {
  const rows = held?.tasks || [];
  const prior = rows.find((row) => row.id === task.id);
  const incoming = preserveTaskActionMarks(prior, actionTask(prior, task, read, projection, options).task, { watchAuthority: true });
  const includes = queryMatches(address, incoming);
  if (!prior && (!includes || address.kind === TRACKER_TASKS_PAGE_KIND)) return null;
  const remaining = rows.filter((row) => row.id !== task.id);
  const next = preserveTaskReviewSummary(prior, incoming, { readOrder: read, allowMissing: true });
  return { columns: [], ...held, read_order: Math.max(held?.read_order || 0, read), tasks: sortTasks(includes ? [...remaining, next] : remaining) };
}

function validateTaskActionOrder(issuedRead) {
  if (issuedRead !== undefined && (!Number.isSafeInteger(issuedRead) || issuedRead <= 0)) throw new Error("Invalid task action dispatch order");
}

const olderAction = (note, issuedRead) => issuedRead === undefined
  ? Boolean(note?.action_read) : (note?.action_read || 0) > issuedRead;
const actionResponseNote = (held, taskId, read, actionRead, detail) => ({
  ...(detail ? withWritten(held, [taskId], read) : held), ...(actionRead === undefined ? {} : { action_read: actionRead }),
});

function normalizedAction(summary, shared, detail) {
  const normalized = summary.reduce(shared, { headerBarrier: detail?.value?.task });
  return normalized?.[0]?.task?.id ? normalized : null;
}

export async function cacheTaskWaitingOnResponse(deviceId, projectId, answer, { accept = () => true, issuedRead, contentRead } = {}) {
  const task = answer?.task;
  if (!task?.id || !accept()) return false;
  validateTaskActionOrder(issuedRead);
  const listed = (await cachedAddresses({ deviceId, entityId: projectId })).filter((address) => listKinds.has(address.kind));
  const catalogue = tasksAddress(deviceId, projectId);
  if (!listed.some((address) => address.kind === TRACKER_TASKS_KIND)) listed.push(catalogue);
  const addresses = [taskAddress(deviceId, projectId, task.id), ...listed, ...listed.map(readsAddress), taskWaitingOnGenerationAddress(deviceId, projectId), readsAddress(taskAddress(deviceId, projectId, task.id))];
  const read = await nextTaskRead();
  const confirmed = issuedRead !== undefined;
  const summary = summaryTaskMutation({ deviceId, projectId }, task, read, { accept, fullTask: true, confirmedAction: confirmed, issuedRead, contentRead });
  return mergeCachedRecordsTogether([...addresses, ...summary.addresses], (allRecords) => {
    if (!accept()) return null;
    const records = allRecords.slice(0, addresses.length);
    const heldNote = records.at(-1)?.value;
    if (olderAction(heldNote, issuedRead)) return null;
    const shared = allRecords.slice(addresses.length);
    const normalized = normalizedAction(summary, shared, records[0]);
    if (!normalized) return null;
    const projection = normalized[0].task;
    const context = summary.context(shared);
    const detail = detailWithResponse(records[0]?.value, answer, read, projection, { context, contentRead,
      address: addresses[0], heldWritten: recordWriteOf(records[0]), headerBarrier: projection });
    const lists = listed.map((address, index) => listWithResponse(address, records[index + 1]?.value, task, read, projection,
      { context, contentRead, address, heldWritten: recordWriteOf(records[index + 1]), headerBarrier: projection }));
    const notes = lists.map((list, index) => list ? withWritten(records[listed.length + index + 1]?.value, [task.id], read) : null);
    return [detail, ...lists, ...notes, detail ? nextReverseGeneration(records.at(-2)?.value) : null,
      actionResponseNote(heldNote, task.id, read, issuedRead, detail), ...normalized];
  });
}

export const taskWaitingOnBlocksAddress = (deviceId, projectId, taskId) => ({
  deviceId, entityId: projectId, kind: "task-waiting-on-blocks", sub: taskId,
});

// A project fence also covers reverse endpoints absent from a task's preview,
// and first pages still in flight when a push or forward mutation arrives.
export const taskWaitingOnGenerationAddress = (deviceId, projectId) => ({
  deviceId, entityId: projectId, kind: "task-waiting-on-generation",
});
const nextReverseGeneration = (held) => ({ generation: (held?.generation || 0) + 1 });

export function invalidateTaskWaitingOnBlocks(deviceId, projectId) {
  return mergeCachedAtomically(taskWaitingOnGenerationAddress(deviceId, projectId), nextReverseGeneration);
}

const dependencySignature = (detail) => JSON.stringify(detail?.task?.dependencies || null);

export async function captureTaskWaitingOnBlocksRead(deviceId, projectId, taskId) {
  const [detail, fence] = await Promise.all([
    readCached(taskAddress(deviceId, projectId, taskId)),
    readCached(taskWaitingOnGenerationAddress(deviceId, projectId)),
  ]);
  return { detailSignature: dependencySignature(detail?.value), readOrder: await nextTaskRead(), reverseGeneration: fence?.value?.generation || 0 };
}

export async function readTaskWaitingOnBlocks(deviceId, projectId, taskId) {
  const [pages, detail, fence] = await Promise.all([
    readCached(taskWaitingOnBlocksAddress(deviceId, projectId, taskId)),
    readCached(taskAddress(deviceId, projectId, taskId)),
    readCached(taskWaitingOnGenerationAddress(deviceId, projectId)),
  ]);
  const held = pages?.value;
  if (!held || held.reverse_generation !== (fence?.value?.generation || 0) || held.detail_signature !== dependencySignature(detail?.value)) return null;
  return { blocks: held.blocks, next_cursor: held.next_cursor };
}

export function cacheTaskWaitingOnBlocks(deviceId, projectId, taskId, answer, options = {}) {
  const addresses = [taskWaitingOnBlocksAddress(deviceId, projectId, taskId), taskAddress(deviceId, projectId, taskId), taskWaitingOnGenerationAddress(deviceId, projectId)];
  return mergeCachedRecordsTogether(addresses, ([pages, detail, fence]) => reverseWithResponse(pages?.value, detail?.value, answer, options, fence?.value?.generation || 0));
}

function acceptsReverseRead(held, signature, { detailSignature, readOrder = 0, accept = () => true }) {
  return accept() && (detailSignature === undefined || detailSignature === signature)
    && (!readOrder || readOrder >= (held?.read_order || 0));
}

function reverseWithResponse(held, detail, answer, options, generation) {
  const signature = dependencySignature(detail);
  if (options.reverseGeneration !== undefined && options.reverseGeneration !== generation) return null;
  if (!acceptsReverseRead(held, signature, options)) return null;
  const base = reversePageBase(held, detail, signature, generation);
  if (options.cursor && base.next_cursor !== options.cursor) return null;
  const blocks = mergeReverseBlocks(options.cursor ? base.blocks : [], answer);
  return [{ ...reverseRecord(blocks, answer, signature, held, options.readOrder), reverse_generation: generation }, null, null];
}

function mergeReverseBlocks(prior, answer) {
  const blocks = new Map(prior.map((row) => [row.task_id, row]));
  for (const row of answer?.blocks || []) blocks.set(row.task_id, row);
  return [...blocks.values()];
}

function reverseRecord(blocks, answer, signature, held, readOrder) {
  return { blocks, next_cursor: answer?.next_cursor || null,
    detail_signature: signature, read_order: Math.max(readOrder || 0, held?.read_order || 0) };
}

function reversePageBase(held, detail, signature, generation) {
  if (held?.detail_signature === signature && held.reverse_generation === generation) return held;
  const dependencies = detail?.task?.dependencies || {};
  return { blocks: dependencies.blocks || [], next_cursor: dependencies.blocks_next_cursor || null };
}
