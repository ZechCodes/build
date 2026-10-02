// Review records belong to the named project, so ordinary sync retains them.
// The server's version is authoritative across tabs; read order settles equal
// versions and errors without letting an old read erase newer metadata.
import { cachedSubKeys, mergeCachedAtomically, readCached } from "./localCache.js";
import { nextTaskRead } from "./taskReadOrder.js";
import { pushFence, removedSince } from "./pushFence.js";
import { readReviewSupport } from "./taskReviewSupport.js";
import { commandRefusalMessage } from "./commandRefusal.js";
import { trailingRead } from "./trailingRead.js";

export const REVIEW_KIND = "task-review";
export const reviewAddress = ({ deviceId, projectId, taskId }) => ({
  deviceId, entityId: projectId, kind: REVIEW_KIND, sub: taskId,
});
export const reviewFailure = (error) => commandRefusalMessage(error, "Update this device's bridge to use reviews.");
const versionOf = (review) => review?.version || 0;
const mayClear = (held, observedVersion, readOrder) => !held ||
  (versionOf(held.review) <= observedVersion && held.read_order <= readOrder);

export function writeReviewRecord(scope, review, readOrder, observedVersion = 0) {
  return mergeCachedAtomically(reviewAddress(scope), (held) => {
    if (!review) return mayClear(held, observedVersion, readOrder)
      ? { review: null, read_order: readOrder } : null;
    if (versionOf(held?.review) > versionOf(review)) return null;
    if (versionOf(held?.review) === versionOf(review) && held?.read_order > readOrder) return null;
    return { review, read_order: readOrder };
  });
}

async function keepFailure(scope, error, readOrder) {
  return mergeCachedAtomically(reviewAddress(scope), (held) => {
    if (held?.read_order > readOrder) return null;
    return { ...held, error: reviewFailure(error), read_order: readOrder };
  });
}

export function createTaskReviewRepository({ callRpc, generationOf = () => null, active = () => true, ...scope }) {
  const canKeep = (fence) => active() && !removedSince(reviewAddress(scope), fence);
  async function read() {
    if (!active() || !(await readReviewSupport(scope.deviceId)).get) return false;
    const readOrder = await nextTaskRead();
    const fence = pushFence();
    const observedVersion = versionOf((await readCached(reviewAddress(scope)))?.value?.review);
    try {
      const answer = await callRpc("tasks.review.get", { task_id: scope.taskId });
      if (!answer || !active()) return false;
      if (!removedSince(reviewAddress(scope), fence)) await writeReviewRecord(scope, answer.review, readOrder, observedVersion);
      return true;
    } catch (error) {
      if (canKeep(fence)) await keepFailure(scope, error, readOrder);
      return false;
    }
  }
  const refresh = trailingRead(read, { generationOf });
  return {
    refresh,
    async mutate(verb, params) {
      const support = await readReviewSupport(scope.deviceId);
      if (!support[verb]) throw new Error("Update this device's bridge to use reviews.");
      const readOrder = await nextTaskRead();
      const fence = pushFence();
      try {
        const answer = await callRpc(`tasks.review.${verb}`, { ...params, task_id: scope.taskId });
        if (!removedSince(reviewAddress(scope), fence)) await writeReviewRecord(scope, answer.review, readOrder);
      } catch (error) {
        if ((error.code || error.error_code) === "stale_version") await refresh();
        throw error;
      }
    },
  };
}

// A sync pass or task invalidation refreshes only reviews previously opened
// here. Bodies are addressed by immutable snapshot identity and stay put.
export async function refreshCachedReviews({ deviceId, projectId, callRpc, active = () => true }, taskIds = null) {
  if (!(await readReviewSupport(deviceId)).get) return;
  const held = await cachedSubKeys(deviceId, projectId, REVIEW_KIND);
  for (const taskId of held) {
    if (!active()) return;
    if (taskIds && !taskIds.includes(taskId)) continue;
    await createTaskReviewRepository({ deviceId, projectId, taskId, callRpc, active }).refresh();
  }
}
