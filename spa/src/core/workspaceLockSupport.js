// The greeted lock capability is cached per device, like the workspace rows.
import { mergeCachedAtomically, readCached, subscribeCache } from "./localCache.js";
export const WORKSPACE_LOCK_SUPPORT_KIND = "workspace-lock-support";
const addressOf = (deviceId) => ({ deviceId, entityId: "", kind: WORKSPACE_LOCK_SUPPORT_KIND });
export function rememberWorkspaceLockSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const setLocked = capabilities?.workspaces?.setLocked === true;
  return mergeCachedAtomically(addressOf(deviceId), (held) => held?.setLocked === setLocked ? null : { setLocked });
}
export function watchWorkspaceLockSupport(deviceId, onSupport) {
  let disposed = false;
  let reading = 0;
  const read = async () => {
    const token = ++reading;
    const record = await readCached(addressOf(deviceId));
    if (!disposed && token === reading) onSupport(record?.value?.setLocked === true);
  };
  const stop = subscribeCache({ deviceId }, (address) => {
    if (address?.kind === WORKSPACE_LOCK_SUPPORT_KIND) void read();
  });
  void read();
  return () => { disposed = true; stop(); };
}
