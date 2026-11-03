// A PR's saved status is independent of its task's ordinary board column.
// Every row reads this same optional review_summary; labels and columns never
// stand in for a review record.
import { esc } from "./text.js";
import { ICON_GIT_PULL_REQUEST } from "./icons.js";
import "../styles/taskReviewSummary.css";

const STATUS_LABELS = new Map([
  ["open", "Open"], ["approved", "Approved"], ["changes_requested", "Changes requested"],
  ["merged", "Merged"], ["closed", "Closed"], ["rejected", "Rejected"],
]);

export const taskReviewStatusLabel = (status) => STATUS_LABELS.get(status) || "";

const isLegacyReview = (summary) => summary?.mode === "snapshot" ||
  (!summary?.mode && Array.isArray(summary?.snapshots));

export function taskReviewSummaryHtml(summary, support = {}) {
  if (isLegacyReview(summary)) return support.reject
    ? '<span class="task-review-summary" data-review-mode="snapshot">Legacy review</span>' : "";
  if (summary?.status === "rejected" && !support.reject) return "";
  const label = taskReviewStatusLabel(summary?.status);
  if (!label) return "";
  return `<span class="task-review-summary" data-review-status="${esc(summary.status)}" aria-label="Review: ${esc(label)}" title="Review: ${esc(label)}"><span class="task-review-icon" aria-hidden="true">${ICON_GIT_PULL_REQUEST}</span><span>${esc(label)}</span></span>`;
}
