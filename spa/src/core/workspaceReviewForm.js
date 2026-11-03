import "../styles/workspaceReview.css";
import { esc } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { watchReviewCreateDraft, watchReviewActionDraft, reviewCreateDraftAddress, reviewActionDraftAddress } from "./taskReviewDrafts.js";
import { readUiRecord, writeUiRecordIfUnwritten } from "./localUiStore.js";
import { createTaskReviewRepository, reviewFailure } from "./taskReviewCache.js";
import { assigneeControlHtml, draftAssignee, emptyAssigneeDraft, wireAssigneeControl } from "./trackerAssigneeControl.js";
import { readReviewTaskChoices, readReviewTaskPage, preselectedReviewTask, refreshReviewTaskChoices } from "./workspaceReviewTasks.js";
import { fieldTraits } from "./fieldTraits.js";
import { mergeCachedAtomically, deleteCached } from "./localCache.js";
import { writeTaskRecord, taskRecord } from "./trackerCache.js";
import { captureTaskContentRead } from "./taskContentAuthority.js";
import { contextFor, deviceContextIdentity } from "./deviceContexts.js";
import { deviceSession } from "./deviceReconnect.js";
import { readWorkspaceReviewContext, reviewBranchPreview, shortReviewRef, sourcePushDestination, workspaceReviewListAddress, boundSourceFacts, canPushSource, pushSourceParams } from "./workspaceReviewState.js";

const PREFIX = "review-create";
const TITLE_MAX_BYTES = 200;
const hasIncludedGit = (draft) => draft.bases.some((base) => !draft.excluded_git_directory_ids.includes(base.directory_id));
const gitSources = (context) => context.sources.filter(({ directory }) => directory.is_git !== false);
const baseOf = (context, directory) => directory.base_branch || context.project?.sources?.find((source) => source.id === directory.source_id)?.base_branch || context.project?.base_branch || "main";
const initialDraft = (context) => ({ request_id: crypto.randomUUID(), title: "", description: "",
  reviewerDraft: emptyAssigneeDraft("user"), for_task_id: context.support.forTask ? preselectedReviewTask(context, context.tasks) : "",
  bases: gitSources(context).map(({ directory }) => ({ directory_id: directory.id, branch: baseOf(context, directory) })),
  excluded_git_directory_ids: gitSources(context).filter(({ directory }) => directory.status === "failed" || directory.status === "unavailable").map(({ directory }) => directory.id) });

function sourceFormHtml({ directory, refs }, draft) {
  if (directory.is_git === false) return `<p><strong>${esc(directory.name)}</strong> · Live directory — files are not saved with this review.</p>`;
  const base = draft.bases.find((entry) => entry.directory_id === directory.id)?.branch || "";
  const excluded = draft.excluded_git_directory_ids.includes(directory.id);
  const options = (refs?.refs || []).filter((ref) => ["local", "branch"].includes(ref.kind))
    .map((ref) => `<option value="${esc(ref.name)}"></option>`).join("");
  return `<fieldset class="workspace-review-source"><legend>${esc(directory.name || directory.id)}</legend>
    <label class="workspace-review-exclude"><input type="checkbox" data-review-exclude="${esc(directory.id)}"${excluded ? " checked" : ""}>Exclude this Git directory</label>
    <label>Base branch<input data-review-base="${esc(directory.id)}" list="review-bases-${esc(directory.id)}" ${fieldTraits("identifier")} value="${esc(base)}"${excluded ? " disabled" : ""}></label>
    <datalist id="review-bases-${esc(directory.id)}">${options}</datalist>
    <p data-review-source-preview>${excluded ? "Excluded — committed changes will not be included." : `${esc(base)} ← <code>${esc(reviewBranchPreview(draft.title))}</code>`}</p>
  </fieldset>`;
}

const workTaskOptions = (tasks) => `<option value="">No linked task</option>${tasks.map((task) => `<option value="${esc(task.id)}">#${Number(task.number)} ${esc(task.title)}</option>`).join("")}`;
const workTaskField = (context) => context.support.forTask ? `<label>For task<select data-review-for-task>${workTaskOptions(context.tasks)}</select></label>
  <button type="button" class="btn" data-review-tasks-more hidden>Load more tasks</button><p class="sub" data-review-tasks-note hidden></p>` : "";

