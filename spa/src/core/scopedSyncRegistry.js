// Mounted scope presence is shared with the tab holding the sync lock. It is
// scheduling metadata only: rendering still reads ordinary cached records.
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";
import { randomToken } from "./idbRecords.js";

export const MAX_TRACKED_SYNC_SCOPES = 16;
const PRESENCE_INTERVAL_MS = 5_000;
const PRESENCE_LEASE_MS = 15_000;
export const taskSyncScope = (taskId) => ({ kind: "task", task_id: taskId });
export function projectSyncScope(projectId, filters = {}) {
  const { order = "number", ...selected } = canonicalSummaryFilters(filters);
  return { kind: "project_tasks", project_id: projectId, filters: selected, order };
}
const scopeKey = ({ deviceId, scope }) => JSON.stringify([deviceId, scope]);

export function createScopedSyncRegistry({ channel, now = Date.now } = {}) {
  const owner = randomToken(), local = new Map(), remote = new Map(), listeners = new Set(), wakeListeners = new Set();
  let sequence = 0, signature = "", disposed = false, timer = null;
  const all = () => [...local.values(), ...[...remote.values()].flatMap((tab) => tab.scopes)];
  function selected() {
    for (const [id, tab] of remote) if (now() - tab.at >= PRESENCE_LEASE_MS) remote.delete(id);
    return cappedScopes(all());
  }
  function notify() {
    const next = JSON.stringify(selected());
    if (next === signature) return;
    signature = next;
    listeners.forEach((listener) => listener());
  }
  function publish() {
    channel?.postMessage({ type: "presence", owner, scopes: [...local.values()] });
  }
  function receive({ data }) {
    if (disposed || !data?.owner || data.owner === owner) return;
    if (data.type === "request") { publish(); return; }
    if (data.type === "wake") { receiveWake(data); return; }
    if (data.type !== "presence" || !Array.isArray(data.scopes)) return;
    remote.set(data.owner, { at: now(), scopes: data.scopes });
    notify();
  }
  function receiveWake(data) {
    if (typeof data.deviceId !== "string" || !Array.isArray(data.scopes)) return;
    const hint = { deviceId: data.deviceId, scopes: data.scopes.slice(0, MAX_TRACKED_SYNC_SCOPES) };
    wakeListeners.forEach((listener) => listener(hint));
  }
  // Mounts advertise their presence; active runtime observers also need to
  // expire remote leases even when this tab has no mounted scope of its own.
  // Merely importing the registry starts no recurring work.
  function updateTimer() {
    const needed = !disposed && (local.size || listeners.size || wakeListeners.size);
    if (!needed) { clearInterval(timer); timer = null; return; }
    if (timer !== null) return;
    timer = setInterval(() => { publish(); notify(); }, PRESENCE_INTERVAL_MS);
    timer.unref?.();
    channel?.postMessage({ type: "request", owner });
  }
  function observe(observers, listener) {
    if (disposed) return () => {};
    observers.add(listener); updateTimer();
    return () => { observers.delete(listener); updateTimer(); };
  }
  if (channel) channel.onmessage = receive;
  channel?.postMessage({ type: "request", owner });

  return {
    register(registration) {
      if (disposed) return () => {};
      // Duplicate mounts belong to one continuous opening. A new opening
      // after the final release must not inherit the previous cursor proof.
      const key = scopeKey(registration);
      const opening = selected().find((entry) => entry.key === key)?.registration || randomToken();
      const id = `${owner}:${++sequence}`;
      local.set(id, { ...registration, registration: opening, opened: now(), ordinal: sequence, owner, id, key });
      updateTimer();
      publish(); notify();
      return () => { local.delete(id); updateTimer(); publish(); notify(); };
    },
    tracked: (deviceId) => selected().filter((entry) => entry.deviceId === deviceId),
    subscribe: (listener) => observe(listeners, listener),
    wake(deviceId, scopes) {
      if (disposed || !scopes.length) return;
      publish();
      channel?.postMessage({ type: "wake", owner, deviceId, scopes: scopes.slice(0, MAX_TRACKED_SYNC_SCOPES) });
    },
    subscribeWake: (listener) => observe(wakeListeners, listener),
    dispose() {
      if (disposed) return;
      local.clear(); publish(); disposed = true; updateTimer(); channel?.close(); listeners.clear(); wakeListeners.clear();
    },
  };
}

function cappedScopes(registrations) {
  const sorted = [...registrations].sort((a, b) => a.opened - b.opened || a.ordinal - b.ordinal || a.id.localeCompare(b.id));
  const devices = new Map();
  for (const entry of sorted) {
    if (!devices.has(entry.deviceId)) devices.set(entry.deviceId, new Map());
    const scopes = devices.get(entry.deviceId);
    scopes.delete(entry.key); scopes.set(entry.key, entry);
  }
  return [...devices.values()].flatMap((scopes) => [...scopes.values()].slice(-MAX_TRACKED_SYNC_SCOPES));
}

const shared = createScopedSyncRegistry({ channel: typeof window !== "undefined" && typeof BroadcastChannel !== "undefined"
  ? new BroadcastChannel("build-scoped-sync-presence") : undefined });
export const registerScopedSync = (registration) => shared.register(registration);
export const trackedSyncScopes = (deviceId) => shared.tracked(deviceId);
export const subscribeScopedSyncScopes = (listener) => shared.subscribe(listener);
export const scopeIsTracked = (deviceId, scope) => shared.tracked(deviceId).some((entry) => JSON.stringify(entry.scope) === JSON.stringify(scope));

export const broadcastScopedSyncWake = (deviceId, scopes) => shared.wake(deviceId, scopes);
export const subscribeScopedSyncWakes = (listener) => shared.subscribeWake(listener);
