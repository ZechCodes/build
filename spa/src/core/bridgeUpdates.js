import { compare, parse } from "./bridgeApi/semver.js";
import { readCached, subscribeCache, writeCached } from "./localCache.js";

export const bridgeUpdateAddress = (deviceId) => ({ deviceId, entityId: "", kind: "bridge-update" });

const statuses = new Map();
const revisions = new Map();
const listeners = new Set();
const watched = new Map();

export function bridgeUpdateStatus(deviceId) {
  return statuses.get(deviceId) || null;
}
export const bridgeUpdateRevision = (deviceId) => revisions.get(deviceId) || 0;

export function bridgeUpdateAvailable(status) {
  if (!status) return false;
  if (typeof status.update_available === "boolean") return status.update_available;
  const latest = status.latest_release?.version;
  return Boolean(parse(latest) && parse(status.running_version) && compare(latest, status.running_version) > 0);
}

export function bridgeCanInstall(status) {
  return Boolean(status && !status.development_build &&
    (typeof status.can_install === "boolean" ? status.can_install : bridgeUpdateAvailable(status)));
}

export function anyBridgeUpdateAvailable(devices = []) {
  return devices.some((device) => device.status === "online" && bridgeUpdateAvailable(bridgeUpdateStatus(device.id)));
}

export function onBridgeUpdatesChanged(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function advanceRevision(deviceId) {
  revisions.set(deviceId, bridgeUpdateRevision(deviceId) + 1);
}

function announce(deviceId, status) {
  statuses.set(deviceId, status || null);
  for (const listener of [...listeners]) listener(deviceId, status || null);
}

function watchDevice(deviceId) {
  if (watched.has(deviceId)) return;
  let generation = 0;
  const reread = async () => {
    const current = ++generation;
    const record = await readCached(bridgeUpdateAddress(deviceId));
    if (watched.has(deviceId) && current === generation) announce(deviceId, record?.value);
  };
  watched.set(deviceId, subscribeCache(bridgeUpdateAddress(deviceId), () => {
    advanceRevision(deviceId);
    void reread();
  }));
  void reread();
}

export const watchBridgeUpdateDevice = watchDevice;

/** Keep one cache subscription per paired machine, including cross-tab writes. */
export function trackBridgeUpdateDevices(devices) {
  const ids = new Set(devices.map((device) => device.id));
  for (const [deviceId, dispose] of watched) {
    if (ids.has(deviceId)) continue;
    dispose();
    watched.delete(deviceId);
    statuses.delete(deviceId);
    revisions.delete(deviceId);
  }
  for (const deviceId of ids) watchDevice(deviceId);
}

export async function rememberBridgeUpdateStatus(deviceId, status) {
  if (!status || typeof status.running_version !== "string") return false;
  advanceRevision(deviceId);
  watchDevice(deviceId);
  await writeCached(bridgeUpdateAddress(deviceId), status);
  return true;
}

/** Unknown-method errors are the expected answer from an older bridge. */
export async function refreshBridgeUpdateStatus(deviceId, call, isCurrent = () => true) {
  watchDevice(deviceId);
  const revision = bridgeUpdateRevision(deviceId);
  try {
    const status = await call("bridge.update_status", {});
    if (isCurrent() && bridgeUpdateRevision(deviceId) === revision) await rememberBridgeUpdateStatus(deviceId, status);
    return status;
  } catch (error) {
    if (/unknown method|method not found/i.test(String(error?.message || ""))) {
      if (isCurrent() && bridgeUpdateRevision(deviceId) === revision) {
        await writeCached(bridgeUpdateAddress(deviceId), null);
      }
      return null;
    }
    throw error;
  }
}
