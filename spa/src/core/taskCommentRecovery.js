import { esc, messageOf } from "./text.js";

const effectOf = (row) => row.effect || row.confirmation?.effect;
const needsAttention = (row) => !row.dismissed && (row.unresolved || ["failed", "outcome_unknown", "result_expired"].includes(row.state));
const failureHtml = (row) => {
  const refused = effectOf(row) === "unapplied";
  return `<div class="chat-recovery-entry" data-write-operation="${esc(row.operationId)}">
    <span><strong>${refused ? "Your comment wasn’t saved" : "Comment outcome unresolved"}</strong>
      <span>${esc(row.draft?.body || "")}</span>
      <span>${esc(messageOf(row.error || row.confirmation?.error || ""))}</span>
    </span>
    <button type="button" data-write-action="retry">${refused ? "Retry" : "Check again"}</button>
    <button type="button" data-write-action="dismiss">Dismiss</button>
  </div>`;
};

/** Action-local recovery is a projection of durable operations. Dismissing a
 * notice never removes its intent or clears the person's restored draft. */
export function mountTaskCommentRecovery({ host, service, taskId, projectId, restoreDraft, retry }) {
  const restored = new Set();
  const restoring = new Set();
  let disposed = false;
  const rows = () => (service?.readOperations() || []).filter((row) => row.owner?.kind === "task-comment"
    && row.owner.taskId === taskId && row.owner.projectId === projectId && needsAttention(row));

  const act = async (button, row) => {
    button.disabled = true;
    try {
      if (button.dataset.writeAction === "dismiss") await service.dismiss(row.operationId);
      else if (effectOf(row) !== "unapplied") await service.checkAgain(row.operationId);
      else {
        await retry(row);
        await service.dismiss(row.operationId);
      }
    } finally {
      if (!disposed) sync();
    }
  };

  const restore = (row) => {
    if (effectOf(row) !== "unapplied" || restored.has(row.operationId) || restoring.has(row.operationId)) return;
    restoring.add(row.operationId);
    void Promise.resolve(restoreDraft(row.draft)).then((settled) => {
      if (settled !== false) restored.add(row.operationId);
    }).finally(() => restoring.delete(row.operationId)).catch(() => {});
  };

  const sync = () => {
    if (disposed) return;
    const held = rows();
    held.forEach(restore);
    const form = host.querySelector("[data-task-composer]");
    if (!form) return;
    let notice = form.querySelector("[data-comment-recovery]");
    if (!notice) {
      notice = document.createElement("div");
      notice.dataset.commentRecovery = "";
      form.prepend(notice);
    }
    notice.innerHTML = held.map(failureHtml).join("");
    notice.querySelectorAll("[data-write-action]").forEach((button) => {
      const operationId = button.closest("[data-write-operation]").dataset.writeOperation;
      button.onclick = () => { void act(button, held.find((row) => row.operationId === operationId)).catch(() => {}); };
    });
  };
  const unsubscribe = service?.subscribe(sync) || (() => {});
  return { sync, dispose() { disposed = true; unsubscribe(); } };
}
