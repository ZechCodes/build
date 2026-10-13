// Durable knowledge preserves facts across legacy writes. It participates in
// assignment arbitration alongside every replica and the opened picker task.
import { hasTaskDependencyFacts, readTaskDependencyKnowledge, taskDependencyKnowledgeAddress } from "./taskDependencyKnowledge.js";
import { cachedAddresses, readCachedMany } from "./localCache.js";
import { taskAddress, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND } from "./trackerCache.js";

const listKinds = [TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND];
const taskInstant = (task) => {
  const stamp = Date.parse(task.updated_at);
  return Number.isFinite(stamp) ? stamp : -Infinity;
};
const taskInRecord = (record, taskId) => record?.value?.task
  || record?.value?.tasks?.find((task) => task.id === taskId);
const knowledgeFacts = (knowledge) => ({ id: knowledge.task_id, updated_at: knowledge.updated_at, dependencies: knowledge.dependencies });

export async function readTaskAssignmentFacts(deviceId, projectId, taskId, opened = null) {
  const knowledge = await readTaskDependencyKnowledge(deviceId, projectId, taskId);
  const addresses = (await cachedAddresses({ deviceId, entityId: projectId }))
    .filter((address) => listKinds.includes(address.kind));
  // A capable write may establish or advance knowledge during discovery.
  // Snapshot it together with all retained replicas, even when it already exists.
  const [observed, ...records] = await readCachedMany([
    taskDependencyKnowledgeAddress(deviceId, projectId, taskId), taskAddress(deviceId, projectId, taskId), ...addresses,
  ]);
  const tasks = [...records.map((record) => taskInRecord(record, taskId)), knowledge && knowledgeFacts(knowledge)]
    .filter((task) => task?.id === taskId);
  return selectTaskAssignmentFacts({ knowledge: observed?.value, replicas: tasks, opened: opened?.id === taskId ? opened : null });
}

export function selectTaskAssignmentFacts({ knowledge = null, replicas = [], opened = null }) {
  const facts = [...replicas, opened, knowledge && knowledgeFacts(knowledge)].filter(hasTaskDependencyFacts);
  const latest = facts.reduce((stamp, task) => Math.max(stamp, taskInstant(task)), -Infinity);
  // Source read/write orders are incomparable. Discard only demonstrably
  // older task versions; ties and unknown stamps retain waiting evidence.
  const candidates = facts.filter((task) => !Number.isFinite(taskInstant(task)) || taskInstant(task) === latest);
  return candidates.find((task) => task.dependencies.blocked) || candidates[0] || null;
}
