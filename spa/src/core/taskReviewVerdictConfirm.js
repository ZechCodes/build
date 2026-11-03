// Verdict confirmations explain the handoff; rejection also captures its reason.
import { confirmAction, confirmModalHtml } from "./confirm.js";
import { openModal } from "./modal.js";
import { fieldTraits } from "./fieldTraits.js";

export const reviewVerdictLabel = (verdict) => ({ approve: "Approve", request_changes: "Request changes", reject: "Reject" })[verdict];

function nextAction(verdict, author, handoff) {
  if (verdict === "reject") return `Nothing is merged. ${author} is told to stop. You can reopen later.`;
  if (verdict === "approve" && handoff) return `${author} is told it's approved and takes it forward (merge, deploy, …)`;
  return `${author} is told and continues.`;
}

export async function confirmReviewVerdict({ verdict, revision, count, author = "The author", summary = "", handoff = false }) {
  const label = reviewVerdictLabel(verdict);
  const options = { title: label, confirmLabel: label, danger: verdict === "reject",
    intro: `Submit revision ${revision} with ${count} pending ${count === 1 ? "comment" : "comments"}.`,
    actions: ["Post the pending comments and your verdict.", nextAction(verdict, author, handoff)] };
  if (verdict !== "reject") return { confirmed: await confirmAction(options) };
  return confirmReject(options, summary);
}

function confirmReject(options, summary) {
  return new Promise((resolve) => {
    let answer = { confirmed: false };
    const { body, close } = openModal({ dialogHtml: confirmModalHtml(options), scrimId: "confirm-scrim", onClose: () => resolve(answer) });
    const label = document.createElement("label");
    label.className = "create-label";
    label.innerHTML = `Reason (required)<textarea rows="3" required data-review-reject-reason ${fieldTraits("prose")} placeholder="Explain why this review should stop"></textarea>`;
    const actions = body.querySelector("[data-confirm-ok]").parentElement;
    body.insertBefore(label, actions);
    const reason = label.querySelector("textarea");
    const confirm = body.querySelector("[data-confirm-ok]");
    reason.value = summary;
    reason.oninput = () => { confirm.disabled = !reason.value.trim(); };
    reason.oninput();
    confirm.onclick = () => {
      if (!reason.value.trim()) return;
      answer = { confirmed: true, reason: reason.value.trim() };
      close();
    };
    body.querySelector("[data-confirm-cancel]").onclick = close;
    reason.focus();
  });
}
