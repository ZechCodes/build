// A kept editor on the task rail. Cache announcements paint graph facts;
// controls keep their nodes, selection and query during pushes and refusals.
import { subscribeCache } from "./localCache.js";
import { readTasksRecord, tasksAddress } from "./trackerCache.js";
import { createTaskWaitingOnActions } from "./taskWaitingOnActions.js";
import { readTaskWaitingOnSupport, taskWaitingOnSupportAddress } from "./taskWaitingOnSupport.js";
import { readTaskWaitingOnBlocks, taskWaitingOnBlocksAddress, taskWaitingOnGenerationAddress } from "./taskWaitingOnCache.js";
import { blockerChoices, removalConsequence, waitingOnEdges, waitingOnNotice } from "./taskWaitingOnModel.js";
import { blockerOptionsHtml, dependencyRowsHtml, waitingOnFrameHtml } from "./taskWaitingOnRender.js";
import { messageOf } from "./text.js";

export function mountTaskWaitingOn(host, { deviceId, projectId, taskId, call, onEdgesChanged = () => {} }) {
  const actions = createTaskWaitingOnActions({ deviceId, projectId, taskId, call });
  const place = { deviceId, projectId };
  let task = null, tasks = [], pages = null;
  let supported = false, busy = false, disposed = false, removing = null;
  host.innerHTML = waitingOnFrameHtml();
  host.hidden = true;
  const find = (selector) => host.querySelector(selector);
  const input = find("[data-waiting-search]");
  const select = find("[data-waiting-choice]");
  let optionsPainted = "";
  let forwardPainted = "", reversePainted = "";
  let detailSignature = "", pagesRead = 0;

  function paintChoices() {
    const html = blockerOptionsHtml(blockerChoices(task, tasks, projectId, input.value));
    if (html !== optionsPainted) {
      const selection = select.value;
      select.innerHTML = html;
      select.value = selection;
      optionsPainted = html;
    }
    find("[data-waiting-add]").disabled = busy || !select.value || waitingOnEdges(task).length >= 20;
  }

  function paintForwardRows() {
    const edges = waitingOnEdges(task);
    const forward = dependencyRowsHtml(edges, place, true);
    if (forward !== forwardPainted) { find("[data-waiting-list]").innerHTML = forward; forwardPainted = forward; }
    find("[data-waiting-empty]").hidden = Boolean(edges.length);
  }

  function paintReverseRows() {
    const dependencies = task?.dependencies || {};
    const edges = pages?.blocks || dependencies.blocks || [];
    const reverse = dependencyRowsHtml(edges, place);
    if (reverse !== reversePainted) { find("[data-blocks-list]").innerHTML = reverse; reversePainted = reverse; }
    find("[data-blocks-empty]").hidden = Boolean(edges.length);
    find("[data-blocks-count]").textContent = dependencies.blocks_count ? `(${dependencies.blocks_count})` : "";
    find("[data-blocks-more]").hidden = !nextCursor();
  }

  function nextCursor() {
    return pages ? pages.next_cursor : task?.dependencies?.blocks_next_cursor;
  }

  function paintDelivery() {
    const notice = waitingOnNotice(task);
    find("[data-waiting-notice]").textContent = notice;
    find("[data-waiting-notice]").hidden = !notice;
    const error = task?.deferred_assignment?.error || "";
    find("[data-waiting-delivery-error]").textContent = error;
    find("[data-waiting-delivery-error]").hidden = !error;
    find("[data-waiting-retry]").hidden = task?.deferred_assignment?.state !== "failed";
  }

  function paintPending() {
    host.querySelectorAll("button, select").forEach((control) => { control.disabled = busy; });
    paintChoices();
    const confirm = find("[data-waiting-confirm]");
    confirm.hidden = !removing;
    find("[data-waiting-consequence]").textContent = removing ? removalConsequence(task, removing) : "";
  }

  function paint() {
    if (disposed) return;
    host.hidden = !supported;
    if (!supported) return;
    paintForwardRows();
    paintReverseRows();
    paintDelivery();
    paintPending();
  }

  async function perform(operation, onSuccess = () => {}) {
    if (busy || !supported) return;
    busy = true;
    find("[data-waiting-error]").hidden = true;
    paintPending();
    try {
      await operation();
      if (!disposed) onSuccess();
    } catch (error) {
      if (disposed) return;
      find("[data-waiting-error]").textContent = messageOf(error?.message || error);
      find("[data-waiting-error]").hidden = false;
    } finally {
      busy = false;
      if (!disposed) paintPending();
    }
  }

  input.oninput = paintChoices;
  select.onchange = paintChoices;
  find("[data-waiting-picker]").onsubmit = (event) => {
    event.preventDefault();
    const id = select.value;
    if (id) void perform(() => actions.add([id]), onEdgesChanged);
  };
  host.onclick = (event) => {
    const button = event.target.closest("[data-waiting-remove]");
    if (!button || busy || !supported) return;
    removing = button.dataset.waitingRemove;
    paintPending();
  };
  find("[data-waiting-cancel]").onclick = () => { removing = null; paintPending(); };
  find("[data-waiting-confirm-remove]").onclick = () => {
    const id = removing;
    if (id) void perform(() => actions.remove([id]), () => { removing = null; onEdgesChanged(); });
  };
  find("[data-blocks-more]").onclick = () => {
    const cursor = nextCursor();
    if (cursor) void perform(() => actions.blocks({ cursor }));
  };
  find("[data-waiting-retry]").onclick = () => {
    const id = task?.deferred_assignment?.id;
    if (id) void perform(() => actions.retry(id));
  };

  async function readSupport() {
    const value = await readTaskWaitingOnSupport(deviceId);
    if (disposed) return;
    supported = value;
    paint();
  }
  async function readChoices() {
    const value = await readTasksRecord(deviceId, projectId);
    if (disposed) return;
    tasks = value?.tasks || [];
    paintChoices();
  }
  async function readPages() {
    const serial = ++pagesRead;
    const value = await readTaskWaitingOnBlocks(deviceId, projectId, taskId);
    if (disposed || serial !== pagesRead) return;
    pages = value;
    paint();
  }
  const stops = [
    subscribeCache(taskWaitingOnSupportAddress(deviceId), readSupport),
    subscribeCache(tasksAddress(deviceId, projectId), readChoices),
    subscribeCache(taskWaitingOnBlocksAddress(deviceId, projectId, taskId), readPages),
    subscribeCache(taskWaitingOnGenerationAddress(deviceId, projectId), readPages),
  ];
  void Promise.all([readSupport(), readChoices(), readPages()]);
  return {
    update(value) {
      const signature = JSON.stringify(value?.dependencies || null);
      task = value;
      if (signature !== detailSignature) {
        detailSignature = signature;
        pages = null;
        void readPages();
      }
      paint();
    },
    dispose() { disposed = true; stops.forEach((stop) => stop()); },
  };
}