const createFormHtml = (context, draft) => `<h3 id="review-form-title">Request review</h3>
  <p class="sub">Create a review task, switch included Git directories to dedicated review branches and publish their committed work.</p>
  <form data-review-create-form class="workspace-review-form">
    <label>Title<input data-review-title required maxlength="${TITLE_MAX_BYTES}" ${fieldTraits("prose")} value="${esc(draft.title)}"></label>
    <label>Description<textarea data-review-description rows="3" ${fieldTraits("prose")}>${esc(draft.description)}</textarea></label>
    <div data-review-reviewer>${assigneeControlHtml(context.reviewerOptions, draft.reviewerDraft, { prefix: PREFIX, catalog: context.catalog, label: "Reviewer" })}</div>
    ${workTaskField(context)}
    <div data-review-create-sources>${context.sources.map((source) => sourceFormHtml(source, draft)).join("")}</div>
    <p>Branch preview: <code data-review-branch-preview>${esc(reviewBranchPreview(draft.title))}</code>. The task number and any required suffix are assigned when opening.</p>
    <p>Only committed changes are included.</p>
    <p class="warn" role="alert" data-review-form-error hidden></p>
    <p class="sub" data-review-retry-note${draft.submitted ? "" : " hidden"}>This opening was submitted. Retry uses the same saved request; its title, reviewer, linked task and sources are fixed.</p>
    <div class="row"><button type="button" class="btn" data-review-form-cancel>Cancel</button><button type="submit" class="btn primary" data-open-review-submit>${draft.submitted ? "Retry opening" : "Request review"}</button></div>
  </form>`;

function paintCreateValues(body, context, draft) {
  setFieldValue(body.querySelector("[data-review-title]"), draft.title);
  setFieldValue(body.querySelector("[data-review-description]"), draft.description);
  body.querySelector("[data-review-branch-preview]").textContent = reviewBranchPreview(draft.title);
  const option = context.reviewerOptions.find((entry) => entry.id === draft.reviewerDraft.optionId);
  if (!option && !draft.submitted) draft.reviewerDraft = emptyAssigneeDraft("user");
  body.querySelector("[data-assignee-select]").value = draft.reviewerDraft.optionId;
  body.querySelector(".task-assignee-hint").textContent = option?.hint || "";
  const workTask = body.querySelector("[data-review-for-task]");
  if (workTask) workTask.value = draft.for_task_id || "";
  paintSourceFields(body, draft);
}

const setFieldValue = (field, value) => { if (field.value !== value) field.value = value; };
function paintSourceFields(body, draft) {
  body.querySelectorAll("[data-review-base]").forEach((field) => {
    const id = field.dataset.reviewBase;
    const base = draft.bases.find((entry) => entry.directory_id === id)?.branch || "";
    const excluded = draft.excluded_git_directory_ids.includes(id);
    setFieldValue(field, base); field.disabled = excluded;
    const source = field.closest("fieldset");
    source.querySelector("[data-review-exclude]").checked = excluded;
    source.querySelector("[data-review-source-preview]").innerHTML = excluded ? "Excluded — committed changes will not be included."
      : `${esc(base)} ← <code>${esc(reviewBranchPreview(draft.title))}</code>`;
  });
}

function freezeCreate(body, submitted, busy, hydrating = false) {
  body.querySelectorAll("input,textarea,select").forEach((field) => {
    const excludedBase = field.hasAttribute("data-review-base") && field.closest("fieldset").querySelector("[data-review-exclude]").checked;
    field.disabled = Boolean(submitted || busy || hydrating || excludedBase);
  });
  body.querySelector("[data-open-review-submit]").disabled = busy || hydrating;
  body.querySelector("[data-open-review-submit]").textContent = hydrating ? "Loading…" : busy ? "Requesting…" : submitted ? "Retry opening" : "Request review";
  body.querySelector("[data-review-retry-note]").hidden = !submitted;
  const more = body.querySelector("[data-review-tasks-more]");
  if (more) more.disabled = Boolean(submitted || busy || hydrating);
}

function createDraftError(draft) {
  if (!draft.title.trim() || !hasIncludedGit(draft)) return "Enter a title and include at least one Git directory.";
  const title = draft.title.trim();
  if (!draft.submitted && (new TextEncoder().encode(title).byteLength > TITLE_MAX_BYTES || /[\r\n\0]/.test(title))) {
    return "Title must be a single line of at most 200 bytes.";
  }
  return "";
}

function createRequest(scope, context, draft) {
  const reviewer = draftAssignee(context.reviewerOptions, draft.reviewerDraft, context.catalog);
  return { workspace_id: scope.workspaceId, request_id: draft.request_id, title: draft.title.trim(), description: draft.description,
    ...(reviewer ? { reviewer } : {}),
    ...(context.support.forTask && draft.for_task_id ? { for_task_id: draft.for_task_id } : {}),
    bases: draft.bases.filter((base) => !draft.excluded_git_directory_ids.includes(base.directory_id)),
    excluded_git_directory_ids: draft.excluded_git_directory_ids };
}

