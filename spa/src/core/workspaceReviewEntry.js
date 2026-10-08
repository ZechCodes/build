import "../styles/taskReview.css";
import "../styles/workspaceReview.css";
import { esc } from "./text.js";
import { readCached, subscribeCache, deleteCached } from "./localCache.js";
import { readTasksRecord } from "./trackerCache.js";
import { hashFromRoute } from "./router.js";
import { subscribeUiRecords } from "./localUiStore.js";
import { reviewCreateDraftAddress } from "./taskReviewDrafts.js";
import { createTaskReviewRepository, reviewAddress, reviewFailure } from "./taskReviewCache.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { openReviewCreateForm, openReviewPushForm } from "./workspaceReviewForm.js";
import { readWorkspaceReviewContext, boundSourceFacts, pendingReviewText, canPushSource, shortReviewRef, sourcePushDestination } from "./workspaceReviewState.js";

const taskOptions = (tasks) => tasks.map((task) => `<option value="${esc(task.id)}">#${Number(task.number)} ${esc(task.title)}</option>`).join("");
const taskForm = (tasks) => `<h3>Create or update review</h3><p class="sub">Save committed work from every directory on a task. Choosing another workspace replaces its previous review history.</p>
  <label>Task<select data-review-task>${taskOptions(tasks)}</select></label>
  ${tasks.length ? "" : '<p>No tasks yet. Create a task in the project first.</p>'}
  <p class="warn" data-workspace-review-error role="alert" hidden></p>
  <div class="row"><button class="btn" data-cancel-workspace-review>Cancel</button><button class="btn primary" data-save-workspace-review${tasks.length ? "" : " disabled"}>Save review</button></div>`;

async function saveWorkspaceReview(scope, workspaceId, callRpc) {
  const repository = createTaskReviewRepository({ ...scope, callRpc });
  const refreshed = await repository.refresh();
  const held = await readCached(reviewAddress(scope));
  if (!refreshed) throw new Error(held?.value?.error || "Could not read the review. Try again.");
  return repository.mutate("snapshot", { workspace_id: workspaceId, expected_version: held?.value?.review?.version || 0 });
}

async function openReviewTaskPicker(options) {
  const { deviceId, projectId, workspaceId, callRpc, navigate } = options;
  const record = await readTasksRecord(deviceId, projectId);
  const tasks = (record?.tasks || []).filter((task) => task.review_summary?.mode !== "pull_request");
  const modal = openModal({ dialogHtml: modalDialogHtml(taskForm(tasks), { className: "modal-create" }), onClose: options.onClosed });
  const select = modal.body.querySelector('[data-review-task]');
  const linked = tasks.find((task) => task.links?.workspace_ids?.includes(workspaceId));
  if (linked) select.value = linked.id;
  const button = modal.body.querySelector('[data-save-workspace-review]');
  const error = modal.body.querySelector('[data-workspace-review-error]');
  let busy = false;
  modal.body.querySelector('[data-cancel-workspace-review]').onclick = modal.close;
  button.onclick = async () => {
    if (busy || !select.value) return;
    busy = true;
    button.disabled = true;
    const taskId = select.value;
    const scope = { deviceId, projectId, taskId };
    try {
      await saveWorkspaceReview(scope, workspaceId, callRpc);
      modal.close();
      navigate?.({ name: "trackerTask", deviceId, projectId, taskId });
    } catch (failure) {
      error.textContent = reviewFailure(failure);
      error.hidden = false;
    } finally { busy = false; button.disabled = false; }
  };
  return modal;
}

async function reviewOptions(options) {
  if (options.callRpc) return options;
  const { contextFor } = await import("./deviceContexts.js");
  return { ...options, callRpc: (...asked) => {
    const context = contextFor(options.deviceId);
    if (!context) throw new Error("This machine is unavailable.");
    return context.rpc(...asked);
  } };
}

const activeForms = new Map();
function sharedWorkspaceForm(options, open) {
  const key = JSON.stringify([options.deviceId, options.projectId, options.workspaceId]);
  if (activeForms.has(key)) return activeForms.get(key);
  const promise = open({ ...options, onClosed: () => activeForms.delete(key) }).then((modal) => {
    if (!modal) activeForms.delete(key);
    return modal;
  }).catch((error) => { activeForms.delete(key); throw error; });
  activeForms.set(key, promise);
  return promise;
}

