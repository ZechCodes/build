import { mergeCachedAtomically, readCached } from "./localCache.js";

export const environmentsAddress = (deviceId) => ({ deviceId, entityId: "", kind: "environments" });

/** The last authoritative list for this device, available before reconnect. */
export async function readCachedEnvironments(deviceId) {
  const held = deviceId ? (await readCached(environmentsAddress(deviceId)))?.value : null;
  return Array.isArray(held) ? held : [];
}

/** Missing lists are partial observations; an explicit [] clears membership. */
export function writeCachedEnvironments(deviceId, rows, active = () => true) {
  if (!deviceId || !Array.isArray(rows)) return Promise.resolve(false);
  return mergeCachedAtomically(environmentsAddress(deviceId), () =>
    active() ? rows.map((row) => ({ ...row, deviceId })) : null);
}
