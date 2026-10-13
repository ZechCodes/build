// Greetings persist current rendering support and the highest support seen.
// Assignment admission reads that history after the current greeting settles.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const taskWaitingOnSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "task-waiting-on-support" });
export const TASK_WAITING_ON_UPDATE = "Update this device's bridge to edit prerequisites or save assignments that wait.";
export const TASK_WAITING_ON_DOWNGRADED = "This device's bridge was downgraded; update it to assign tasks.";
const everSupported = (value) => value?.everWaitingOn === true || value?.waitingOn === true;

export function rememberTaskWaitingOnSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId || !current()) return Promise.resolve(false);
  const waitingOn = capabilities?.tasks?.waitingOn === true;
  // An admitted greeting remains evidence even if another greeting overtakes
  // its storage transaction. Only the current rendering flag is replaceable.
  return mergeCachedAtomically(taskWaitingOnSupportAddress(deviceId), (held) => {
    const next = { waitingOn: current() ? waitingOn : held?.waitingOn === true,
      everWaitingOn: waitingOn || everSupported(held) };
    return held?.waitingOn === next.waitingOn && held?.everWaitingOn === next.everWaitingOn ? null : next;
  });
}

export async function readTaskWaitingOnSupport(deviceId) {
  if (!deviceId) return false;
  return (await readCached(taskWaitingOnSupportAddress(deviceId)))?.value?.waitingOn === true;
}

export async function readTaskWaitingOnEverSupported(deviceId) {
  if (!deviceId) return false;
  return everSupported((await readCached(taskWaitingOnSupportAddress(deviceId)))?.value);
}
