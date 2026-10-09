// Lifecycle controls write through the shared repository and render cached
// reviews. Restored drafts never execute an operation.
import { esc } from "./text.js";
import { fieldTraits } from "./fieldTraits.js";
import { reviewFailure } from "./taskReviewCache.js";
import { reviewSupportFor } from "./taskReviewSupport.js";
import { watchReviewActionDraft } from "./taskReviewDrafts.js";
import { mountTaskReviewReclaim } from "./taskReviewReclaim.js";

const activeReview = (review) => ["open", "approved", "changes_requested"].includes(review?.pull_request?.status);
const statusLabel = (review) => {
  const text = (review?.pull_request?.status || "open").replaceAll("_", " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
};

function closeHtml(review, support) {
  return support.close && activeReview(review) ? `<details data-review-close-sheet><summary>Close review</summary>
    <form data-review-close><label>Why close this review?<textarea data-review-close-description rows="2" ${fieldTraits("prose")} required></textarea></label>
    <button class="btn" type="submit">Close review</button></form></details>` : "";
}

function lifecycleHtml(review, support) {
  const close = closeHtml(review, support);
  const reopen = support.reopen && review?.pull_request?.status === "closed"
    ? '<button class="btn" type="button" data-review-reopen>Reopen review</button>' : "";
  const advanced = support.refresh ? '<details data-review-advanced><summary>Advanced</summary><button class="btn" type="button" data-review-repair>Refresh published review</button></details>' : "";
  const readOnly = !close && !reopen && !advanced && !support.merge
    ? `<p class="sub" data-review-read-only role="status">Pull request: ${esc(statusLabel(review))} · Read-only</p>` : "";
  return `<div class="task-review-actions">${close}${reopen}${advanced}${readOnly}
    <p class="warn" data-review-lifecycle-error role="alert" hidden></p></div>`;
}

export function mountTaskReviewLifecycle(host, options) {
  let review = options.review;
  let record = options.record || { review };
  const support = reviewSupportFor(review, options.support);
  const { repository, keepReadingPlace = (paint) => paint() } = options;
  let draft = { description: "" };
  let hydrated = false;
  let busy = false;
  let disposed = false;
  let error = "";
  let rendered = "";
  host.innerHTML = '<div data-review-lifecycle></div><div data-review-reclaim-host></div>';
  const content = host.querySelector('[data-review-lifecycle]');
  const reclaim = mountTaskReviewReclaim(host.querySelector('[data-review-reclaim-host]'), { ...options, record });

  function paint() {
    if (disposed) return;
    keepReadingPlace(() => {
      const html = lifecycleHtml(review, support);
      if (rendered !== html) { content.innerHTML = html; rendered = html; wire(); }
      const field = content.querySelector('[data-review-close-description]');
      if (field && field.value !== draft.description) field.value = draft.description;
      content.querySelectorAll('button').forEach((button) => { button.disabled = busy || !hydrated; });
      const warning = content.querySelector('[data-review-lifecycle-error]');
      warning.textContent = error;
      warning.hidden = !error;
    });
  }
  const state = watchReviewActionDraft({ ...options, snapshotId: options.snapshot?.id }, "close", (saved) => {
    if (disposed) return;
    draft = { description: "", ...saved }; paint();
  });
  async function submit(verb, params = {}) {
    if (disposed || busy || !hydrated || !support[verb]) return;
    busy = true; error = ""; paint();
    try {
      await state.flush();
      if (disposed) return;
      await repository.mutate(verb, { ...params, expected_version: review.version });
      await options.onTaskChanged?.();
    } catch (failure) { if (!disposed) error = reviewFailure(failure); }
    finally { busy = false; paint(); }
  }
  function close(event) {
    event.preventDefault();
    if (!activeReview(review)) return;
    const description = draft.description.trim();
    if (!description) { error = "Describe why this review is closing."; paint(); return; }
    if (new TextEncoder().encode(description).length > 2000) { error = "Keep the description within 2,000 bytes."; paint(); return; }
    void submit("close", { description });
  }
  function wire() {
    const field = content.querySelector('[data-review-close-description]');
    if (field) field.oninput = () => { draft = { description: field.value }; state.schedule(draft); };
    const form = content.querySelector('[data-review-close]');
    if (form) form.onsubmit = close;
    const reopen = content.querySelector('[data-review-reopen]');
    if (reopen) reopen.onclick = () => { if (review?.pull_request?.status === "closed") void submit("reopen"); };
    const repair = content.querySelector('[data-review-repair]');
    if (repair) repair.onclick = () => void submit("refresh");
  }
  paint();
  const ready = Promise.all([state.ready, reclaim.ready]).then(() => { hydrated = true; paint(); });
  return {
    ready,
    update(nextReview, nextRecord) { review = nextReview; record = nextRecord || { review }; paint(); reclaim.update(record); },
    dispose() { disposed = true; state.dispose(); reclaim.dispose(); },
  };
}
