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

export function createScopedSyncRegistry({ channel: providedChannel, createChannel, now = Date.now } = {}) {
  const owner = randomToken(), local = new Map(), remote = new Map(), listeners = new Set(), wakeListeners = new Set();
  let sequence = 0, signature = "", disposed = false, timer = null, channel = null;
  const all = () => [...local.values(), ...[...remote.values()].flatMap((tab) => tab.scopes)];
  function selected() {
    for (const [id, tab] of remote) if (now() - tab.at >= PRESENCE_LEASE_MS) remote.delete(id);
    const registrations = all(), winners = winningOpenings(registrations);
    return cappedScopes(registrations).map((entry) => ({ ...entry, ...winners.get(entry.key) }));
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
    adoptOpenings();
    notify();
  }
  function adoptOpenings() {
    const winners = winningOpenings(all());
    let changed = false;
    for (const [id, entry] of local) {
      const winner = winners.get(entry.key);
      if (!winner || sameOpening(entry, winner)) continue;
      local.set(id, { ...entry, ...winner }); changed = true;
    }
    if (changed) publish();
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
    if (channel) {
      channel.onmessage = null;
      if (createChannel) channel.close();
      channel = null;
    }
    remote.clear();
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

  return {
    register(registration) {
      if (disposed) return () => {};
      // Duplicate mounts belong to one continuous opening. A new opening
      // after the final release must not inherit the previous cursor proof.
      const key = scopeKey(registration);
      const opening = selected().find((entry) => entry.key === key), opened = now();
      const id = `${owner}:${++sequence}`;
      local.set(id, { ...registration, registration: opening?.registration || randomToken(),
        registrationOpened: opening?.registrationOpened ?? opened, opened, ordinal: sequence, owner, id, key });
      updateTimer();
      publish(); notify();
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

const sameOpening = (entry, winner) => entry.registration === winner.registration
  && entry.registrationOpened === winner.registrationOpened;
const earlierOpening = (candidate, held) => !held || candidate.registrationOpened < held.registrationOpened
  || candidate.registrationOpened === held.registrationOpened && candidate.registration < held.registration;

function openingIdentity(entry) {
  const registrationOpened = entry.registrationOpened ?? entry.opened;
  return typeof entry.registration === "string" && Number.isFinite(registrationOpened)
    ? { registration: entry.registration, registrationOpened } : null;
}

// Presence is asynchronous: two tabs can open before hearing each other.
// Carry the earliest opening through every surviving local duplicate. Its
// identity time is separate from `opened`, which still orders the tracking cap.
function winningOpenings(registrations) {
  const winners = new Map();
  for (const entry of registrations) {
    const candidate = openingIdentity(entry);
    if (candidate && earlierOpening(candidate, winners.get(entry.key))) winners.set(entry.key, candidate);
  }
  return winners;
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
