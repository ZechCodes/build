// Review-loop facts are derived only from cached comments, assignment and support.
export function advisoryReviewOpinion(row, context = {}) {
  if (!context.reviewSupport?.reject || !row.opinion) return false;
  const actor = row.actor || row.author;
  if (actor?.kind !== "agent") return false;
  const assignee = context.reviewAssignee;
  return assignee?.kind !== "agent" || assignee.agent_id !== actor.agent_id;
}

export const advisoryReviewHtml = (row, context) => advisoryReviewOpinion(row, context)
  ? '<span class="task-review-advisory">Advisory</span>' : "";

const NOTICE_TEXT = Object.freeze({
  revision_ready: "A new revision is ready for review.",
  verdict_undelivered: "The verdict could not be delivered to the author.",
  revision_undelivered: "The revision could not be delivered to the reviewer.",
});

export function reviewNoticeText(row, context = {}) {
  if (!context.reviewSupport?.verdictDelivery || (row.actor || row.author)?.kind !== "build") return "";
  return NOTICE_TEXT[row.reviewNotice || row.review_notice] || "";
}
