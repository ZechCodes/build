import { mergeCachedRecordsTogether, mergeCachedTogether, readCached } from "./localCache.js";
import { randomToken } from "./idbRecords.js";

export const environmentsAddress = (deviceId) => ({ deviceId, entityId: "", kind: "environments" });
const orderAddress = (deviceId) => ({ deviceId, entityId: "", kind: "environment-list-order" });

/** The last authoritative list for this device, available before reconnect. */
export async function readCachedEnvironments(deviceId) {
  const held = deviceId ? (await readCached(environmentsAddress(deviceId)))?.value : null;
  return Array.isArray(held) ? held : [];
}

/** Reserve before queueing a push or read. Admissions share an order across
 * tabs without changing or announcing the last good list. Failed reads never
 * roll back a reservation, and a cache reset cannot reuse an old identity. */
export async function admitEnvironmentListWrite(deviceId, active = () => true) {
  if (!deviceId) return null;
  const token = randomToken();
  try {
    const committed = await mergeCachedRecordsTogether([orderAddress(deviceId)], () =>
      active() ? [{ token }] : null, { notify: false });
    return committed ? token : null;
  } catch {
    return null;
  }
}

/** Only the newest admission may publish, checked with the list replacement
 * in one transaction. Missing lists retain rows; an explicit [] clears them. */
export async function writeCachedEnvironments(deviceId, rows, admission, active = () => true) {
  if (!deviceId || !Array.isArray(rows)) return false;
  const token = await admission;
  if (!token) return false;
  return mergeCachedTogether([orderAddress(deviceId), environmentsAddress(deviceId)], ([order]) =>
    active() && order?.token === token ? [null, rows.map((row) => ({ ...row, deviceId }))] : null);
}
