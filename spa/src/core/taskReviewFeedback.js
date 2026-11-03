// Review drafts stay with their saved revision; verdicts end an ordered batch.
import { messageOf, esc } from "./text.js";
import { fieldTraits } from "./fieldTraits.js";
import { notifyError } from "./notify.js";
import { confirmAction } from "./confirm.js";
import { createTaskReviewBatch } from "./taskReviewBatch.js";
import { createReviewInlineComposer } from "./taskReviewInlineComposer.js";
import { assigneeOptions, selectedOptionId, assignableAgents } from "./trackerAssignee.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";

const snapshotIdOf = (snapshot) => snapshot?.id || snapshot?.snapshot_id || "";
const validAnchor = (anchor, snapshotId) => !anchor || anchor.snapshot_id === snapshotId
  && typeof anchor.directory_id === "string" && typeof anchor.path === "string"
  && ["old", "new"].includes(anchor.side) && Number.isSafeInteger(anchor.line) && anchor.line > 0;
const verdictLabel = (verdict) => verdict === "approve" ? "Approve" : "Request changes";

function draftHtml(draft, locked) {
  const place = draft.anchor ? `${draft.anchor.path}:${draft.anchor.line} · ${draft.anchor.side}` : "Reply";
  return `<li data-review-draft="${esc(draft.id)}"><button class="task-review-draft-jump" type="button" data-review-draft-jump>${esc(place)}</button>
    <p>${esc(draft.body)}</p><div class="row"><button class="btn" type="button" data-review-draft-edit${locked ? " disabled" : ""}>Edit</button><button class="btn" type="button" data-review-draft-remove${locked ? " disabled" : ""}>Remove</button></div></li>`;
}

function submissionMessage(state, sending) {
  if (state.submission?.enqueueError) return `${state.error} Your review is saved here. Retry the saved review to send it.`;
  if (state.error) return `${state.error} The review could not finish. Pending comments are kept here; edit them to try again.`;
  if (state.submission) return 'Review saved. Waiting for acknowledgement; it will send when this device is available.';
  return sending ? 'Saving review…' : '';
}

