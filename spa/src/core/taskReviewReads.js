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
const successfulCommitList = (value) => Array.isArray(value?.commits) && !value.read_error;
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

function refreshedCommits(held, answer) {
  if (!held?.commits?.length) return answer;
  const merged = appendedCommits(held, answer);
  const preserveProgress = held.commits.length > (answer.commits || []).length;
  const result = { ...answer, commits: merged.commits };
  if (preserveProgress) {
    result.commits_truncated = held.commits_truncated;
    result.next_cursor = held.next_cursor;
  }
  return result;
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
    return value;
  };
  const keepAnswer = async (answer, cursor, readOrder, accepts) => {
    if (!answer || !accepts()) return false;
    const previous = (await readCached(at))?.value;
    const next = cursor ? appendedCommits(previous, answer) : refreshedCommits(previous, answer);
    await keepRead(at, next, readOrder, accepts);
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
  void ready.then((held) => { if (current() && !successfulCommitList(held)) return refresh(); });
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

const taskIsSelected = (taskId, options) => options.taskIds ? options.taskIds.includes(taskId) : !options.taskId || options.taskId === taskId;
function needsReadRefresh(record, selection, latestSnapshotIds) {
  if (record.value?.read_error) return true;
  return selection.comparison.mode === "commits" && latestSnapshotIds?.get(selection.taskId) === selection.snapshot.id;
}

async function refreshReadRecord(record, selection, { rpc, active }) {
  const at = record.address;
  const readOrder = await nextTaskRead();
  const fence = pushFence();
  const accepts = () => active() && !removedSince(at, fence);
  try {
    const isCommits = selection.comparison.mode === "commits";
    const answer = await rpc("tasks.review.diff", { ...reviewDiffParams(selection, selection.comparison), ...(isCommits ? {} : { patch: false }) });
    if (!answer) return;
    const next = isCommits ? refreshedCommits((await readCached(at))?.value, answer) : answer;
    await keepRead(at, next, readOrder, accepts);
  } catch (error) { await keepReadFailure(at, error, readOrder, accepts); }
}

/** One project-wide reconnect scan retries failures and refreshes only the
 * latest rail. Published successful comparison lists are immutable. */
export async function refreshCachedReviewReads({ deviceId, projectId, callRpc, support, active = () => true, latestSnapshotIds, ...options }) {
  const groups = await Promise.all(LIST_KINDS.map((kind) => cachedRecords({ deviceId, entityId: projectId, kind })));
  const rpc = reviewMutationRpc(deviceId, callRpc);
  for (const record of groups.flat()) {
    if (!active()) return;
    const selection = cachedSelection(record);
    if (!selection || !taskIsSelected(selection.taskId, options)) continue;
    if (!canReviewRead(support, selection.comparison) || !needsReadRefresh(record, selection, latestSnapshotIds)) continue;
    await refreshReadRecord(record, selection, { rpc, active });
  }
}