async function keepOpenedWorkspace(scope, context, answer, contentRead, accept) {
  if (answer.task) await writeTaskRecord(scope.deviceId, scope.projectId, answer.task.id, taskRecord(answer.task), { contentRead, accept });
  const bindings = answer.review.bindings || [];
  await mergeCachedAtomically(workspaceReviewListAddress(scope), (held) => (held || []).map((workspace) => {
    if ((workspace.workspace_id || workspace.id) !== scope.workspaceId) return workspace;
    return { ...workspace, directories: (workspace.directories || []).map((directory) => {
      const binding = bindings.find((entry) => entry.directory_id === directory.id);
      const before = context.sources.find((source) => source.directory.id === directory.id)?.directory.branch;
      return binding?.preparation === "ready" && directory.branch === before
        ? { ...directory, branch: shortReviewRef(binding.dedicated_branch_ref) } : directory;
    }) };
  }));
  await deleteCached(context.sources.map((source) => ({ deviceId: scope.deviceId, entityId: source.entityId, kind: "refs" })));
}

function captureOpeningOwner({ deviceId }) {
  const context = contextFor(deviceId), identity = deviceContextIdentity(deviceId), session = deviceSession(deviceId);
  return () => contextFor(deviceId) === context && deviceContextIdentity(deviceId) === identity
    && deviceSession(deviceId) === session && (!context || context.active());
}

// Each explicit retry keeps the exact operation. Hydration never submits it.
export async function openReviewCreateForm(scope) {
  const context = await readWorkspaceReviewContext(scope);
  if (!context.support.open) return null;
  let draft = initialDraft(context);
  let hydrating = true;
  let busy = false;
  let closed = false;
  let writer;
  const anchor = document.activeElement;
  const modal = openModal({ dialogHtml: modalDialogHtml(createFormHtml(context, draft), { className: "modal-create workspace-review-modal" }),
    canDismiss: () => { closed = true; return true; },
    onClose: () => { closed = true; writer?.dispose(); scope.onClosed?.(); if (anchor?.isConnected) anchor.focus(); } });
  modal.body.setAttribute("aria-labelledby", "review-form-title");
  const taskChoices = mountReviewTaskChoices(modal.body, scope, context, {
    active: () => !closed, draft: () => draft, changed: () => { saveDraft(); freezeCreate(modal.body, draft.submitted, busy, hydrating); },
  });
  modal.body.querySelector("[data-review-form-cancel]").onclick = () => { closed = true; return modal.close(); };
  freezeCreate(modal.body, draft.submitted, busy, hydrating);
  const error = modal.body.querySelector("[data-review-form-error]");
  const saveDraft = () => writer.schedule(draft);
  const paint = (saved) => {
    if (busy || closed || !saved) return;
    draft = saved; paintCreateValues(modal.body, context, draft); wire(); freezeCreate(modal.body, draft.submitted, busy, hydrating);
  };
  const preview = () => {
    modal.body.querySelector("[data-review-branch-preview]").textContent = reviewBranchPreview(draft.title);
    paintSourceFields(modal.body, draft);
  };
  function wireSources() {
    modal.body.querySelectorAll("[data-review-base]").forEach((field) => { field.oninput = () => {
      draft.bases = draft.bases.map((base) => base.directory_id === field.dataset.reviewBase ? { ...base, branch: field.value } : base);
      saveDraft(); paintSourceFields(modal.body, draft);
    }; });
    modal.body.querySelectorAll("[data-review-exclude]").forEach((field) => { field.onchange = () => {
      draft.excluded_git_directory_ids = draft.excluded_git_directory_ids.filter((id) => id !== field.dataset.reviewExclude);
      if (field.checked) draft.excluded_git_directory_ids.push(field.dataset.reviewExclude);
      saveDraft(); preview();
    }; });
  }
  function wire() {
    modal.body.querySelector("[data-review-title]").oninput = (event) => { draft.title = event.target.value; saveDraft(); preview(); };
    modal.body.querySelector("[data-review-description]").oninput = (event) => { draft.description = event.target.value; saveDraft(); };
    wireAssigneeControl(modal.body, draft.reviewerDraft, { prefix: PREFIX, onDraft: (reviewerDraft) => {
      draft.reviewerDraft = reviewerDraft; saveDraft(); paint(draft);
    } });
    taskChoices.wire();
    wireSources();
  }
  writer = watchReviewCreateDraft(scope, paint);
  await writer.ready;
  if (closed) return modal;
  wire();
  modal.body.querySelector("[data-review-create-form]").onsubmit = async (event) => {
    event.preventDefault(); if (busy) return;
    const invalid = createDraftError(draft);
    if (invalid) { error.textContent = invalid; error.hidden = false; return; }
    busy = true; error.hidden = true;
    draft.submitted ||= createRequest(scope, context, draft);
    const sent = structuredClone(draft);
    freezeCreate(modal.body, true, busy);
    let captured;
    try {
      await writer.write(sent);
      captured = await captureMatchingDraft(reviewCreateDraftAddress(scope), sent);
      const accept = captureOpeningOwner(scope);
      // Opening discovers its task ID. This is action dispatch authority;
      // it cannot claim an addressed full-body revalidation baseline.
      const contentRead = await captureTaskContentRead(scope);
      if (!accept()) throw new Error("This machine's session changed before opening the review.");
      const repository = createTaskReviewRepository({ ...scope, active: accept });
      const answer = await repository.mutate("open", sent.submitted);
      if (!accept()) throw new Error("This machine's session changed while opening the review.");
      if (answer.opening_state !== "published") throw new Error("Opening is incomplete. Retry this saved opening to resume.");
      await keepOpenedWorkspace(scope, context, answer, contentRead, accept);
      await settleCreateDraft(scope, sent, captured, answer);
      await modal.close();
    } catch (failure) {
      draft = await recoverRejectedCreateDraft(scope, sent, captured, failure);
      if (!closed) { error.textContent = reviewFailure(failure); error.hidden = false; }
    } finally { busy = false; if (!closed) freezeCreate(modal.body, draft.submitted, busy); }
  };
  hydrating = false;
  await taskChoices.paint();
  void taskChoices.refresh();
  freezeCreate(modal.body, draft.submitted, busy, hydrating);
  modal.body.querySelector("[data-review-title]").focus();
  return modal;
}

