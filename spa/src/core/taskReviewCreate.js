// Ordinary tasks keep their existing review creation controls. The task page
// switches to the dedicated template when review metadata reaches its cache.
import { subscribeCache } from "./localCache.js";
import { createTaskReviewRepository } from "./taskReviewCache.js";
import { readReviewSupport, reviewSupportAddress } from "./taskReviewSupport.js";
import { mountTaskReviewControls } from "./taskReviewControls.js";

export function mountTaskReviewCreate(host, options) {
  const { deviceId } = options;
  let disposed = false;
  let controls = null;
  let supportKey = "";
  let pending = Promise.resolve();
  const repository = createTaskReviewRepository({ ...options, active: () => !disposed });
  const workspaces = () => options.workspaces?.() || [];
  host.classList.add("task-review-create");
  host.innerHTML = '<header><h2>Review</h2><button type="button" class="btn" data-review-refresh>Refresh</button></header><div data-review-controls></div>';
  host.querySelector('[data-review-refresh]').onclick = () => void repository.refresh();
  async function hydrate() {
    const support = await readReviewSupport(deviceId);
    if (disposed) return;
    host.hidden = !support.get;
    const key = JSON.stringify(support);
    if (key === supportKey) return;
    supportKey = key;
    controls?.dispose();
    controls = mountTaskReviewControls(host.querySelector('[data-review-controls]'), {
      ...options, review: null, snapshot: null, support, repository, workspaces: workspaces(),
      onSaved: () => repository.refresh(),
    });
  }
  const paint = () => { pending = hydrate(); return pending; };
  const unwatch = subscribeCache(reviewSupportAddress(deviceId), () => void paint().then(() => repository.refresh()));
  const ready = paint();
  void ready.then(() => !disposed && repository.refresh());
  return {
    ready, whenPainted: () => pending,
    feedMoved() { controls?.update(null, workspaces()); },
    refresh: () => repository.refresh(),
    openAnchor: async () => false,
    reply: async () => {},
    dispose() { disposed = true; unwatch(); controls?.dispose(); },
  };
}
