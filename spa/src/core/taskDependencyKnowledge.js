// Dependency knowledge survives replacement of ordinary task replicas by an
// older wire. It shares the task's device/project scope and cache lifetime.
import { mergeCachedRecordsTogether, readCached } from "./localCache.js";

export const taskDependencyKnowledgeAddress = (deviceId, projectId, taskId) => ({
  deviceId, entityId: String(projectId || ""), kind: "tracker-task-dependencies", sub: String(taskId || ""),
});
export function hasTaskDependencyFacts(task) {
  const summary = task?.dependencies;
  return summary?.blocked === true || (summary?.blocked === false
    && Array.isArray(summary.waiting_on) && summary.waiting_on.length === 0);
}
const instant = (record) => Date.parse(record?.updated_at);

function nextKnowledge(held, task) {
  if (!hasTaskDependencyFacts(task)) return null;
  const next = { task_id: task.id, updated_at: task.updated_at, dependencies: task.dependencies };
  if (!held) return next;
  const prior = instant(held), incoming = instant(next);
  if (Number.isFinite(prior) && Number.isFinite(incoming)) {
    if (incoming > prior) return next;
    if (incoming < prior) return null;
  }
  // Only a demonstrably newer capable version can erase waiting evidence.
  // At tied or unknown versions, waiting wins without borrowing read clocks.
  return !held.dependencies.blocked && next.dependencies.blocked ? next : null;
}

export async function readTaskDependencyKnowledge(deviceId, projectId, taskId) {
  return (await readCached(taskDependencyKnowledgeAddress(deviceId, projectId, taskId)))?.value || null;
}

// Replica admission and dependency observation commit together. Rejected
// writes cannot establish knowledge, and views announce only committed data.
export function mergeTaskDependencyRecords(addresses, tasks, merge) {
  const observations = [...new Map(tasks.filter((task) => task?.id && hasTaskDependencyFacts(task)).map((task) => [task.id, task])).values()];
  const { deviceId, entityId } = addresses[0];
  const knowledgeAddresses = observations.map((task) => taskDependencyKnowledgeAddress(deviceId, entityId, task.id));
  return mergeCachedRecordsTogether([...addresses, ...knowledgeAddresses], (records) => {
    const next = merge(records.slice(0, addresses.length));
    if (!next) return null;
    return [...next, ...observations.map((task, index) => nextKnowledge(records[addresses.length + index]?.value, task))];
  });
}
