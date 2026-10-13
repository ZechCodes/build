// Greetings persist current rendering support and the highest support seen.
// Assignment admission reads that history after the current greeting settles.
import { mergeCachedAtomically, readCached, readCachedOutcome } from "./localCache.js";

export const taskWaitingOnSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "task-waiting-on-support" });
export const TASK_WAITING_ON_UPDATE = "Update this device's bridge to edit prerequisites or save assignments that wait.";
export const TASK_WAITING_ON_DOWNGRADED = "This device's bridge was downgraded; update it to assign tasks. Retry or refresh if capability history could not be saved or read.";
const everSupported = (value) => value?.everWaitingOn === true || value?.waitingOn === true;
const seen = new Set();
const uncertain = new Set();
let accountEra = 0;

export function resetTaskWaitingOnHistory() {
  accountEra += 1;
  seen.clear();
  uncertain.clear();
}

function historyOf(value) {
  if (value === undefined) return false;
  if (everSupported(value)) return true;
  if (value?.everWaitingOn === false) return false;
  if (value?.everWaitingOn === undefined && value?.waitingOn === false) return false;
  return null;
}

export function rememberTaskWaitingOnSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const waitingOn = capabilities?.tasks?.waitingOn === true;
  const era = accountEra;
  // Authenticated evidence is monotonic, regardless of session supersession
  // or a refused put. Only the rendering flag belongs to the current hello.
  if (waitingOn) seen.add(deviceId);
  uncertain.add(deviceId);
  return mergeCachedAtomically(taskWaitingOnSupportAddress(deviceId), (held) => {
    if (era !== accountEra) return null;
    if (everSupported(held)) seen.add(deviceId);
    const next = { waitingOn: current() ? waitingOn : held?.waitingOn === true,
      everWaitingOn: seen.has(deviceId) ? true : historyOf(held) };
    return held?.waitingOn === next.waitingOn && held?.everWaitingOn === next.everWaitingOn ? null : next;
  }, { reportCommit: true }).then((committed) => {
    if (era === accountEra) {
      if (committed) uncertain.delete(deviceId);
      else uncertain.add(deviceId);
    }
    return committed;
  });
}

export async function readTaskWaitingOnSupport(deviceId) {
  if (!deviceId) return false;
  return (await readCached(taskWaitingOnSupportAddress(deviceId)))?.value?.waitingOn === true;
}

export async function readTaskWaitingOnEverSupported(deviceId) {
  if (!deviceId) return null;
  if (seen.has(deviceId)) return true;
  const era = accountEra;
  const { committed, result } = await readCachedOutcome(taskWaitingOnSupportAddress(deviceId));
  if (!committed || era !== accountEra) return null;
  const history = historyOf(result?.value);
  if (history === true) seen.add(deviceId);
  return uncertain.has(deviceId) && history !== true ? null : history;
}
