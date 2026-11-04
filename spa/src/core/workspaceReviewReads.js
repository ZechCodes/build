// A visible Changes header owns one review's metadata and one task scope.
// Cache paints only select demand; the existing repositories publish reads.
import { bridgeCapabilities, onBridgeGreeted } from "./changeEvents.js";
import { captureDeviceGreeting, contextFor } from "./deviceContexts.js";
import { registerScopedSync, taskSyncScope } from "./scopedSyncRegistry.js";
import { lazyViewReadsEnabled, scopedSyncOwns, wakeScopedSync } from "./scopedSyncRuntime.js";
import { createTaskReviewRepository } from "./taskReviewCache.js";
import { requestTaskDetail, taskDetailRecord } from "./taskTimelinePages.js";
import { captureTaskContentRead } from "./taskContentAuthority.js";
import { captureTaskDetailRead, taskAddress, writeTaskRecord } from "./trackerCache.js";

function selectedReview(context, workspaceId) {
  const review = context.held?.review || {};
  const hint = context.workspace?.active_review;
  const summary = matchingReviewHint(hint, workspaceId);
  return { taskId: review.task_id || summary.task_id,
    version: Math.max(review.version || 0, summary.version || 0) };
}
const matchingReviewHint = (hint, workspaceId) => hint?.workspace_id === workspaceId ? hint : {};

async function readSelectedTask(options) {
  if (!options.active()) return;
  const scope = taskSyncScope(options.taskId);
  if (bridgeCapabilities(options.deviceId)?.tasks?.timelinePaged === true && scopedSyncOwns(options.deviceId, scope)) {
    await wakeScopedSync(options.deviceId, (candidate) => candidate.kind === "task" && candidate.task_id === options.taskId);
    return;
  }
  const address = taskAddress(options.deviceId, options.projectId, options.taskId);
  const [detailBasis, contentRead] = await Promise.all([
    captureTaskDetailRead(options.deviceId, options.projectId, options.taskId),
    captureTaskContentRead(address, { revalidate: "full" }),
  ]);
  const request = await requestTaskDetail(options);
  if (!request?.accept()) return;
  await writeTaskRecord(options.deviceId, options.projectId, options.taskId, taskDetailRecord(request.answer), {
    detailBasis, contentRead, accept: request.accept,
  });
}

export function createWorkspaceReviewReads({ active = () => true, ...options }) {
  let selected = null, disposed = false;
  const current = (owner) => !disposed && active() && selected === owner && lazyViewReadsEnabled(options.deviceId);
  const retire = () => { selected?.release(); selected = null; };

  function readOptions(owner) {
    const context = contextFor(options.deviceId);
    const greeting = context && captureDeviceGreeting(context);
    return { ...options, taskId: owner.taskId, active: () => current(owner)
      && contextFor(options.deviceId) === context && (!context || Boolean(greeting?.current())) };
  }

  function refresh(owner) {
    if (!current(owner)) return;
    const generation = contextFor(options.deviceId)?.greeted;
    if (owner.pending?.generation === generation && owner.pending) {
      owner.pending.again = true; return owner.pending.promise;
    }
    const request = readOptions(owner);
    const repository = createTaskReviewRepository(request);
    const attempt = { generation, again: false };
    owner.pending = attempt;
    attempt.promise = Promise.allSettled([repository.refresh(), readSelectedTask(request)])
      .finally(() => finishRead(owner, attempt));
    return attempt.promise;
  }

  function finishRead(owner, attempt) {
    if (owner.pending !== attempt) return;
    owner.pending = null;
    if (attempt.again && current(owner)) void refresh(owner);
  }

  function select(context) {
    if (disposed || !active() || !lazyViewReadsEnabled(options.deviceId)) { retire(); return; }
    const next = selectedReview(context, options.workspaceId);
    if (!next.taskId) { retire(); return; }
    if (selected?.taskId === next.taskId) {
      if (next.version > selected.version) { selected.version = next.version; void refresh(selected); }
      return;
    }
    retire();
    const scope = taskSyncScope(next.taskId);
    selected = { ...next, release: registerScopedSync({ deviceId: options.deviceId, projectId: options.projectId, scope }) };
    void refresh(selected);
  }
  const stopGreeting = onBridgeGreeted((deviceId) => {
    if (deviceId === options.deviceId && selected) void refresh(selected);
  });
  return { select, suspend: retire, dispose() { disposed = true; retire(); stopGreeting(); } };
}
