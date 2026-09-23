import { compare, parse } from "./bridgeApi/semver.js";
import { cacheAvailable, captureCachedGeneration, readCached, subscribeCache, writeCached, writeCachedIfGeneration } from "./localCache.js";

export const bridgeUpdateAddress = (deviceId) => ({ deviceId, entityId: "", kind: "bridge-update" });

const statuses = new Map();
const revisions = new Map();
const listeners = new Set();
const watched = new Map();

export function bridgeUpdateStatus(deviceId) {
  return statuses.get(deviceId) || null;
}
export const bridgeUpdateRevision = (deviceId) => revisions.get(deviceId) || 0;
export const bridgeUpdateCacheGeneration = async (deviceId) =>
  captureCachedGeneration(bridgeUpdateAddress(deviceId));

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

async function showStoredStatus(deviceId, fallback) {
  const record = await readCached(bridgeUpdateAddress(deviceId));
  const status = record?.value ?? (await cacheAvailable() ? null : fallback);
  if (watched.has(deviceId)) announce(deviceId, status);
}

export async function rememberBridgeUpdateStatus(deviceId, status, generation) {
  if (!status || typeof status.running_version !== "string") return false;
  advanceRevision(deviceId);
  watchDevice(deviceId);
  if (generation === undefined) {
    if (!await writeCached(bridgeUpdateAddress(deviceId), status)) await showStoredStatus(deviceId, status);
  }
  else if (!await writeCachedIfGeneration(bridgeUpdateAddress(deviceId), status, generation)) {
    await showStoredStatus(deviceId, status);
    return false;
  }
  return true;
}

const requestIsCurrent = (deviceId, revision, isCurrent) =>
  isCurrent() && bridgeUpdateRevision(deviceId) === revision;

/** Unknown-method errors are the expected answer from an older bridge. */
export async function refreshBridgeUpdateStatus(deviceId, call, isCurrent = () => true) {
  watchDevice(deviceId);
  const revision = bridgeUpdateRevision(deviceId);
  const generation = await bridgeUpdateCacheGeneration(deviceId);
  try {
    const status = await call("bridge.update_status", {});
    if (requestIsCurrent(deviceId, revision, isCurrent)) {
      await rememberBridgeUpdateStatus(deviceId, status, generation);
    }
    return status;
  } catch (error) {
    if (/unknown method|method not found/i.test(String(error?.message || ""))) {
      if (requestIsCurrent(deviceId, revision, isCurrent)) {
        if (!await writeCachedIfGeneration(bridgeUpdateAddress(deviceId), null, generation)) {
          await showStoredStatus(deviceId, null);
        }
      }
      return null;
    }
    throw error;
  }
}