function mountReviewTaskChoices(body, scope, context, state) {
  const field = body.querySelector("[data-review-for-task]");
  if (!field) return { wire() {}, paint: async () => {}, refresh: async () => {} };
  const more = body.querySelector("[data-review-tasks-more]");
  const note = body.querySelector("[data-review-tasks-note]");
  let reading = false;
  let next = null;
  const wire = () => { field.onchange = () => {
    Object.assign(state.draft(), { for_task_id: field.value, for_task_touched: true }); state.changed();
  }; };
  const paint = async () => {
    const [tasks, page] = await Promise.all([readReviewTaskChoices(scope), readReviewTaskPage(scope)]);
    if (!state.active()) return;
    context.tasks = tasks;
    const draft = state.draft();
    if (!draft.submitted && !draft.for_task_id && !draft.for_task_touched) draft.for_task_id = preselectedReviewTask(context, tasks);
    field.innerHTML = workTaskOptions(tasks);
    if (draft.for_task_id && !tasks.some((task) => task.id === draft.for_task_id)) {
      field.add(new Option("Selected task is unavailable", draft.for_task_id));
    }
    field.value = draft.for_task_id || "";
    next = page?.next_cursor;
    more.hidden = !next;
    state.changed();
    more.disabled = reading || Boolean(draft.submitted);
  };
  const refresh = async (cursor = null) => {
    if (reading || state.draft().submitted || !context.support.taskChoices) return;
    reading = true; more.disabled = true; note.hidden = true;
    try { await refreshReviewTaskChoices(scope, context.support, state.active, paint, cursor); }
    catch (error) { if (state.active()) { note.textContent = `Could not load tasks. ${reviewFailure(error)}`; note.hidden = false; } }
    finally { reading = false; if (state.active()) more.disabled = Boolean(state.draft().submitted); }
  };
  more.onclick = () => void refresh(next);
  return { wire, paint, refresh };
}

async function captureMatchingDraft(address, sent) {
  const captured = await readUiRecord(address);
  // A peer can replace the record during the writer's readback. Its write is
  // never ours to clear or relabel, even when it predates this capture.
  return JSON.stringify(captured?.value) === JSON.stringify(sent) ? captured : undefined;
}

async function recoverRejectedCreateDraft(scope, sent, captured, failure) {
  const code = failure.code || failure.error_code;
  if (code !== "invalid_params" || failure.uncertain || failure.timedOut) return sent;
  const editable = { ...sent };
  delete editable.submitted;
  delete editable.dispatchFailed;
  await writeUiRecordIfUnwritten(reviewCreateDraftAddress(scope), captured, editable);
  return editable;
}

