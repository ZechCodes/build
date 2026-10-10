// A settled PR retains its workspace until the reader explicitly reclaims it.
// The existing cached lock is authoritative; the bridge checks it again.
import { readCached, subscribeCache } from "./localCache.js";
import { workspaceSettingsAddress } from "./settingsRecords.js";
import { settledMerge } from "./taskReviewMerge.js";

export function mountTaskReviewReclaim(host, options) {
  const { deviceId, callRpc, keepReadingPlace = (paint) => paint() } = options;
  const workspaceId = options.review.workspace_id;
  const listAddress = { deviceId, entityId: "", kind: "workspaces" };
  const detailAddress = workspaceSettingsAddress(deviceId, workspaceId);
  let record = options.record;
  let workspace = null;
  let busy = false;
  let error = "";
  let disposed = false;
  let readSerial = 0;
  // Evaluate cached obligations, including later results that settle retained
  // interruption rows. The historical intent label does not decide Reclaim.
  const available = () => Boolean(options.support?.pullRequests && workspace && settledMerge(record));

  function paint() {
    if (disposed) return;
    keepReadingPlace(() => {
      if (!available()) { host.innerHTML = ""; return; }
      if (!host.querySelector('[data-review-reclaim]')) host.innerHTML = `<div class="task-review-reclaim">
        <p data-review-retention></p><button class="btn" type="button" data-review-reclaim>Reclaim workspace</button>
        <p class="warn" data-review-delete-error role="alert" hidden></p></div>`;
      const button = host.querySelector('[data-review-reclaim]');
      const locked = workspace.locked === true;
      button.disabled = busy || locked || typeof workspace.locked !== "boolean";
      button.title = locked ? "Unlock the workspace to delete it" : "";
      button.textContent = busy ? "Reclaiming…" : "Reclaim workspace";
      host.querySelector('[data-review-retention]').textContent = locked ? "Merged · Workspace locked" : "Merged · Workspace retained";
      const warning = host.querySelector('[data-review-delete-error]');
      warning.textContent = error;
      warning.hidden = !error;
      button.onclick = () => void reclaim();
    });
  }

  async function hydrate() {
    const serial = ++readSerial;
    const [list, detail] = await Promise.all([readCached(listAddress), readCached(detailAddress)]);
    if (disposed || serial !== readSerial) return;
    workspace = list ? list.value?.find((row) => (row.workspace_id || row.id) === workspaceId) : detail?.value;
    paint();
  }

  async function reclaim() {
    if (busy || disposed || workspace?.locked !== false || !available()) return;
    busy = true; error = ""; paint();
    try {
      await callRpc("workspace.reclaim", { workspace_id: workspaceId });
      await options.onReclaimed?.();
    } catch (failure) { error = failure?.message || String(failure); }
    finally { busy = false; paint(); }
  }
  const stopList = subscribeCache(listAddress, () => void hydrate());
  const stopDetail = subscribeCache(detailAddress, () => void hydrate());
  const ready = hydrate();
  return {
    ready,
    update(nextRecord) { record = nextRecord; paint(); },
    dispose() { disposed = true; stopList(); stopDetail(); },
  };
}
