// Request-review task choices keep the project catalogue separate from their
// bounded query pages. Every option is read back from cache after a pull.
import { cachedAddresses, readCachedMany } from "./localCache.js";
import { TRACKER_TASK_KIND, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, readTasksQueryRecord, writeTasksQueryRecord } from "./trackerCache.js";
import { nextTaskRead } from "./taskReadOrder.js";

const listKinds = new Set([TRACKER_TASK_KIND, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, "task-review"]);
const eligible = (task, projectId) => task.state === "open" && !task.review_summary && (!task.project_id || task.project_id === projectId);
const newest = (held, next) => !held || (Date.parse(next.updated_at) || 0) >= (Date.parse(held.updated_at) || 0);
export const reviewTaskQuery = (scope) => ({ project_id: scope.projectId, summary: true, state: "open" });
const linkedQuery = (scope) => ({ ...reviewTaskQuery(scope), linked_workspace_id: scope.workspaceId });

export async function readReviewTaskChoices(scope) {
  const addresses = (await cachedAddresses({ deviceId: scope.deviceId, entityId: scope.projectId })).filter((address) => listKinds.has(address.kind));
  const records = await readCachedMany(addresses);
  const tasks = new Map();
  for (const record of records.filter(Boolean).sort((a, b) => a.at - b.at)) {
    const rows = record.value?.task ? [record.value.task] : record.value?.tasks || [];
    for (const task of rows) if (newest(tasks.get(task.id), task)) tasks.set(task.id, task);
  }
  const reviews = new Set(records.filter((record) => record?.value?.review).map((record) => record.value.review.task_id));
  return [...tasks.values()].filter((task) => !reviews.has(task.id) && eligible(task, scope.projectId)).sort((a, b) => b.number - a.number);
}

export function preselectedReviewTask(context, tasks) {
  const workspaceId = context.workspace?.workspace_id || context.workspace?.id;
  return tasks.find((task) => task.links?.workspace_ids?.includes(workspaceId) || task.matched_memberships?.linked_workspace_id === workspaceId)?.id || "";
}

export async function readReviewTaskPage(scope) {
  return readTasksQueryRecord(scope.deviceId, scope.projectId, reviewTaskQuery(scope));
}

async function readTaskChoicesPage(scope, params) {
  const { contextFor, whenGreeted } = await import("./deviceContexts.js");
  const context = contextFor(scope.deviceId);
  if (!context) return scope.callRpc("tasks.list", params);
  const request = await whenGreeted(context, () => {
    const support = context.adapter?.capabilities?.reviews;
    if (!support?.taskChoices || params.linked_workspace_id && !support.linkedTaskChoices) throw new Error("Update the bridge to choose review tasks.");
    return scope.callRpc("tasks.list", params);
  });
  if (!request) throw new Error("This machine is unavailable.");
  return request.sent;
}

async function pullChoicePage(scope, params, cursor, active) {
  const read = await nextTaskRead();
  const answer = await readTaskChoicesPage(scope, { ...params, limit: 100, ...(cursor ? { cursor } : {}) });
  if (!active()) return null;
  const prior = cursor ? await readTasksQueryRecord(scope.deviceId, scope.projectId, params) : null;
  const tasks = new Map((prior?.tasks || []).map((task) => [task.id, task]));
  for (const task of answer.tasks || []) tasks.set(task.id, task);
  await writeTasksQueryRecord(scope.deviceId, scope.projectId, params, { tasks: [...tasks.values()], next_cursor: answer.next_cursor || null, read_order: read });
  return readTasksQueryRecord(scope.deviceId, scope.projectId, params);
}

async function refreshLinkedChoices(scope, active, changed) {
  let cursor = null;
  do {
    const page = await pullChoicePage(scope, linkedQuery(scope), cursor, active);
    if (!page) return;
    await changed();
    const next = page.next_cursor;
    if (next === cursor) break;
    cursor = next;
  } while (cursor && active());
}

export async function refreshReviewTaskChoices(scope, support, active, changed, cursor = null) {
  if (!support.taskChoices) return;
  if (!cursor && support.linkedTaskChoices) await refreshLinkedChoices(scope, active, changed);
  if (!active()) return;
  await pullChoicePage(scope, reviewTaskQuery(scope), cursor, active);
  if (active()) await changed();
}
