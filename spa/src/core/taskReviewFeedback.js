// Feedback on a saved task review. The task timeline remains its history;
// this control only writes task comments with optional review metadata.
import { messageOf } from "./text.js";
import { fieldTraits } from "./fieldTraits.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { readUiRecord, writeUiRecordIfUnwritten } from "./localUiStore.js";
import { notifyError } from "./notify.js";
import { assigneeOptions, selectedOptionId, workspaceAgents } from "./trackerAssignee.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";

const emptyDraft = () => ({ body: "", verdict: "", anchor: null, replyTo: null });
const snapshotIdOf = (snapshot) => snapshot?.id || snapshot?.snapshot_id || "";
const validAnchor = (anchor, snapshotId) => anchor?.snapshot_id === snapshotId
  && typeof anchor.directory_id === "string" && typeof anchor.path === "string"
  && ["old", "new"].includes(anchor.side) && Number.isSafeInteger(anchor.line) && anchor.line > 0;

const anchorLabel = (anchor) => `${anchor.path} · ${anchor.side} line ${anchor.line}`;
const sameDraft = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const hasBody = (draft) => Boolean(draft.body.trim());
const commentParams = (taskId, snapshotId, draft) => ({
  task_id: taskId,
  body: draft.body.trim(),
  ...(draft.anchor ? { anchor: draft.anchor } : {}),
  ...(draft.replyTo ? { reply_to: draft.replyTo } : {}),
  ...(draft.verdict ? { opinion: { snapshot_id: snapshotId, verdict: draft.verdict } } : {}),
});

/** One review's composer. A new mount reads its task/snapshot draft from build-ui. */
export function mountTaskReviewFeedback(host, {
  deviceId, projectId, taskId, snapshot, callRpc, onSent = null,
}) {
  const snapshotId = snapshotIdOf(snapshot);
  let draft = emptyDraft();
  let sending = false;
  let disposed = false;
  let draftRevision = 0;
  host.innerHTML = `<form class="task-review-feedback" data-review-feedback>
    <label class="create-label" for="task-review-feedback-body">Review feedback</label>
    <p class="sub" data-review-target hidden></p>
    <textarea id="task-review-feedback-body" rows="3" ${fieldTraits("prose")} placeholder="Comment on this review"></textarea>
    <div class="row task-review-feedback-row">
      <label class="create-label" for="task-review-feedback-opinion">Opinion</label>
      <select id="task-review-feedback-opinion" data-review-opinion>
        <option value="">Comment only</option><option value="approve">Approve</option>
        <option value="request_changes">Request changes</option>
      </select>
      <button class="btn primary" type="submit" disabled>Comment</button>
    </div>
  </form>`;
  const form = host.querySelector("[data-review-feedback]");
  const field = form.querySelector("textarea");
  const opinion = form.querySelector("select");
  const target = form.querySelector("[data-review-target]");
  const button = form.querySelector('button[type="submit"]');
  const paint = (saved) => {
    if (disposed) return;
    draft = { ...emptyDraft(), ...saved };
    if (field.value !== draft.body) field.value = draft.body;
    opinion.value = draft.verdict;
    const parts = [draft.anchor && anchorLabel(draft.anchor), draft.replyTo && `Reply to ${draft.replyTo}`].filter(Boolean);
    target.textContent = parts.join(" · ");
    target.hidden = !parts.length;
    button.disabled = sending || !hasBody(draft);
    button.textContent = sending ? "sending…" : "Comment";
  };
  const address = uiAddress({ deviceId, entityId: projectId, kind: "draft", view: "task-review-feedback", sub: `${taskId}:${snapshotId}` });
  const saved = watchUiState(address, paint, { debounceMs: 100 });
  const edit = (changes) => {
    draftRevision += 1;
    draft = { ...draft, ...changes };
    saved.schedule(draft);
    paint(draft);
  };
  field.oninput = () => edit({ body: field.value });
  opinion.onchange = () => edit({ verdict: opinion.value });
  const canSubmit = () => !disposed && !sending && hasBody(draft);
  const clearSentDraft = async (sent, sentRevision, captured) => {
    if (draftRevision !== sentRevision || !sameDraft(draft, sent)) return;
    if (sameDraft(captured?.value, sent)) await writeUiRecordIfUnwritten(address, captured, emptyDraft());
    else if (!captured) paint(emptyDraft());
  };
  form.onsubmit = async (event) => {
    event.preventDefault();
    if (!canSubmit()) return;
    const sent = { ...draft };
    const sentRevision = draftRevision;
    sending = true;
    paint(draft);
    try {
      await saved.flush();
      const captured = await readUiRecord(address).catch(() => null);
      await callRpc("tasks.comment", commentParams(taskId, snapshotId, sent));
      await clearSentDraft(sent, sentRevision, captured);
      if (!disposed) await onSent?.();
    } catch (error) {
      if (!disposed) notifyError("Could not add review feedback", messageOf(error));
    } finally {
      sending = false;
      paint(draft);
    }
  };
  return {
    comment(anchor = null, replyTo = null) {
      if (disposed) return;
      if (anchor && !validAnchor(anchor, snapshotId)) return;
      edit({ anchor, replyTo });
      field.focus();
    },
    dispose() {
      disposed = true;
      saved.dispose();
      form.onsubmit = null;
    },
  };
}

/** Route a saved snapshot to any ordinary task assignee. */
export function openTaskReviewer({ task, snapshot, feed, projectKey, catalog = null, callRpc, onAssigned = null }) {
  const groups = workspaceAgents(typeof feed === "function" ? feed() : feed, projectKey);
  const number = snapshot?.number ?? snapshot?.snapshot_number ?? snapshotIdOf(snapshot);
  return openAssigneePicker({
    task,
    options: assigneeOptions(groups),
    current: selectedOptionId(task.assignee),
    catalog: typeof catalog === "function" ? catalog() : catalog,
    callRpc,
    onAssigned,
    note: `Review snapshot ${number} of task #${task.number ?? task.id}.`,
  });
}
