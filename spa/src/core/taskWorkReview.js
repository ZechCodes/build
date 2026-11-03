// Work tasks link to their separate review tasks using saved cache facts.
import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { taskReviewStatusLabel } from "./taskReviewSummaryRender.js";

const parentRoute = (context, taskId) => ({ name: "trackerTask", deviceId: context.deviceId, projectId: context.projectId, taskId });

export function taskWorkReviewHtml(task, context = {}) {
  if (!context.forTask || task.review_summary || context.review) return "";
  const available = [...(task.linked_reviews || [])].sort((a, b) => b.number - a.number);
  const children = available.slice(0, 20);
  const truncated = task.linked_reviews_truncated || available.length > children.length;
  if (children.length) return `<div class="task-work-reviews">${children.map((child) => {
    const label = child.status === "rejected" && !context.reviewSupport?.reject ? "" : taskReviewStatusLabel(child.status);
    return `<a class="task-work-review-chip" data-work-task-review href="${esc(hashFromRoute(parentRoute(context, child.task_id)))}">Review #${esc(child.number)}${label ? ` · ${esc(label)}` : ""}</a>`;
  }).join("")}</div>${truncated ? `<p class="task-work-review-hint">Showing latest ${esc(children.length)} reviews.</p>` : ""}`;
  return workspaceHintHtml(context);
}

function workspaceHintHtml(context) {
  const workspace = context.links?.find((link) => link.kind === "workspace" && link.route);
  return workspace ? `<p class="task-work-review-hint"><a href="${esc(hashFromRoute(workspace.route))}">Request a review from @${esc(workspace.label)}</a></p>` : "";
}
