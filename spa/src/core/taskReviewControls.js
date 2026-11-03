// Explicit completion for existing snapshot reviews. Drafts belong to task/snapshot in
// build-ui; a stale server version refreshes metadata and leaves them intact.
import { uiAddress, watchUiState } from "./localUiState.js";
import { reviewFailure } from "./taskReviewCache.js";
import { reviewSupportFor } from "./taskReviewSupport.js";
import { fieldTraits } from "./fieldTraits.js";
import { mountTaskReviewLifecycle } from "./taskReviewLifecycle.js";

function controlsHtml(review, support) {
  return `<div class="task-review-actions">
    ${support.complete && review?.state === "open" ? `<details><summary>Mark complete</summary><form data-review-complete>
      <label>What was done?<textarea data-review-description rows="2" ${fieldTraits("prose")} placeholder="For example: merged API to dev; pushed web" required></textarea></label>
      <button class="btn primary" type="submit">Mark complete</button></form></details>` : ""}
    <p class="warn" data-review-action-error role="alert" hidden></p></div>`;
}

export function mountTaskReviewControls(host, options) {
  const { deviceId, projectId, taskId, snapshot, repository, onSaved, onTaskChanged, keepReadingPlace = (paint) => paint() } = options;
  let review = options.review;
  if (review?.mode === "pull_request") return mountTaskReviewLifecycle(host, options);
  const support = reviewSupportFor(review, options.support);
  let draft = { description: "" };
  let busy = false;
  let disposed = false;
  host.innerHTML = controlsHtml(review, support);
  const errorNode = host.querySelector('[data-review-action-error]');
  const description = host.querySelector('[data-review-description]');
  function paint(saved) {
    keepReadingPlace(() => {
      if (disposed) return;
      draft = { ...draft, ...saved };
      if (description && description.value !== draft.description) description.value = draft.description;
    });
  }
  const state = watchUiState(uiAddress({ deviceId, entityId: projectId, view: "task-review-actions", kind: "draft",
    sub: JSON.stringify([taskId, snapshot?.id || "new"]) }), paint, { debounceMs: 180 });
  const edit = (changes) => { draft = { ...draft, ...changes }; state.schedule(draft); };
  if (description) description.oninput = () => edit({ description: description.value });
  function showError(message) {
    keepReadingPlace(() => { errorNode.textContent = message; errorNode.hidden = !message; });
  }
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
  function complete(event) {
    event.preventDefault();
    const text = draft.description.trim();
    if (!text) return showError("Describe what was done.");
    if (new TextEncoder().encode(text).length > 2000) return showError("Keep the description within 2,000 bytes.");
    void submit("complete", { description: text });
  }
  const completeForm = host.querySelector('[data-review-complete]');
  if (completeForm) completeForm.onsubmit = complete;
  paint(draft);
  return {
    update(nextReview) { review = nextReview; },
    dispose() { disposed = true; state.dispose(); },
  };
}
