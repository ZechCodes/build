// Assignment can outlive the row that opened its picker. Consult every list
// replica as well as detail: a pushed list may reblock before tasks.get lands.
import { cachedAddresses, readCachedMany } from "./localCache.js";
import { listAskedAt, taskAddress, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND } from "./trackerCache.js";

const listKinds = [TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TRACKER_TASKS_PAGE_KIND];
const taskInstant = (candidate) => Date.parse(candidate.task.updated_at) || 0;
const taskInRecord = (record, taskId) => record?.value?.task
  || record?.value?.tasks?.find((task) => task.id === taskId);
const newestFirst = (a, b) => taskInstant(b) - taskInstant(a) || b.read - a.read || b.order - a.order;

export async function readTaskAssignmentFacts(deviceId, projectId, taskId) {
  const addresses = (await cachedAddresses({ deviceId, entityId: projectId }))
    .filter((address) => listKinds.includes(address.kind));
  const records = await readCachedMany([taskAddress(deviceId, projectId, taskId), ...addresses]);
  const candidates = records.map((record) => ({
    task: taskInRecord(record, taskId),
    // Order list observations by when they were asked, since pages can land
    // late with the same task stamp. Details and legacy lists use write time.
    read: listAskedAt(record), order: record?.order || 0,
  }));
  // Missing legacy fields cannot erase known prerequisite facts.
  return candidates.filter((candidate) => candidate.task?.dependencies)
    .sort(newestFirst)[0]?.task || null;
}
