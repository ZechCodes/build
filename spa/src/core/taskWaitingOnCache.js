// Successful task actions update every replica already used by task views.
// Lists and their per-row read fences move in the same cache transaction.
import { cachedAddresses, mergeCachedAtomically, mergeCachedRecordsTogether, readCached } from "./localCache.js";
import { nextTaskRead, readsAddress, withWritten } from "./taskReadOrder.js";
import {
  TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND,
  taskAddress, tasksAddress, preserveTaskFloors, preserveTaskReviewSummary,
} from "./trackerCache.js";
import { mergeTaskDependencyRecords } from "./taskDependencyKnowledge.js";
import { filterTasks, sortTasks } from "./trackerFilters.js";
import { assigneeKey } from "./trackerModel.js";

const listKinds = new Set([TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND]);
const newer = (held, incoming) => Date.parse(held?.updated_at) > Date.parse(incoming?.updated_at);

function detailWithResponse(held, answer) {
  if (newer(held?.task, answer.task)) return null;
  const timeline = Object.hasOwn(answer, "timeline") ? answer.timeline : held?.timeline;
  return preserveTaskFloors(held, { ...held, task: answer.task, timeline: timeline || [] });
}

function queryMatches(address, task) {
  if (address.kind === TRACKER_TASKS_KIND) return true;
  let params;
  try { params = JSON.parse(address.sub || "{}"); }
  catch { return false; }
  const filters = { ...params, assignee: typeof params.assignee === "object" ? assigneeKey(params.assignee) : params.assignee };
  return filterTasks([task], filters).length > 0;
}

function listWithResponse(address, held, task, read) {
  const rows = held?.tasks || [];
  const prior = rows.find((row) => row.id === task.id);
  if (newer(prior, task)) return null;
  const includes = queryMatches(address, task);
  if (!prior && (!includes || address.kind === TRACKER_TASKS_PAGE_KIND)) return null;
  const remaining = rows.filter((row) => row.id !== task.id);
  const next = preserveTaskReviewSummary(prior, task, { readOrder: read, allowMissing: true });
  return { columns: [], ...held, read_order: Math.max(held?.read_order || 0, read), tasks: sortTasks(includes ? [...remaining, next] : remaining) };
}

export async function cacheTaskWaitingOnResponse(deviceId, projectId, answer, { accept = () => true } = {}) {
  const task = answer?.task;
  if (!task?.id || !accept()) return false;
  const listed = (await cachedAddresses({ deviceId, entityId: projectId })).filter((address) => listKinds.has(address.kind));
  const catalogue = tasksAddress(deviceId, projectId);
  if (!listed.some((address) => address.kind === TRACKER_TASKS_KIND)) listed.push(catalogue);
  const addresses = [taskAddress(deviceId, projectId, task.id), ...listed, ...listed.map(readsAddress), taskWaitingOnGenerationAddress(deviceId, projectId), readsAddress(taskAddress(deviceId, projectId, task.id))];
  const read = await nextTaskRead();
  return mergeTaskDependencyRecords(addresses, [task], (records) => {
    if (!accept()) return null;
    const detail = detailWithResponse(records[0]?.value, answer);
    const lists = listed.map((address, index) => listWithResponse(address, records[index + 1]?.value, task, read));
    const notes = lists.map((list, index) => list ? withWritten(records[listed.length + index + 1]?.value, [task.id], read) : null);
    return [detail, ...lists, ...notes, detail ? nextReverseGeneration(records.at(-2)?.value) : null,
      detail ? withWritten(records.at(-1)?.value, [task.id], read) : null];
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