function settleCreateDraft(scope, sent, captured, answer) {
  const retained = answer.reviewer_dispatch?.state === "failed" ? { ...sent, dispatchFailed: true } : null;
  return writeUiRecordIfUnwritten(reviewCreateDraftAddress(scope), captured, retained);
}

export async function openReviewPushForm(scope, review, fact) {
  const actionScope = { ...scope, taskId: review.task_id };
  let draft;
  let busy = false;
  let closed = false;
  const anchor = document.activeElement;
  const modal = openModal({ dialogHtml: modalDialogHtml(`<h3 id="review-push-title">Push to review</h3>
    <div data-push-review-destinations></div>
    <p data-push-review-pins></p><p>Only committed changes are included.</p>
    <p class="warn" role="alert" data-review-form-error hidden></p>
    <button type="button" class="btn" data-review-push-reset>Use latest cached changes</button>
    <div class="row"><button type="button" class="btn" data-review-form-cancel>Cancel</button><button type="button" class="btn primary" data-push-review-submit>Push</button></div>`, { className: "modal-create workspace-review-modal" }),
    onClose: () => { closed = true; writer.dispose(); scope.onClosed?.(); if (anchor?.isConnected) anchor.focus(); } });
  modal.body.setAttribute("aria-labelledby", "review-push-title");
  const paint = () => {
    modal.body.querySelector("[data-push-review-destinations]").innerHTML = pushDestinationsHtml(review, draft);
    modal.body.querySelector("[data-push-review-pins]").textContent = `Review version ${draft.expected_version} · ${draft.sources.map((source) => `${source.directory_id}: ${source.expected_head} (received ${source.expected_received_head})`).join("; ")}`;
  };
  const writer = watchReviewActionDraft(actionScope, "push", (saved) => {
    if (busy || closed || !saved) return;
    draft = saved; paint();
  });
  await writer.ready;
  draft ||= { expected_version: review.version, sources: [pushSourceParams(fact)] };
  // A prior unsuccessful request is still pinned even after observations move.
  paint();
  const button = modal.body.querySelector("[data-push-review-submit]");
  const error = modal.body.querySelector("[data-review-form-error]");
  const reset = modal.body.querySelector("[data-review-push-reset]");
  reset.onclick = async () => {
    const context = await readWorkspaceReviewContext(scope);
    const latest = boundSourceFacts(context).find((source) => source.binding.directory_id === fact.binding.directory_id);
    if (!context.support.push || !canPushSource(latest)) {
      error.textContent = "The selected review branch has no publishable cached observation. Switch back or wait for its next observation."; error.hidden = false; return;
    }
    review = context.held.review;
    draft = { expected_version: review.version, sources: [pushSourceParams(latest)] };
    await writer.write(draft); paint(); error.hidden = true;
  };
  modal.body.querySelector("[data-review-form-cancel]").onclick = modal.close;
  button.onclick = async () => {
    if (busy) return; busy = true; button.disabled = true; reset.disabled = true; error.hidden = true;
    const sent = structuredClone(draft);
    try {
      await writer.write(sent);
      const captured = await captureMatchingDraft(reviewActionDraftAddress(actionScope, "push"), sent);
      const answer = await createTaskReviewRepository(actionScope).mutate("push", sent);
      if (answer.sources?.some((source) => ["failed", "interrupted"].includes(source.status))) throw new Error(answer.sources.map((source) => source.error).filter(Boolean).join("; ") || "Publication did not finish. Inspect the saved result before retrying.");
      await writeUiRecordIfUnwritten(reviewActionDraftAddress(actionScope, "push"), captured, null); await modal.close();
    } catch (failure) { if (!closed) { error.textContent = reviewFailure(failure); error.hidden = false; } }
    finally { busy = false; if (!closed) { button.disabled = false; reset.disabled = false; } }
  };
  return modal;
}

function pushDestinationsHtml(review, draft) {
  return draft.sources.map((source) => {
    const binding = review.bindings.find((entry) => entry.directory_id === source.directory_id);
    const name = review.snapshots.at(-1)?.directories.find((directory) => directory.id === source.directory_id)?.name || source.directory_id;
    return `<p>Publish committed work from ${esc(name)} to:</p><p><code>${esc(binding ? sourcePushDestination({ binding }) : "Saved destination unavailable")}</code></p>`;
  }).join("");
}
