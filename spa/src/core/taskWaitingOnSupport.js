// Greetings persist this rendering contract. Dispatch checks the current
// greeting separately: a cached capability can outlive its bridge release.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const taskWaitingOnSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "task-waiting-on-support" });

export function rememberTaskWaitingOnSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const waitingOn = capabilities?.tasks?.waitingOn === true;
  return mergeCachedAtomically(taskWaitingOnSupportAddress(deviceId), (held) =>
    !current() || held?.waitingOn === waitingOn ? null : { waitingOn });
}

export async function readTaskWaitingOnSupport(deviceId) {
  if (!deviceId) return false;
  return (await readCached(taskWaitingOnSupportAddress(deviceId)))?.value?.waitingOn === true;
}
