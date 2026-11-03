// Greetings persist rendering support. Dispatch always checks live capabilities;
// a downgraded greeting keeps authoritative cached totals available to paint.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const taskCountsSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "task-counts-support" });

export function rememberTaskCountsSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const counts = capabilities?.tasks?.counts === true;
  return mergeCachedAtomically(taskCountsSupportAddress(deviceId), (held) => {
    if (!current()) return null;
    const next = { counts, everCounts: counts || held?.everCounts === true || held?.counts === true };
    return held?.counts === next.counts && held?.everCounts === next.everCounts ? null : next;
  });
}

export async function readTaskCountsSupport(deviceId) {
  if (!deviceId) return false;
  const held = (await readCached(taskCountsSupportAddress(deviceId)))?.value;
  return held?.everCounts === true || held?.counts === true;
}
