// Work tasks link to their separate review tasks using saved cache facts.
import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { cachedRecords, subscribeCache } from "./localCache.js";
import { taskReviewStatusLabel } from "./taskReviewSummaryRender.js";
import { TRACKER_TASKS_KIND, TRACKER_TASK_KIND } from "./trackerCache.js";

const reviewOf = (row) => row?.review_summary;
const isChildReview = (row, taskId) => reviewOf(row)
  && (reviewOf(row).for_task_id || row.links?.parent_task_id) === taskId;
function workReviewRows(task, cached = []) {
  const saved = (task.linked_reviews || []).map((summary) => ({ id: summary.task_id, number: summary.number, review_summary: summary }));
  const rows = new Map(saved.map((row) => [row.id, row]));
  for (const row of cached) {
    const before = rows.get(row.id);
    if (!before || reviewOf(row).version > reviewOf(before).version) rows.set(row.id, { ...row, number: row.number ?? before?.number });
  }
  return [...rows.values()].sort((a, b) => b.number - a.number);
}

const parentRoute = (context, taskId) => ({ name: "trackerTask", deviceId: context.deviceId, projectId: context.projectId, taskId });

export function taskWorkReviewHtml(task, context = {}) {
  if (!context.forTask || task.review_summary || context.review) return "";
  const available = workReviewRows(task, context.childReviews);
  const children = available.slice(0, 20);
  const truncated = task.linked_reviews_truncated || available.length > children.length;
  if (children.length) return `<div class="task-work-reviews">${children.map((child) => {
    const label = taskReviewStatusLabel(reviewOf(child).status);
    return `<a class="task-work-review-chip" data-work-task-review href="${esc(hashFromRoute(parentRoute(context, child.id)))}">Review #${esc(child.number)} · ${esc(label)}</a>`;
  }).join("")}</div>${truncated ? `<p class="task-work-review-hint">Showing latest ${esc(children.length)} reviews.</p>` : ""}`;
  const workspace = context.links?.find((link) => link.kind === "workspace" && link.route);
  return workspace ? `<p class="task-work-review-hint"><a href="${esc(hashFromRoute(workspace.route))}">Request a review from @${esc(workspace.label)}</a></p>` : "";
}

function newestReviewRows(records, taskId) {
  const rows = new Map();
  for (const { address, value } of records) {
    const tasks = address.kind === TRACKER_TASK_KIND ? [value?.task] : value?.tasks || [];
    for (const task of tasks) {
      if (!isChildReview(task, taskId)) continue;
      const held = rows.get(task.id);
      if (!held || reviewOf(task).version >= reviewOf(held).version) rows.set(task.id, task);
    }
  }
  return [...rows.values()].sort((a, b) => b.number - a.number);
}

export function watchWorkTaskReviews({ deviceId, projectId, taskId, onChange }) {
  let disposed = false;
  let serial = 0;
  async function read() {
    const current = ++serial;
    const [listed, details] = await Promise.all([
      cachedRecords({ deviceId, entityId: projectId, kind: TRACKER_TASKS_KIND }),
      cachedRecords({ deviceId, entityId: projectId, kind: TRACKER_TASK_KIND }),
    ]);
    if (!disposed && current === serial) onChange(newestReviewRows([...listed, ...details], taskId));
  }
  const stopList = subscribeCache({ deviceId, entityId: projectId, kind: TRACKER_TASKS_KIND }, () => void read());
  const stopDetails = subscribeCache({ deviceId, entityId: projectId, kind: TRACKER_TASK_KIND }, () => void read());
  void read();
  return () => { disposed = true; stopList(); stopDetails(); };
}

export function namesLinkedReview(items, task, children = []) {
  const ids = new Set([...(task?.linked_reviews || []).map((review) => review.task_id), ...children.map((review) => review.id)]);
  return (items || []).some((item) => item.tasks?.task_ids?.some((id) => ids.has(id)));
}
