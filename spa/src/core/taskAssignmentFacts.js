// Assignment reads durable dependency knowledge. Pre-upgrade caches without
// that record retain conservative arbitration of replicas and the opened row.
import { readTaskDependencyKnowledge } from "./taskDependencyKnowledge.js";
import { cachedAddresses, readCachedMany } from "./localCache.js";
import { taskAddress, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND } from "./trackerCache.js";

const listKinds = [TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND];
const taskInstant = (task) => {
  const stamp = Date.parse(task.updated_at);
  return Number.isFinite(stamp) ? stamp : -Infinity;
};
const taskInRecord = (record, taskId) => record?.value?.task
  || record?.value?.tasks?.find((task) => task.id === taskId);
const hasDependencyFacts = (task) => typeof task.dependencies?.blocked === "boolean";

export async function readTaskAssignmentFacts(deviceId, projectId, taskId, opened = null) {
  const knowledge = await readTaskDependencyKnowledge(deviceId, projectId, taskId);
  if (knowledge) return { id: knowledge.task_id, updated_at: knowledge.updated_at, dependencies: knowledge.dependencies };
  const addresses = (await cachedAddresses({ deviceId, entityId: projectId }))
    .filter((address) => listKinds.includes(address.kind));
  const records = await readCachedMany([taskAddress(deviceId, projectId, taskId), ...addresses]);
  const tasks = [...records.map((record) => taskInRecord(record, taskId)), opened]
    .filter((task) => task?.id === taskId);
  return selectAssignmentFacts(tasks);
}

function selectAssignmentFacts(tasks) {
  const known = tasks.filter(hasDependencyFacts);
  const waiting = known.filter((task) => task.dependencies.blocked);
  // A legacy refresh knows no readiness. While any source is unknown,
  // no ready version can erase another source's known waiting evidence.
  const facts = tasks.some((task) => !hasDependencyFacts(task)) && waiting.length ? waiting : known;
  const latest = facts.reduce((stamp, task) => Math.max(stamp, taskInstant(task)), -Infinity);
  // Source read/write orders are incomparable. Discard only demonstrably
  // older task versions; ties and unknown stamps retain waiting evidence.
  const candidates = facts.filter((task) => !Number.isFinite(taskInstant(task)) || taskInstant(task) === latest);
  return candidates.find((task) => task.dependencies.blocked) || candidates[0] || null;
}
