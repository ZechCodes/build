// Successful task actions update every replica already used by task views.
// Lists and their per-row read fences move in the same cache transaction.
import { cachedAddresses, mergeCachedAtomically, mergeCachedRecordsTogether, readCached } from "./localCache.js";
import { nextTaskRead, readsAddress, withWritten } from "./taskReadOrder.js";
import {
  TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND,
  taskAddress, tasksAddress, preserveTaskFloors, preserveTaskReviewSummary,
} from "./trackerCache.js";

const listKinds = new Set([TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND]);
const newer = (held, incoming) => Date.parse(held?.updated_at) > Date.parse(incoming?.updated_at);

function detailWithResponse(held, answer) {
  if (newer(held?.task, answer.task)) return null;
  const timeline = Object.hasOwn(answer, "timeline") ? answer.timeline : held?.timeline;
  return preserveTaskFloors(held, { ...held, task: answer.task, timeline: timeline || [] });
}

function listWithResponse(held, task, read) {
  if (!held?.tasks?.some((row) => row.id === task.id)) return null;
  return { ...held, read_order: Math.max(held.read_order || 0, read), tasks: held.tasks.map((row) => {
    if (row.id !== task.id || newer(row, task)) return row;
    return preserveTaskReviewSummary(row, task, { readOrder: read, allowMissing: true });
  }) };
}

export async function cacheTaskWaitingOnResponse(deviceId, projectId, answer, { accept = () => true } = {}) {
  const task = answer?.task;
  if (!task?.id || !accept()) return false;
  const listed = (await cachedAddresses({ deviceId, entityId: projectId })).filter((address) => listKinds.has(address.kind));
  const catalogue = tasksAddress(deviceId, projectId);
  if (!listed.some((address) => address.kind === TRACKER_TASKS_KIND)) listed.push(catalogue);
  const addresses = [taskAddress(deviceId, projectId, task.id), ...listed, ...listed.map(readsAddress)];
  const read = await nextTaskRead();
  return mergeCachedRecordsTogether(addresses, (records) => {
    if (!accept()) return null;
    const detail = detailWithResponse(records[0]?.value, answer);
    const lists = listed.map((_address, index) => listWithResponse(records[index + 1]?.value, task, read));
    const notes = lists.map((list, index) => list ? withWritten(records[listed.length + index + 1]?.value, [task.id], read) : null);
    return [detail, ...lists, ...notes];
  });
}

export const taskWaitingOnBlocksAddress = (deviceId, projectId, taskId) => ({
  deviceId, entityId: projectId, kind: "task-waiting-on-blocks", sub: taskId,
});

export async function readTaskWaitingOnBlocks(deviceId, projectId, taskId) {
  return (await readCached(taskWaitingOnBlocksAddress(deviceId, projectId, taskId)))?.value || null;
}

export function cacheTaskWaitingOnBlocks(deviceId, projectId, taskId, answer, { cursor, accept = () => true } = {}) {
  return mergeCachedAtomically(taskWaitingOnBlocksAddress(deviceId, projectId, taskId), (held) => {
    if (!accept()) return null;
    // A continuation belongs to the first page's chain. A fresh first page
    // that overtook it cannot have the old continuation appended to it.
    if (cursor && held?.next_cursor !== cursor) return null;
    const blocks = new Map((cursor ? held?.blocks || [] : []).map((row) => [row.task_id, row]));
    for (const row of answer?.blocks || []) blocks.set(row.task_id, row);
    return { blocks: [...blocks.values()], next_cursor: answer?.next_cursor || null };
  });
}
