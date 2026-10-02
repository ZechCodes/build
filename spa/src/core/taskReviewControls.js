// Snapshot edits and explicit completion. Drafts belong to task/snapshot in
// build-ui; a stale server version refreshes metadata and leaves them intact.
import { esc } from "./text.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { reviewFailure } from "./taskReviewCache.js";
import { fieldTraits } from "./fieldTraits.js";

const workspaceId = (workspace) => workspace.workspace_id || workspace.id;
const optionsHtml = (workspaces, selected) => {
  const rows = workspaces.map((workspace) => ({ id: workspaceId(workspace), name: workspace.name || workspace.label || workspaceId(workspace) }));
  if (selected && !rows.some((row) => row.id === selected)) rows.push({ id: selected, name: selected });
  return '<option value="">Choose a workspace</option>' + rows.map((row) =>
    `<option value="${esc(row.id)}">${esc(row.name)}</option>`).join("");
};

const baseField = (directory) => `<label class="task-review-base">${esc(directory.name)} base
  <input data-review-base="${esc(directory.id)}" type="text" ${fieldTraits("identifier")} placeholder="Automatic · branch, tag or commit" list="review-bases-${esc(directory.id)}" />
  <datalist id="review-bases-${esc(directory.id)}">${[directory.base?.name, directory.branch].filter(Boolean).map((name) => `<option value="${esc(name)}"></option>`).join("")}</datalist></label>`;

const savedBaseOverrides = (snapshot) => Object.fromEntries((snapshot?.directories || [])
  .filter((directory) => directory.base?.kind === "override")
  .map((directory) => [directory.id, directory.base.name || directory.base.oid]));

function controlsHtml(review, snapshot, support) {
  return `<div class="task-review-actions">
    ${support.snapshot ? `<details data-review-snapshot-form><summary>${review ? "Update review" : "Create review"}</summary>
      <form data-review-save><label>Workspace<select data-review-workspace></select></label>
      <div data-review-bases>${(snapshot?.directories || []).filter((directory) => directory.is_git).map(baseField).join("")}</div>
      <p class="sub">Save committed work from every directory. A different workspace replaces this review's history.</p>
      <button class="btn primary" data-review-update type="submit">Save snapshot</button></form></details>` : ""}
    ${support.complete && review?.state === "open" ? `<details><summary>Mark complete</summary><form data-review-complete>
      <label>What was done?<textarea data-review-description rows="2" ${fieldTraits("prose")} placeholder="For example: merged API to dev; pushed web" required></textarea></label>
      <button class="btn primary" type="submit">Mark complete</button></form></details>` : ""}
    <p class="warn" data-review-action-error role="alert" hidden></p></div>`;
}

export function mountTaskReviewControls(host, options) {
  const { deviceId, projectId, taskId, snapshot, repository, support, onSaved, onTaskChanged } = options;
  let review = options.review;
  let workspaces = options.workspaces;
  let draft = { workspace: review?.workspace_id || "", bases: savedBaseOverrides(snapshot), description: "" };
  let busy = false;
  let disposed = false;
  host.innerHTML = controlsHtml(review, snapshot, support);
  const errorNode = host.querySelector('[data-review-action-error]');
  const workspace = host.querySelector('[data-review-workspace]');
  const description = host.querySelector('[data-review-description]');
  const bases = [...host.querySelectorAll('[data-review-base]')];
  function paintBases() {
    for (const input of bases) {
      input.disabled = draft.workspace !== review?.workspace_id;
      const wanted = draft.bases[input.dataset.reviewBase] || "";
      if (input.value !== wanted) input.value = wanted;
    }
  }
  function paint(saved) {
    if (disposed) return;
    draft = { ...draft, ...saved, bases: { ...draft.bases, ...saved?.bases } };
    if (workspace) workspace.value = draft.workspace;
    if (description && description.value !== draft.description) description.value = draft.description;
    paintBases();
  }
  function paintWorkspaces() {
    if (!workspace) return;
    workspace.innerHTML = optionsHtml(workspaces, draft.workspace);
    workspace.value = draft.workspace;
    paintBases();
  }
  const state = watchUiState(uiAddress({ deviceId, entityId: projectId, view: "task-review-actions", kind: "draft",
    sub: JSON.stringify([taskId, snapshot?.id || "new"]) }), paint, { debounceMs: 180 });
  const edit = (changes) => { draft = { ...draft, ...changes }; state.schedule(draft); };
  if (workspace) workspace.onchange = () => { edit({ workspace: workspace.value }); paintBases(); };
  if (description) description.oninput = () => edit({ description: description.value });
  for (const input of bases) input.oninput = () => edit({ bases: { ...draft.bases, [input.dataset.reviewBase]: input.value } });
  function showError(message) { errorNode.textContent = message; errorNode.hidden = !message; }
  async function submit(verb, params) {
    if (busy) return;
    busy = true;
    showError("");
    host.querySelectorAll('button').forEach((button) => { button.disabled = true; });
    try {
      await state.flush();
      await repository.mutate(verb, { ...params, expected_version: review?.version || 0 });
      await onSaved?.(verb);
      await onTaskChanged?.();
    } catch (error) { if (!disposed) showError(reviewFailure(error)); }
    finally {
      busy = false;
      host.querySelectorAll('button').forEach((button) => { button.disabled = false; });
    }
  }
  function saveSnapshot(event) {
    event.preventDefault();
    if (!draft.workspace) return showError("Choose the workspace to review.");
    const overrides = Object.fromEntries(Object.entries(draft.bases).filter(([, value]) => value.trim()).map(([id, value]) => [id, value.trim()]));
    // A different workspace has different directory IDs; its initial snapshot
    // uses that manifest's bases, then offers its own per-directory fields.
    void submit("snapshot", { workspace_id: draft.workspace, base_overrides: draft.workspace === review?.workspace_id ? overrides : {} });
  }
  function complete(event) {
    event.preventDefault();
    const text = draft.description.trim();
    if (!text) return showError("Describe what was done.");
    if (new TextEncoder().encode(text).length > 2000) return showError("Keep the description within 2,000 bytes.");
    void submit("complete", { description: text });
  }
  const saveForm = host.querySelector('[data-review-save]');
  const completeForm = host.querySelector('[data-review-complete]');
  if (saveForm) saveForm.onsubmit = saveSnapshot;
  if (completeForm) completeForm.onsubmit = complete;
  paintWorkspaces();
  paint(draft);
  return {
    update(nextReview, nextWorkspaces) { review = nextReview; workspaces = nextWorkspaces; paintWorkspaces(); },
    dispose() { disposed = true; state.dispose(); },
  };
}
