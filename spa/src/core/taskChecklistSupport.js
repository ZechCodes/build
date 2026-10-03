// A greeting records whether the bridge checks task body hashes. Views read
// that cached fact, including before a reconnect has finished greeting.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const taskChecklistSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "task-checklist-support" });

export function rememberTaskChecklistSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const bodyPrecondition = capabilities?.tasks?.bodyPrecondition === true;
  return mergeCachedAtomically(taskChecklistSupportAddress(deviceId), (held) =>
    !current() || held?.bodyPrecondition === bodyPrecondition ? null : { bodyPrecondition });
}

export async function readTaskChecklistSupport(deviceId) {
  if (!deviceId) return false;
  return (await readCached(taskChecklistSupportAddress(deviceId)))?.value?.bodyPrecondition === true;
}
