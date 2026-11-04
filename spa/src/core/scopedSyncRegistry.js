// Mounted scope presence is shared with the tab holding the sync lock. It is
// scheduling metadata only: rendering still reads ordinary cached records.
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";
import { randomToken } from "./idbRecords.js";

export const MAX_TRACKED_SYNC_SCOPES = 16;
const PRESENCE_INTERVAL_MS = 5_000;
// Hidden tabs can batch timer work into sixty-second heartbeats. Give a
// continuously mounted follower a grace window before retiring its scopes.
export const SCOPED_SYNC_PRESENCE_LEASE_MS = 90_000;
export const taskSyncScope = (taskId) => ({ kind: "task", task_id: taskId });
export function projectSyncScope(projectId, filters = {}) {
  const { order = "number", ...selected } = canonicalSummaryFilters(filters);
  return { kind: "project_tasks", project_id: projectId, filters: selected, order };
}
const scopeKey = ({ deviceId, scope }) => JSON.stringify([deviceId, scope]);

export function createScopedSyncRegistry({ channel: providedChannel, createChannel, now = Date.now } = {}) {
  const owner = randomToken(), local = new Map(), remote = new Map(), listeners = new Set(), wakeListeners = new Set();
  let sequence = 0, signature = "", disposed = false, timer = null, channel = null;
  function all() {
    for (const [id, tab] of remote) if (now() - tab.at >= SCOPED_SYNC_PRESENCE_LEASE_MS) remote.delete(id);
    return [...local.values(), ...[...remote.values()].flatMap((tab) => tab.scopes)];
  }
  const selected = () => cappedScopes(all());
  function updateSignature() {
    const next = JSON.stringify(selected());
    if (next === signature) return false;
    signature = next;
    return true;
  }
  function notify() {
    if (!updateSignature()) return;
    listeners.forEach((listener) => listener());
  }
  function restoreObservers() {
    if (!updateSignature()) return;
    for (const listener of listeners) {
      try { listener(); } catch { /* Continue restoring observers after the original failure. */ }
    }
  }
  function publish() {
    channel?.postMessage({ type: "presence", owner, scopes: [...local.values()] });
  }
  function receive({ data }) {
    if (disposed || !data?.owner || data.owner === owner) return;
    if (data.type === "request") { publish(); return; }
    if (data.type === "wake") { receiveWake(data); return; }
    if (data.type !== "presence" || !Array.isArray(data.scopes)) return;
    remote.set(data.owner, { at: now(), scopes: data.scopes.map(withoutResumeIdentity) });
    notify();
  }
  function receiveWake(data) {
    if (typeof data.deviceId !== "string" || !Array.isArray(data.scopes)) return;
    const hint = { deviceId: data.deviceId, scopes: data.scopes.slice(0, MAX_TRACKED_SYNC_SCOPES) };
    wakeListeners.forEach((listener) => listener(hint));
  }
  function openChannel() {
    channel = createChannel ? createChannel() : providedChannel;
    if (!channel) return;
    channel.onmessage = receive;
    channel.postMessage({ type: "request", owner });
  }
  function closeChannel() {
    const closing = channel;
    channel = null;
    remote.clear();
    if (!closing) return;
    closing.onmessage = null;
    if (createChannel) closing.close();
  }
  // Mounts advertise their presence; active runtime observers also need to
  // expire remote leases even when this tab has no mounted scope of its own.
  // Merely importing the registry starts no recurring work.
  function updateTimer() {
    const needed = !disposed && (local.size || listeners.size || wakeListeners.size);
    if (!needed) { clearInterval(timer); timer = null; closeChannel(); return; }
    if (timer !== null) return;
    timer = setInterval(() => { publish(); notify(); }, PRESENCE_INTERVAL_MS);
    timer.unref?.();
    openChannel();
  }
  function observe(observers, listener) {
    if (disposed) return () => {};
    observers.add(listener); updateTimer();
    return () => { observers.delete(listener); updateTimer(); };
  }
  function rollbackRegistration(id) {
    local.delete(id);
    // Retract demand that a peer or earlier listener may already have seen.
    // Cleanup failures must not replace the registration's original error.
    for (const restore of [publish, updateTimer, restoreObservers]) {
      try { restore(); } catch { /* Attempt every rollback step, then rethrow the primary error. */ }
    }
  }

  return {
    register(registration) {
      if (disposed) return () => {};
      // Mount ids describe scheduling demand, never cursor authority. The
      // engine snapshots whenever a scope becomes newly tracked there.
      const key = scopeKey(registration);
      const id = `${owner}:${++sequence}`;
      local.set(id, { ...withoutResumeIdentity(registration), opened: now(), ordinal: sequence, owner, id, key });
      try {
        updateTimer();
        publish(); notify();
      } catch (error) {
        rollbackRegistration(id);
        throw error;
      }
      return () => { local.delete(id); publish(); updateTimer(); notify(); };
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
      local.clear(); publish(); disposed = true; updateTimer(); providedChannel?.close(); listeners.clear(); wakeListeners.clear();
    },
  };
}

// Older open tabs may still advertise the retired token metadata. It is not
// part of scheduling presence and cannot authorize replay in another tab.
function withoutResumeIdentity({ registration: _token, registrationOpened: _opened, ...presence }) {
  return presence;
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

const shared = createScopedSyncRegistry({ createChannel: () => typeof window !== "undefined" && typeof BroadcastChannel !== "undefined"
  ? new BroadcastChannel("build-scoped-sync-presence") : undefined });
export const registerScopedSync = (registration) => shared.register(registration);
export const trackedSyncScopes = (deviceId) => shared.tracked(deviceId);
export const subscribeScopedSyncScopes = (listener) => shared.subscribe(listener);
export const scopeIsTracked = (deviceId, scope) => shared.tracked(deviceId).some((entry) => JSON.stringify(entry.scope) === JSON.stringify(scope));

export const broadcastScopedSyncWake = (deviceId, scopes) => shared.wake(deviceId, scopes);
export const subscribeScopedSyncWakes = (listener) => shared.subscribeWake(listener);
