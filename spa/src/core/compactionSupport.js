// Rendering remembers the latest greeting; dispatch still needs its live adapter.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const compactionSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "compaction-support" });
export function rememberCompactionSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const harnessThresholds = capabilities?.compaction?.harnessThresholds === true;
  return mergeCachedAtomically(compactionSupportAddress(deviceId), (held) =>
    current() && held?.harnessThresholds !== harnessThresholds ? { harnessThresholds } : null);
}
export async function readCompactionSupport(deviceId) {
  const held = deviceId ? (await readCached(compactionSupportAddress(deviceId)))?.value : null;
  return { harnessThresholds: held?.harnessThresholds === true };
}