async function openConfiguredReview(options) {
  const configured = await reviewOptions(options);
  const context = await readWorkspaceReviewContext(options);
  if (canOpenPullRequest(context)) return openReviewCreateForm(configured);
  if (context.held?.review?.mode !== "pull_request" && context.support.get && context.support.snapshot) return openReviewTaskPicker(configured);
  return null;
}

export const openWorkspaceReview = (options) => sharedWorkspaceForm(options, openConfiguredReview);
const canOpenPullRequest = (context) => context.support.open &&
  (Boolean(context.createDraft?.submitted) || context.sources.some(({ directory }) => directory.is_git !== false));

const statusLabels = { open: "Open", approved: "Approved", changes_requested: "Changes requested", merged: "Merged", closed: "Closed" };
const reviewRoute = (options, taskId) => ({ name: "trackerTask", deviceId: options.deviceId, projectId: options.projectId, taskId });
const reviewHref = (options, taskId) => hashFromRoute(reviewRoute(options, taskId));

function sourceActionHtml(fact, support, terminal) {
  const { binding, differentBranch } = fact;
  const pending = pendingReviewText(fact);
  const destination = sourcePushDestination(fact);
  if (differentBranch) return switchBranchHtml(fact);
  if (support.push && !terminal && canPushSource(fact)) return `<button class="btn" type="button" data-review-push="${esc(binding.directory_id)}">Push · ${esc(pending)}<span class="workspace-review-destination">${esc(destination)}</span></button>`;
  return `<span>${esc(pending)}</span>`;
}

function switchBranchHtml({ binding, source }) {
  const current = source?.refs?.current;
  const branch = current?.name || current?.kind || source?.directory.branch;
  return `<p>Checked out on ${esc(branch)}. Review branch: <code>${esc(shortReviewRef(binding.dedicated_branch_ref))}</code></p><button class="btn" type="button" data-review-switch="${esc(binding.directory_id)}">Switch back to review branch</button>`;
}

function sourceHtml(fact, support, terminal) {
  const { binding, source } = fact;
  const label = source?.directory.name || binding.directory_id;
  const dirty = source?.status?.files?.length;
  return `<div class="workspace-review-publication"><strong>${esc(label)}</strong><span>${esc(shortReviewRef(binding.base_branch_ref))} ← ${esc(shortReviewRef(binding.dedicated_branch_ref))}</span>
    ${sourceActionHtml(fact, support, terminal)}${fact.sinceReview ? `<span>${esc(fact.sinceReview)}</span>` : ""}
    ${dirty > 0 ? `<span>${dirty} uncommitted file${dirty === 1 ? "" : "s"} excluded</span>` : ""}</div>`;
}

function entryIdentity(context) {
  const review = context.held?.review;
  const candidate = context.workspace?.active_review;
  const summary = candidate && candidate.workspace_id === (context.workspace?.workspace_id || context.workspace?.id) ? candidate : {};
  return { review, summary, task: context.held?.task || {} };
}

function snapshotHtml(review, summary) {
  const snapshot = review?.snapshots?.at(-1);
  const count = review?.snapshots?.length ?? summary.snapshot_count;
  return `${snapshot ? `<span>Snapshot ${snapshot.number}</span>` : ""}${count !== undefined ? `<span>${count} snapshot${count === 1 ? "" : "s"}</span>` : ""}`;
}

