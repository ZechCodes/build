// Saved Git comparisons are immutable identities. Lists and bodies stay in the
// project cache; the reader only announces values it has read back from there.
import { cachedRecords, mergeCachedAtomically, readCached, subscribeCache } from "./localCache.js";
import { canReviewRead, readReviewSupport, reviewMutationRpc } from "./taskReviewSupport.js";
import { nextTaskRead } from "./taskReadOrder.js";
import { pushFence, removedSince } from "./pushFence.js";
import { trailingRead } from "./trailingRead.js";

const baseSub = ({ taskId, snapshot, directory }) => [taskId, snapshot.id, directory.id];
const address = (scope, kind, sub) => ({ deviceId: scope.deviceId, entityId: scope.projectId, kind, sub: JSON.stringify(sub) });
const modeOf = (comparison) => comparison?.mode || "changes";
const selectionOf = (comparison) => modeOf(comparison) === "commit" ? comparison.commit : comparison.from_snapshot_id;
const errorMessage = (error) => String(error?.message || "Source unavailable");
const commitOid = (row) => row.sha || row.oid;
const commitsParams = (scope, cursor) => ({ ...reviewDiffParams(scope), mode: "commits", ...(cursor ? { cursor } : {}) });

export const reviewCommitsAddress = (scope) => address(scope, "task-review-commits", baseSub(scope));

/** Preserve older whole-range addresses so saved viewed marks and patches work. */
export function reviewDiffAddress(scope, comparison = {}, path = null) {
  const mode = modeOf(comparison);
  const sub = baseSub(scope);
  if (mode !== "changes") sub.push(selectionOf(comparison));
  const prefix = mode === "changes" ? "task-review" : `task-review-${mode}`;
  if (path !== null) sub.push(path, "old..new");
  return address(scope, `${prefix}-${path === null ? "changes" : "patch"}`, sub);
}

export function reviewDiffParams(scope, comparison = {}) {
  const mode = modeOf(comparison);
  const params = { task_id: scope.taskId, snapshot_id: scope.snapshot.id, directory_id: scope.directory.id, mode };
  if (mode === "commit") params.commit = comparison.commit;
  if (mode === "since") params.from_snapshot_id = comparison.from_snapshot_id;
  return params;
}

const publicValue = (value) => {
  if (!value) return null;
  const { read_order: _readOrder, ...visible } = value;
  return visible;
};

async function keepRead(at, value, readOrder, accepts) {
  await mergeCachedAtomically(at, (held) => accepts() && !(held?.read_order > readOrder) ? { ...value, read_order: readOrder } : null);
}

async function keepReadFailure(at, error, readOrder, accepts) {
  await mergeCachedAtomically(at, (held) => accepts() && !(held?.read_order > readOrder)
    ? { ...held, read_error: errorMessage(error), read_order: readOrder } : null);
}

function appendedCommits(held, answer) {
  const commits = new Map((held?.commits || []).map((row) => [commitOid(row), row]));
  for (const row of answer.commits || []) commits.set(commitOid(row), row);
  return { ...answer, commits: [...commits.values()] };
}

/** A kept rail reads the cache before requesting a fresh first page. */
export function createReviewCommitsReader({ callRpc, onChange = () => {}, active = () => true, generationOf = () => null, ...scope }) {
  const at = reviewCommitsAddress(scope);
  const rpc = reviewMutationRpc(scope.deviceId, callRpc);
  let disposed = false;
  let value = null;
  let sequence = 0;
  const current = () => !disposed && active();
  const hydrate = async () => {
    const saved = await readCached(at);
    if (!current()) return;
    value = publicValue(saved?.value);
    onChange(value);
  };
  const keepAnswer = async (answer, cursor, readOrder, accepts) => {
    if (!answer || !accepts()) return false;
    const previous = cursor ? (await readCached(at))?.value : null;
    await keepRead(at, cursor ? appendedCommits(previous, answer) : answer, readOrder, accepts);
    await hydrate();
    return true;
  };
  const read = async (cursor = null) => {
    const support = await readReviewSupport(scope.deviceId);
    if (!current() || !canReviewRead(support, { mode: "commits" })) return false;
    const serial = ++sequence;
    const readOrder = await nextTaskRead();
    const fence = pushFence();
    const accepts = () => current() && serial === sequence && !removedSince(at, fence);
    try {
      const answer = await rpc("tasks.review.diff", commitsParams(scope, cursor));
      return keepAnswer(answer, cursor, readOrder, accepts);
    } catch (error) {
      await keepReadFailure(at, error, readOrder, accepts);
      await hydrate();
      return false;
    }
  };
  const refresh = trailingRead(() => read(), { generationOf });
  const unwatch = subscribeCache(at, () => void hydrate());
  const ready = hydrate();
  void ready.then(() => current() && refresh());
  return { ready, refresh, more: () => value?.next_cursor ? read(value.next_cursor) : Promise.resolve(false), dispose() { disposed = true; sequence++; unwatch(); } };
}

const LIST_KINDS = ["task-review-commits", "task-review-changes", "task-review-commit-changes", "task-review-since-changes"];
function cachedSelection(record) {
  let sub;
  try { sub = JSON.parse(record.address.sub); } catch { return null; }
  if (!Array.isArray(sub) || sub.length < 3) return null;
  const mode = record.address.kind === "task-review-commits" ? "commits" : record.address.kind.replace(/^task-review-/, "").replace(/-changes$/, "");
  const comparison = { mode, ...(mode === "commit" ? { commit: sub[3] } : {}), ...(mode === "since" ? { from_snapshot_id: sub[3] } : {}) };
  return { taskId: sub[0], snapshot: { id: sub[1] }, directory: { id: sub[2] }, comparison };
}

/** Reconnect only refreshes reads the reader has visited. It never publishes. */
export async function refreshCachedReviewReads({ deviceId, projectId, taskId, callRpc, support, active = () => true }) {
  const groups = await Promise.all(LIST_KINDS.map((kind) => cachedRecords({ deviceId, entityId: projectId, kind })));
  const rpc = reviewMutationRpc(deviceId, callRpc);
  for (const record of groups.flat()) {
    if (!active()) return;
    const selection = cachedSelection(record);
    if (!selection || selection.taskId !== taskId || !canReviewRead(support, selection.comparison)) continue;
    const at = { deviceId, entityId: projectId, kind: record.address.kind, sub: record.address.sub };
    const readOrder = await nextTaskRead();
    const fence = pushFence();
    const accepts = () => active() && !removedSince(at, fence);
    try {
      const answer = await rpc("tasks.review.diff", { ...reviewDiffParams(selection, selection.comparison), ...(selection.comparison.mode === "commits" ? {} : { patch: false }) });
      if (answer) await keepRead(at, answer, readOrder, accepts);
    } catch (error) { await keepReadFailure(at, error, readOrder, accepts); }
  }
}
