// Assignment can outlive the row that opened its picker. Consult every list
// replica as well as detail: a pushed list may reblock before tasks.get lands.
import { cachedAddresses, readCachedMany } from "./localCache.js";
import { taskAddress, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND } from "./trackerCache.js";

const listKinds = [TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND];
const taskInstant = (task) => {
  const stamp = Date.parse(task.updated_at);
  return Number.isFinite(stamp) ? stamp : -Infinity;
};
const taskInRecord = (record, taskId) => record?.value?.task
  || record?.value?.tasks?.find((task) => task.id === taskId);

export async function readTaskAssignmentFacts(deviceId, projectId, taskId) {
  const addresses = (await cachedAddresses({ deviceId, entityId: projectId }))
    .filter((address) => listKinds.includes(address.kind));
  const records = await readCachedMany([taskAddress(deviceId, projectId, taskId), ...addresses]);
  // Missing legacy fields cannot erase known prerequisite facts.
  const tasks = records.map((record) => taskInRecord(record, taskId))
    .filter((task) => task?.dependencies);
  const latest = tasks.reduce((stamp, task) => Math.max(stamp, taskInstant(task)), -Infinity);
  // Source read/write orders are incomparable. Discard only demonstrably
  // older task versions; ties and unknown stamps retain waiting evidence.
  const candidates = tasks.filter((task) => !Number.isFinite(taskInstant(task)) || taskInstant(task) === latest);
  return candidates.find((task) => task.dependencies.blocked) || candidates[0] || null;
}
