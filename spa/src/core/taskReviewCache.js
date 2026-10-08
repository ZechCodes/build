// Review records belong to the named project, so ordinary sync retains them.
// The server's version is authoritative across tabs; read order settles equal
// versions and errors without letting an old read erase newer metadata.
import { cachedRecords, cachedSubKeys, mergeCachedAtomically, readCached } from "./localCache.js";
import { nextTaskRead } from "./taskReadOrder.js";
import { pushFence, removedSince } from "./pushFence.js";
import { canReviewOperation, readReviewSupport } from "./taskReviewSupport.js";
import { mergeReviewObservations } from "./taskReviewObservations.js";
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

const newerReview = (held, review, readOrder) => versionOf(review) > versionOf(held?.review) ||
  (versionOf(review) === versionOf(held?.review) && readOrder >= (held?.read_order || 0));
const precedesClear = (held, readOrder) => held?.review === null && held.read_order > readOrder;

function mergeReviewReply(held, answer, readOrder, observedVersion) {
  if (precedesClear(held, readOrder)) return null;
  if (!answer.review) return mayClear(held, observedVersion, readOrder)
    ? { review: null, read_order: readOrder } : null;
  const observations = mergeReviewObservations(held, answer.sync, readOrder);
  if (!newerReview(held, answer.review, readOrder)) return observations ? { ...held, ...observations } : null;
  // A later metadata-only read retains the last operation's additive result:
  // opening dispatch, publication failures and saved merge intents are facts.
  const next = { ...held, ...answer, ...observations, read_order: readOrder };
  if (!observations && held?.sync) next.sync = held.sync;
  delete next.error;
  return next;
}

export const writeReviewReply = (scope, answer, readOrder, observedVersion = 0) =>
  mergeCachedAtomically(reviewAddress(scope), (held) => mergeReviewReply(held, answer, readOrder, observedVersion));

export const writeReviewRecord = (scope, review, readOrder, observedVersion = 0) =>
  writeReviewReply(scope, { review }, readOrder, observedVersion);

async function keepFailure(scope, error, readOrder) {
  return mergeCachedAtomically(reviewAddress(scope), (held) => {
    if (held?.read_order > readOrder) return null;
    return { ...held, error: reviewFailure(error), read_order: readOrder };
  });
}

export function createTaskReviewRepository({ callRpc, generationOf = () => null, active = () => true, ...scope }) {
  const canKeep = (fence) => active() && !removedSince(reviewAddress(scope), fence);
  const canRead = async () => Boolean(scope.taskId) && active() && (await readReviewSupport(scope.deviceId)).get;
  const mutationScope = (answer) => ({ ...scope, taskId: answer?.review?.task_id || scope.taskId });
  async function keepMutation(answer, readOrder, fence) {
    if (!answer || !active()) return;
    const destination = mutationScope(answer);
    if (destination.taskId && !removedSince(reviewAddress(destination), fence)) {
      await writeReviewReply(destination, answer, readOrder);
    }
  }
  async function requireOperation(verb) {
    const support = await readReviewSupport(scope.deviceId);
    const held = (await readCached(reviewAddress(scope)))?.value?.review;
    if (!active() || !canReviewOperation(support, verb, held)) throw new Error("Update this device's bridge to use reviews.");
  }
  async function read() {
    if (!(await canRead())) return false;
    const readOrder = await nextTaskRead();
    const fence = pushFence();
    const observedVersion = versionOf((await readCached(reviewAddress(scope)))?.value?.review);
    try {
      const answer = await callRpc("tasks.review.get", { task_id: scope.taskId });
      if (!answer || !active()) return false;
      if (!removedSince(reviewAddress(scope), fence)) await writeReviewReply(scope, answer, readOrder, observedVersion);
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
      await requireOperation(verb);
      const readOrder = await nextTaskRead();
      const fence = pushFence();
      try {
        const request = verb === "open" ? params : { ...params, task_id: scope.taskId };
        const answer = await callRpc(`tasks.review.${verb}`, request);
        await keepMutation(answer, readOrder, fence);
      } catch (error) {
        if ((error.code || error.error_code) === "stale_version") await refresh();
        throw error;
      }
    },
  };
}

// Reconnect and invalidations discover review metadata through summaries too;
// none of these reads publishes branches or replays saved action requests.
export async function refreshCachedReviews({ deviceId, projectId, callRpc, active = () => true, discoveredTaskIds = [] }, taskIds = null) {
  const support = await readReviewSupport(deviceId);
  if (!support.get) return;
  const held = await cachedSubKeys(deviceId, projectId, REVIEW_KIND);
  const discovered = support.pullRequests ? discoveredTaskIds : [];
  for (const taskId of new Set([...held, ...discovered])) {
    if (!active()) return;
    if (taskIds && !taskIds.includes(taskId)) continue;
    await createTaskReviewRepository({ deviceId, projectId, taskId, callRpc, active }).refresh();
  }
}

async function workspaceReviewHint({ deviceId, workspaceId, activeReview }) {
  if (activeReview !== undefined) return activeReview;
  const workspaces = (await readCached({ deviceId, entityId: "", kind: "workspaces" }))?.value || [];
  return workspaces.find((row) => (row.workspace_id || row.id) === workspaceId)?.active_review;
}

const matchingHint = (hint, workspaceId) => hint?.task_id && hint.workspace_id === workspaceId;

/** Workspace controls can warm metadata before the task page has ever opened. */
export async function refreshWorkspaceReview(scope) {
  const support = await readReviewSupport(scope.deviceId);
  if (!support.pullRequests || !support.get) return false;
  const hint = await workspaceReviewHint(scope);
  if (!matchingHint(hint, scope.workspaceId)) return false;
  return createTaskReviewRepository({ ...scope, taskId: hint.task_id }).refresh();
}

/** Read only local replicas. A just-created PR may precede the workspace-list
 * read, so its full cached opening result can resolve that workspace as well. */
export async function readWorkspaceReview(scope) {
  const hint = await workspaceReviewHint(scope);
  if (hint !== undefined) {
    if (!matchingHint(hint, scope.workspaceId)) return null;
    const held = (await readCached(reviewAddress({ ...scope, taskId: hint.task_id })))?.value;
    return held?.review?.workspace_id === scope.workspaceId ? held : null;
  }
  const records = await cachedRecords({ deviceId: scope.deviceId, entityId: scope.projectId, kind: REVIEW_KIND });
  return records.filter(({ value }) => value?.review?.workspace_id === scope.workspaceId)
    .sort((left, right) => right.value.read_order - left.value.read_order)[0]?.value || null;
}
