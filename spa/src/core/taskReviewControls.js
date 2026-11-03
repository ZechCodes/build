// Legacy reviews are read-only, with an explicit close action under More.
import { reviewFailure } from "./taskReviewCache.js";
import { reviewSupportFor } from "./taskReviewSupport.js";
import { mountTaskReviewLifecycle } from "./taskReviewLifecycle.js";
import { confirmAction } from "./confirm.js";

export function mountTaskReviewControls(host, options) {
  if (options.review?.mode === "pull_request") return mountTaskReviewLifecycle(host, options);
  let review = options.review;
  let busy = false;
  let disposed = false;
  const support = reviewSupportFor(review, options.support);
  host.innerHTML = `<div class="task-review-actions">${support.complete && review?.state === "open"
    ? '<button class="btn" type="button" data-review-close-legacy>Close legacy review</button>' : ""}<p class="warn" data-review-action-error role="alert" hidden></p></div>`;
  const button = host.querySelector('[data-review-close-legacy]');
  const error = host.querySelector('[data-review-action-error]');
  async function closeLegacy() {
    if (busy || disposed) return;
    busy = true;
    button.disabled = true;
    try {
      const confirmed = await confirmAction({ title: "Close legacy review?", intro: "The saved review stays readable. Request a review from its workspace to continue reviewing.", confirmLabel: "Close legacy review" });
      if (!confirmed || disposed) return;
      error.hidden = true;
      await options.repository.mutate("complete", { expected_version: review.version, description: "Closed legacy review" });
      await options.onSaved?.("complete");
      await options.onTaskChanged?.();
    } catch (failure) { if (!disposed) { error.textContent = reviewFailure(failure); error.hidden = false; } }
    finally { busy = false; if (!disposed) button.disabled = false; }
  }
  if (button) button.onclick = () => void closeLegacy();
  return { update(nextReview) { review = nextReview; }, dispose() { disposed = true; } };
}