export function mountTaskReviewFeedback(host, options) {
  const { deviceId, projectId, taskId, snapshot, callRpc, writeOperations, taskNumber, priorAnchors,
    verdictHost = host, allowOpinion = true, latestSnapshotId = () => null, onSent, onJump,
    onCount = () => {}, placeComposer } = options;
  let disposed = false;
  let sending = false;
  let confirming = false;
  let state = { drafts: [], summary: "" };
  host.innerHTML = `<section class="task-review-pending" data-review-pending aria-label="Pending review comments">
    <h3>Pending <span data-review-count>0</span></h3><p class="sub" data-review-draft-revision></p>
    <table data-review-reply-composer><tbody></tbody></table><ol data-review-drafts></ol><label class="create-label">Summary (optional)<textarea rows="2" data-review-summary ${fieldTraits("prose")} placeholder="Summarize your review"></textarea></label>
    <button class="btn" type="button" data-review-comment-batch>Comment</button><p class="sub" data-review-submit-status role="status"></p><button class="btn" type="button" data-review-recover hidden>Edit pending comments</button><button class="btn" type="button" data-review-retry hidden>Retry saved review</button></section>`;
  const bar = document.createElement("div");
  bar.className = "task-review-verdict-bar";
  bar.innerHTML = '<button class="btn" type="button" data-review-verdict="request_changes">Request changes</button><button class="btn primary" type="button" data-review-verdict="approve">Approve</button>';
  verdictHost.append(bar);
  const summary = host.querySelector('[data-review-summary]');
  const historical = () => Boolean(latestSnapshotId()) && latestSnapshotId() !== snapshotIdOf(snapshot);
  const batch = createTaskReviewBatch({ deviceId, projectId, taskId, taskNumber, snapshot, callRpc, writeOperations, priorAnchors,
    onChange: (saved) => { state = saved; paint(); } });
  const composer = createReviewInlineComposer({ batch, snapshotNumber: snapshot.number, placeComposer: (row, anchor) => {
    if (!placeComposer?.(row, anchor)) host.querySelector('[data-review-reply-composer] tbody').append(row);
  } });
  const commentButton = host.querySelector('[data-review-comment-batch]');

  function paintSubmission() {
    host.querySelector('[data-review-submit-status]').textContent = submissionMessage(state, sending);
    host.querySelector('[data-review-recover]').hidden = state.submission?.status !== 'failed' || Boolean(state.submission?.enqueueError);
    host.querySelector('[data-review-retry]').hidden = !state.submission?.enqueueError;
    host.querySelector('[data-review-retry]').disabled = sending;
  }

  function paint() {
    if (disposed) return;
    const drafts = state.drafts.filter((draft) => draft.body.trim());
    host.querySelector('[data-review-count]').textContent = String(drafts.length);
    host.querySelector('[data-review-drafts]').innerHTML = drafts.map((draft) => draftHtml(draft, Boolean(state.submission))).join("");
    if (summary !== document.activeElement && summary.value !== state.summary) summary.value = state.summary || "";
    const revision = host.querySelector('[data-review-draft-revision]');
    revision.textContent = historical()
      ? `Drafts belong to earlier revision ${snapshot.number}. Opinions on this earlier revision do not change the current review status.`
      : `Revision ${snapshot.number}`;
    bar.querySelectorAll('[data-review-verdict]').forEach((button) => {
      button.disabled = !opinionAllowed() || busy() || confirming;
      button.title = historical() ? "Choose the latest revision to submit a verdict." : "";
    });
    commentButton.disabled = busy() || !drafts.length;
    summary.disabled = busy();
    paintSubmission();
    onCount(drafts.length);
  }

  const opinionAllowed = () => allowOpinion && !historical();
  const busy = () => sending || Boolean(state.submission);
  async function send(verdict = "") {
    if (disposed || busy()) return;
    if (verdict && !opinionAllowed()) return;
    sending = true; composer.close(); paint();
    try {
      await batch.submit(verdict);
      if (!disposed) void Promise.resolve(onSent?.()).catch(() => {});
    } catch (error) {
      if (!disposed) notifyError("Could not submit review", messageOf(error));
    } finally { sending = false; paint(); }
  }
  async function confirmVerdict(verdict) {
    if (disposed || confirming || busy() || historical()) return;
    confirming = true; paint();
    const count = state.drafts.filter((draft) => draft.body.trim()).length;
    const confirmed = await confirmAction({ title: verdictLabel(verdict), confirmLabel: verdictLabel(verdict),
      intro: `Submit revision ${snapshot.number} with ${count} pending ${count === 1 ? "comment" : "comments"}.`,
      actions: ["Post the pending comments and your verdict.", "The author is told and continues."],
    });
    confirming = false; paint();
    if (confirmed && !disposed) await send(verdict);
  }
  summary.oninput = () => { void batch.summary(summary.value); };
  commentButton.onclick = () => void send();
  bar.querySelectorAll('[data-review-verdict]').forEach((button) => {
    button.onclick = () => void confirmVerdict(button.dataset.reviewVerdict);
  });
  host.querySelector('[data-review-retry]').onclick = async () => {
    if (sending) return;
    sending = true; paint();
    try { await batch.retry(); }
    catch (error) { notifyError("Could not submit review", messageOf(error)); }
    finally { sending = false; paint(); }
  };
  host.querySelector('[data-review-recover]').onclick = async () => {
    try { await batch.recover(); }
    catch (error) { notifyError("Could not edit pending comments", messageOf(error)); }
  };
  host.onclick = (event) => {
    const row = event.target.closest('[data-review-draft]');
    if (!row) return;
    const draft = state.drafts.find((draft) => draft.id === row.dataset.reviewDraft);
    if (!draft) return;
    if (event.target.closest('[data-review-draft-jump]')) { onJump?.(draft.anchor, draft.view); return; }
    if (state.submission) return;
    if (event.target.closest('[data-review-draft-edit]')) {
      void Promise.resolve(onJump?.(draft.anchor, draft.view)).then(() => { if (!disposed) composer.edit(draft); });
    }
    if (event.target.closest('[data-review-draft-remove]')) { composer.remove(draft.id); void batch.remove(draft.id); }
  };
  paint();
  return {
    ready: batch.ready,
    update: paint,
    placeComposer: composer.place,
    comment(anchor = null, replyTo = null, view = "changes") {
      if (disposed || !validAnchor(anchor, snapshotIdOf(snapshot))) return;
      return composer.comment(anchor, replyTo, view);
    },
    dispose() { disposed = true; composer.dispose(); batch.dispose(); bar.remove(); host.onclick = null; },
  };
}

/** Route a saved snapshot to any ordinary task assignee. */
export function openTaskReviewer({ task, snapshot, feed, projectKey, deviceId, projectId, catalog = null, callRpc, onAssigned = null }) {
  const groups = assignableAgents(typeof feed === "function" ? feed() : feed, projectKey);
  const number = snapshot?.number ?? snapshot?.snapshot_number ?? snapshotIdOf(snapshot);
  return openAssigneePicker({
    task,
    deviceId,
    projectId,
    options: assigneeOptions(groups),
    current: selectedOptionId(task.assignee),
    catalog: typeof catalog === "function" ? catalog() : catalog,
    callRpc,
    onAssigned,
    note: `Review revision ${number} of task #${task.number ?? task.id}.`,
  });
}