function linkedReviewHtml(context, options, taskId) {
  const { review, summary, task } = entryIdentity(context);
  const status = cachedReviewStatus(review, summary);
  const number = task.number || summary.task_number;
  const title = task.title || summary.title || "Review";
  const terminal = ["closed", "merged"].includes(status);
  return `<div class="workspace-review-heading"><h2>Changes</h2><a data-review-link href="${esc(reviewHref(options, taskId))}">${number ? `#${Number(number)} ` : ""}${esc(title)}</a>
      <span class="workspace-review-badge">${esc(statusLabels[status] || status)}</span>${snapshotHtml(review, summary)}${context.support.open ? resumeOpeningHtml(context.createDraft) : ""}</div>
      ${boundSourceFacts(context).map((fact) => sourceHtml(fact, context.support, terminal)).join("")}
      ${dispatchFailureHtml(context.held)}`;
}
const cachedReviewStatus = (review, summary) => summary.version > review?.version
  ? summary.status : review?.pull_request?.status || summary.status || "open";
const resumeOpeningHtml = (draft) => draft?.submitted
  ? `<button type="button" class="btn" data-workspace-review>${draft.dispatchFailed ? "Retry reviewer dispatch" : "Resume opening"}</button>` : "";
const dispatchFailureHtml = (held) => held?.reviewer_dispatch?.state === "failed"
  ? `<p class="warn">${esc(held.reviewer_dispatch.error)}</p>` : "";

const availableEntryAction = (context) => {
  if (canOpenPullRequest(context)) return "Open review";
  return context.support.get && context.support.snapshot ? "Create snapshot review" : "";
};

function entryHtml(context, options) {
  const { review, summary } = entryIdentity(context);
  const taskId = review?.task_id || summary?.task_id;
  if (taskId && review?.mode === "pull_request") return linkedReviewHtml(context, options, taskId);
  if (summary.task_id) return linkedReviewHtml(context, options, taskId);
  const action = availableEntryAction(context);
  return `<div class="workspace-review-heading"><h2>Changes</h2>${action ? `<button type="button" class="btn" data-workspace-review>${action}</button>` : ""}</div>`;
}

async function switchReviewBranch(options, fact) {
  await options.callRpc("git.checkout_ref", { workspace_id: options.workspaceId, source_id: fact.binding.source_id, full_ref: fact.binding.dedicated_branch_ref });
  const { refreshFeed } = await import("./taskFeed.js");
  const { contextFor } = await import("./deviceContexts.js");
  const entityId = fact.source.entityId;
  await deleteCached(["refs", "status", "log", "unpushed", "diff"].map((kind) => ({ deviceId: options.deviceId, entityId, kind })));
  if (contextFor(options.deviceId)) void refreshFeed(options.deviceId);
}

export function mountWorkspaceReviewEntry(host, options) {
  let disposed = false;
  let modal = null;
  let revision = 0;
  let html = null;
  async function paint() {
    const read = ++revision;
    const context = await readWorkspaceReviewContext(options);
    if (disposed || read !== revision) return;
    const next = entryHtml(context, options);
    if (html === next) { wirePublication(context); return; }
    html = next;
    replaceEntryHtml(host, next);
    const open = host.querySelector("[data-workspace-review]");
    if (open) open.onclick = async () => {
      modal = await openWorkspaceReview(options);
      if (disposed) modal?.close();
    };
    const link = host.querySelector("[data-review-link]");
    if (link && options.navigate) link.onclick = (event) => { event.preventDefault(); options.navigate(reviewRoute(options, context.held?.review?.task_id || context.workspace.active_review.task_id)); };
    wirePublication(context);
  }
  function wirePublication(context) {
    const facts = boundSourceFacts(context);
    host.querySelectorAll("[data-review-push]").forEach((button) => { button.onclick = async () => {
      const fact = facts.find((entry) => entry.binding.directory_id === button.dataset.reviewPush);
      modal = await sharedWorkspaceForm(await reviewOptions(options), (configured) => openReviewPushForm(configured, context.held.review, fact));
      if (disposed) modal?.close();
    }; });
    host.querySelectorAll("[data-review-switch]").forEach((button) => { button.onclick = async () => {
      button.disabled = true;
      try { await switchReviewBranch(await reviewOptions(options), facts.find((fact) => fact.binding.directory_id === button.dataset.reviewSwitch)); }
      catch (failure) { const error = document.createElement("p"); error.setAttribute("role", "alert"); error.textContent = reviewFailure(failure); host.appendChild(error); }
      finally { button.disabled = false; }
    }; });
  }
  const unwatch = subscribeCache({ deviceId: options.deviceId }, () => void paint());
  const unwatchDraft = subscribeUiRecords(reviewCreateDraftAddress(options), () => void paint());
  void paint();
  return { dispose() { disposed = true; unwatch(); unwatchDraft(); return modal?.close(); } };
}

function replaceEntryHtml(host, next) {
  const focused = host.contains(document.activeElement) ? document.activeElement : null;
  const selector = focused?.getAttributeNames().find((name) => name.startsWith("data-"));
  const focusValue = selector && focused.getAttribute(selector);
  host.classList.add("workspace-review-entry");
  host.innerHTML = next;
  if (selector) Array.from(host.querySelectorAll(`[${selector}]`)).find((field) => field.getAttribute(selector) === focusValue)?.focus();
}
