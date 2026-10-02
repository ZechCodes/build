import "../styles/taskReview.css";
import { esc } from "./text.js";
import { readCached, subscribeCache } from "./localCache.js";
import { readTasksRecord } from "./trackerCache.js";
import { readReviewSupport, reviewSupportAddress } from "./taskReviewSupport.js";
import { createTaskReviewRepository, reviewAddress, reviewFailure } from "./taskReviewCache.js";
import { modalDialogHtml, openModal } from "./modal.js";

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
  const tasks = record?.tasks || [];
  const modal = openModal({ dialogHtml: modalDialogHtml(taskForm(tasks), { className: "modal-create" }) });
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

export function mountWorkspaceReviewEntry(host, options) {
  let disposed = false;
  let modal = null;
  async function paint() {
    const support = await readReviewSupport(options.deviceId);
    if (disposed) return;
    host.hidden = !support.get || !support.snapshot;
    if (host.hidden || host.querySelector('button')) return;
    host.className = "workspace-review-entry";
    host.innerHTML = '<button class="btn" data-workspace-review>Create / Update review</button>';
    host.querySelector('button').onclick = async () => {
      modal = await openReviewTaskPicker(options);
      if (disposed) modal.close();
    };
  }
  const unwatch = subscribeCache(reviewSupportAddress(options.deviceId), () => void paint());
  void paint();
  return { dispose() { disposed = true; unwatch(); modal?.close(); } };
}
