// The journal's task_head deliberately omits body bytes. This supplemental
// reader owns only those bytes: the journal retains header/timeline authority.
import { readCanonicalCachedMany, recordWriteOf } from "./localCache.js";
import { captureTaskContentRead, mergeTaskContentRecordsTogether, observeTaskContent,
  reconcileTaskContentWrite, TASK_CONTENT_CACHE, withTaskContentComponents } from "./taskContentAuthority.js";
import { taskMutationEpoch, taskMutationsPending } from "./taskMutationFence.js";
import { taskAddress } from "./trackerCache.js";
import { revisionAddress } from "./scopedTaskSyncRecords.js";

const bodyBasis = (revision) => JSON.stringify([revision?.incarnation, revision?.revision]);
const bodyCurrent = (detail, revision) => typeof detail?.task?.body === "string"
  && detail.stale !== true && detail.sync_body_basis === bodyBasis(revision);
const sameGeneration = (read, context) => ["storeGeneration", "scopeGeneration", "retirementEpoch"]
  .every((field) => read.ticket?.[field] && read.ticket[field] === context[field]);

function bodyTask(held, selected) {
  const task = withTaskContentComponents(selected, held, "summary");
  const previous = held[TASK_CONTENT_CACHE], next = selected[TASK_CONTENT_CACHE];
  // Preserve the actual body's proof, including its full-header identity.
  // A compact journal header never certifies full-body/header equivalence.
  task[TASK_CONTENT_CACHE] = { ...next, header: previous?.header,
    identities: { ...next.identities, header: previous?.identities?.header },
    retired: { ...next.retired, ...previous?.retired } };
  return task;
}

function supplement(held, answer, contentRead, context, address) {
  if (held.value.task.updated_at !== answer.task.updated_at) return null;
  const result = reconcileTaskContentWrite(held?.value?.task,
    observeTaskContent(answer.task, { contentRead, coverage: "full" }),
    { context, address, heldWritten: recordWriteOf(held) });
  if (!result.acceptedBody) return null;
  return { ...held.value, task: bodyTask(held.value.task, result.task), stale: false };
}

const canReadBody = (options, detail, revision) => options.active() && Boolean(detail?.value?.task)
  && !bodyCurrent(detail.value, revision?.value);
const validBodyAnswer = (answer, options) => answer?.task?.id === options.taskId
  && typeof answer.task.body === "string"
  && (!answer.task.project_id || answer.task.project_id === options.projectId);

function acceptsReply(options, contentRead, held, revision, baseline, context) {
  return options.active() && options.mutationEpoch === taskMutationEpoch(options.deviceId)
    && taskMutationsPending(options.deviceId) === 0 && Boolean(held?.value?.task)
    && contentRead.written === recordWriteOf(held) && recordWriteOf(revision) === recordWriteOf(baseline)
    && sameGeneration(contentRead, context);
}

async function readBody(options, address, revisionAddress) {
  const [detail, revision] = await readCanonicalCachedMany([address, revisionAddress]);
  if (!canReadBody(options, detail, revision)) return false;
  const contentRead = await captureTaskContentRead(address, { revalidate: "full" });
  if (!options.active() || options.mutationEpoch !== taskMutationEpoch(options.deviceId)) return false;
  const answer = await options.callRpc("tasks.get", { task_id: options.taskId });
  if (!validBodyAnswer(answer, options)) return false;
  return mergeTaskContentRecordsTogether(options, [address, revisionAddress], ([held, current], context) => {
    if (!acceptsReply(options, contentRead, held, current, revision, context)) return null;
    const value = supplement(held, answer, contentRead, context, address);
    if (!value) return null;
    return { values: [{ ...value, sync_body_basis: bodyBasis(current?.value) }, null] };
  });
}

export async function refreshScopedTaskBody({ deviceId, projectId, taskId, callRpc, active = () => true }) {
  if (!active() || taskMutationsPending(deviceId) > 0) return false;
  const options = { deviceId, projectId, taskId, callRpc, active, mutationEpoch: taskMutationEpoch(deviceId) };
  return readBody(options, taskAddress(deviceId, projectId, taskId), revisionAddress(options, taskId));
}
