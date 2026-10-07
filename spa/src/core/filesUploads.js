// Connect the cache-rendered Files affordances to device-owned upload state.
import { contextFor } from "./deviceContexts.js";
import { deleteCached, subscribeCache } from "./localCache.js";
import { readFileUploadSupport, fileUploadSupportAddress } from "./fileUploadSupport.js";
import { fileUploadRpc } from "./fileUploadRpc.js";
import { uploadsFor } from "./fileUploads.js";
import { mountFileUploadTray } from "./fileUploadTray.js";
import { mountFilesUploadActions } from "./filesUploadActions.js";

const sameScope = (left, right) => Object.keys(left).length === Object.keys(right).length
  && Object.entries(left).every(([key, value]) => right[key] === value);

export function mountFilesUploads({ treeEl, viewerEl, roots, keyOf, tree, callRpc, deviceId, listingAddress }) {
  let disposed = false;
  const refreshed = new Set();
  const rpc = fileUploadRpc(contextFor(deviceId), callRpc);
  const uploads = uploadsFor(deviceId, { callRpc: rpc });
  // Ignore-file writes need their own invalidation: a files push is not
  // guaranteed. Deleting the held parent also keeps the next mount fresh when
  // a folder creation answers after this view has gone away.
  const invalidate = async (root, parent) => {
    const address = listingAddress(root, parent);
    if (address) await deleteCached([address]);
  };
  const refresh = async (root, parent, path) => {
    await invalidate(root, parent);
    if (disposed) return;
    const current = roots.find((candidate) => candidate.id === root.id && sameScope(candidate.scope, root.scope));
    if (!current) return;
    await tree.reveal(keyOf(current, path));
    if (!disposed) tree.relist([parent], root.id);
  };
  const actions = mountFilesUploadActions(treeEl, {
    roots, capabilities: { uploads: false, createDirectory: false }, uploads, callRpc: rpc,
    onCreated: (root, parent, path) => void refresh(root, parent, path),
    onFinished: ({ rootId, scope, parent }) => invalidate({ id: rootId, scope }, parent),
  });
  const readSupport = async () => {
    const support = await readFileUploadSupport(deviceId);
    if (!disposed) { actions.setCapabilities(support); stopTray.setCapabilities(support); }
  };
  const stopSupport = deviceId ? subscribeCache(fileUploadSupportAddress(deviceId), () => void readSupport()) : () => {};
  void readSupport();
  const onUploads = ({ recent }) => {
    for (const item of recent) {
      const completed = `${item.id}:${item.finishedAt}`;
      if (item.status !== "finished" || refreshed.has(completed)) continue;
      const root = roots.find((candidate) => candidate.id === item.rootId && sameScope(candidate.scope, item.scope));
      if (!root) continue;
      refreshed.add(completed);
      void refresh(root, item.parent, item.path);
    }
  };
  const stopUploads = uploads.subscribe(onUploads);
  onUploads(uploads.snapshot());
  const stopTray = mountFileUploadTray(viewerEl, { uploads, capabilities: { uploads: false } });
  return {
    dispose() { disposed = true; stopSupport(); stopUploads(); stopTray(); actions.dispose(); },
  };
}
