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
  const owner = randomToken(), local = new Map(), remote = new Map(), listeners = new Set();
  let sequence = 0, signature = "", disposed = false;
  const all = () => [...local.values(), ...[...remote.values()].flatMap((tab) => tab.scopes)];
  const selected = () => cappedScopes(all());
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
    if (data.type !== "presence" || !Array.isArray(data.scopes)) return;
    remote.set(data.owner, { at: now(), scopes: data.scopes });
    notify();
  }
  if (channel) channel.onmessage = receive;
  channel?.postMessage({ type: "request", owner });
  const timer = setInterval(() => {
    for (const [id, tab] of remote) if (now() - tab.at >= PRESENCE_LEASE_MS) remote.delete(id);
    publish(); notify();
  }, PRESENCE_INTERVAL_MS);
  timer.unref?.();

  return {
    register(registration) {
      const id = `${owner}:${++sequence}`;
      local.set(id, { ...registration, opened: now(), ordinal: sequence, owner, id, key: scopeKey(registration) });
      publish(); notify();
      return () => { local.delete(id); publish(); notify(); };
    },
    tracked: (deviceId) => selected().filter((entry) => entry.deviceId === deviceId),
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    dispose() {
      if (disposed) return;
      local.clear(); publish(); disposed = true; clearInterval(timer); channel?.close(); listeners.clear();
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
