// Dependency markup is painted from saved digests. All human and wire text is
// escaped, including task titles, ids, and delivery errors.
import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { edgeResolutionLabel } from "./taskWaitingOnModel.js";

export function dependencyRowsHtml(edges, place, removable = false) {
  return edges.map((edge) => {
    const href = hashFromRoute({ name: "trackerTask", ...place, taskId: edge.task_id });
    const remove = removable ? `<button class="btn" type="button" data-waiting-remove="${esc(edge.task_id)}" aria-label="Remove Waiting on #${esc(edge.number)}">Remove</button>` : "";
    return `<li class="task-dependency"><div><a href="${esc(href)}">#${esc(edge.number)} ${esc(edge.title)}</a><span class="task-dependency-resolution" data-resolved-by="${esc(edge.resolved_by || "")}">${esc(edgeResolutionLabel(edge))}</span></div>${remove}</li>`;
  }).join("");
}

export const blockerOptionsHtml = (choices) => '<option value="">Choose a task</option>' + choices.map((task) =>
  `<option value="${esc(task.id || task.task_id)}">#${esc(task.number)} ${esc(task.title)}</option>`).join("");

export const waitingOnFrameHtml = () => `<section class="task-rail-section task-waiting-section">
  <h2>Waiting on</h2>
  <ul class="task-dependencies" data-waiting-list></ul>
  <p class="sub" data-waiting-empty>No prerequisites.</p>
  <p class="sub" data-waiting-notice hidden></p>
  <p class="sub" data-waiting-delivery-error hidden></p>
  <button class="btn" type="button" data-waiting-retry hidden>Retry saved assignment</button>
  <form data-waiting-picker>
    <label class="sr-only" for="task-waiting-search">Search tasks</label>
    <input id="task-waiting-search" type="text" data-waiting-search aria-label="Search tasks" placeholder="Search by number or title">
    <label class="sr-only" for="task-waiting-choice">Prerequisite task</label>
    <select id="task-waiting-choice" data-waiting-choice aria-label="Prerequisite task"></select>
    <button class="btn" type="submit" data-waiting-add>Add prerequisite</button>
  </form>
  <div data-waiting-confirm hidden><p class="sub" data-waiting-consequence></p>
    <div class="task-waiting-confirm-actions"><button class="btn" type="button" data-waiting-confirm-remove>Remove link</button><button class="btn" type="button" data-waiting-cancel>Cancel</button></div>
  </div>
  <p class="sub task-waiting-error" data-waiting-error role="status" hidden></p>
</section>
<section class="task-rail-section task-waiting-section">
  <h2>Blocks <span data-blocks-count></span></h2>
  <ul class="task-dependencies" data-blocks-list></ul>
  <p class="sub" data-blocks-empty>No dependent tasks.</p>
  <button class="btn" type="button" data-blocks-more hidden>Show more dependent tasks</button>
</section>`;
