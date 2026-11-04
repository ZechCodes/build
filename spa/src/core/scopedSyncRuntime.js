// Runtime joins mounted registrations to the lock holder or a bounded-timeout
// worker. Cursor compare-and-swap protects competing recovery publications.
import { captureDeviceGreeting, liveContexts, onDeviceStateChanged } from "./deviceContexts.js";
import { bridgeCapabilities, onBridgeGreeted } from "./changeEvents.js";
import { createScopedSyncEngine } from "./scopedSyncEngine.js";
import { projectSyncScope, trackedSyncScopes, scopeIsTracked } from "./scopedSyncRegistry.js";
import { broadcastScopedSyncWake, subscribeScopedSyncWakes, subscribeScopedSyncScopes } from "./scopedSyncRegistry.js";
import { subscribeCache } from "./localCache.js";
import { refreshScopedProjectColumns } from "./scopedProjectColumns.js";
import { refreshScopedTaskBody } from "./scopedTaskBody.js";
import { createScopedTaskSyncCache } from "./scopedTaskSyncCache.js";

const engines = new Map();
const scopeDefinitions = {
  project_tasks: { capability: "projectTasks", storeFor: createScopedTaskSyncCache,
    namedBy: (entry, item) => entry.scope.project_id === item.entity_id && Boolean(item.tasks) },
  task: { capability: "task", storeFor: createScopedTaskSyncCache,
    namedBy: (entry, item) => entry.projectId === item.entity_id
      && Boolean(item.tasks?.truncated || item.tasks?.task_ids?.includes(entry.scope.task_id)) },
};
const supported = (caps, scope) => caps?.sync?.[scopeDefinitions[scope.kind]?.capability] === true;
let runtime = null;
let subscriptions = [];

const browserLockAvailable = () => typeof globalThis.navigator?.locks?.request === "function";
export const scopedSyncWriter = () => Boolean(browserLockAvailable() && runtime?.ownsLock());
export const scopedSyncOwns = (deviceId, scope) => browserLockAvailable() && supported(bridgeCapabilities(deviceId), scope)
  && scopeIsTracked(deviceId, scope);
export const projectScopedSyncOwns = (deviceId, projectId) => scopedSyncOwns(deviceId, projectSyncScope(projectId));

function retire(deviceId) {
  engines.get(deviceId)?.engine.dispose();
  engines.delete(deviceId);
}

function createOwner(context, greeting) {
  const owner = runtime, session = context.session, cacheScope = context.cacheScope;
  let state;
  const active = () => runtime === owner && owner.ownsLock() && context.active()
    && context.session === session && context.cacheScope === cacheScope && greeting.current()
    && engines.get(context.deviceId) === state;
  const entries = new Map();
  const engine = createScopedSyncEngine({ active, epoch: () => greeting.epoch,
    request: (method, params) => owner.request(context.deviceId, method, params, params.scope),
    storeFor: (scope) => scopeStore(context, entries, scope, active),
  });
  state = { context, session, cacheScope, greeting, engine, entries, active, bodies: new Map() };
  return state;
}

function scopeStore(context, entries, scope, active) {
  const key = JSON.stringify(scope), entry = entries.get(key);
  return scopeDefinitions[scope.kind].storeFor({ deviceId: context.deviceId, scope,
    projectId: entry?.projectId, queryParams: entry?.queryParams, bridgeIdentity: context.deviceId,
    active: () => active() && entries.get(key) === entry });
}

function reconcileEntries(held, entries) {
  const next = new Map(entries.map((entry) => [JSON.stringify(entry.scope), entry]));
  for (const [key, entry] of held.entries) if (!next.has(key)) {
    held.engine.untrack(entry.scope);
    held.entries.delete(key);
  }
  for (const [key, entry] of next) {
    if (held.entries.has(key)) continue;
    held.entries.set(key, entry);
    held.engine.track(entry.scope);
    void Promise.all([held.engine.catchUp(entry.scope), prepareScope(held, entry)]).catch(() => {});
  }
}

export function reconcileScopedSyncRuntime() {
  if (!runtime) return;
  const contexts = browserLockAvailable() && runtime.ownsLock() ? liveContexts() : [];
  const live = new Set(contexts.map((context) => context.deviceId));
  for (const deviceId of engines.keys()) if (!live.has(deviceId)) retire(deviceId);
  for (const context of contexts) reconcileDevice(context);
}

function reconcileDevice(context) {
  const greeting = captureDeviceGreeting(context);
  if (!greeting) { retire(context.deviceId); return; }
  let held = engines.get(context.deviceId);
  if (held && (held.context !== context || held.greeting.epoch !== greeting.epoch || !held.active())) { retire(context.deviceId); held = null; }
  if (!held) {
    held = createOwner(context, greeting);
    engines.set(context.deviceId, held);
  }
  reconcileEntries(held, trackedSyncScopes(context.deviceId).filter(({ scope }) => supported(greeting.capabilities, scope)));
}

function prepareScope(held, entry) {
  if (entry.scope.kind !== "project_tasks") return refreshBody(held, entry);
  return refreshScopedProjectColumns({ deviceId: held.context.deviceId, projectId: entry.projectId,
    active: held.active, callRpc: (method, params) => runtime.request(held.context.deviceId, method, params, entry.scope) });
}

function refreshBody(held, entry) {
  if (entry.scope.kind !== "task" || !held.active()) return Promise.resolve(false);
  const taskId = entry.scope.task_id;
  const pending = held.bodies.get(taskId);
  if (pending) { pending.again = true; return pending.promise; }
  const state = { again: false };
  state.promise = refreshScopedTaskBody({ deviceId: held.context.deviceId, projectId: entry.projectId, taskId,
    active: () => held.active() && held.entries.has(JSON.stringify(entry.scope)), callRpc: (method, params) => runtime.request(held.context.deviceId, method, params, entry.scope),
  }).catch(() => false).finally(() => {
    held.bodies.delete(taskId);
    if (state.again && held.active()) void refreshBody(held, entry);
  });
  held.bodies.set(taskId, state);
  return state.promise;
}

function cacheMoved(address) {
  if (address.kind !== "tracker-task") return;
  const held = engines.get(address.deviceId);
  if (!held) return;
  for (const entry of held.entries.values()) {
    if (entry.scope.task_id === address.sub && entry.projectId === address.entityId) void refreshBody(held, entry);
  }
}

/** Capture the scopes open now. Future notifications never extend this cut. */
export function captureScopedSync(deviceId) {
  reconcileScopedSyncRuntime();
  const held = engines.get(deviceId);
  return Promise.all([...held?.entries.values() || []].map(async (entry) => {
    const recovered = await held.engine.catchUp(entry.scope);
    await prepareScope(held, entry);
    return recovered;
  }));
}

function wakeOwned(held, predicate) {
  return held.engine.wake(predicate).then(async (result) => {
    await Promise.all([...held.entries.values()].filter(({ scope }) => predicate(scope)).map((entry) => prepareScope(held, entry)));
    return result;
  });
}

function receiveWake({ deviceId, scopes }) {
  reconcileScopedSyncRuntime();
  const held = engines.get(deviceId);
  if (!held) return;
  const keys = new Set(scopes.map((scope) => JSON.stringify(scope)).filter((key) => held.entries.has(key)));
  if (!keys.size) return;
  return wakeOwned(held, (scope) => keys.has(JSON.stringify(scope))).catch(() => {});
}

export function wakeScopedSync(deviceId, predicate = () => true) {
  reconcileScopedSyncRuntime();
  const held = engines.get(deviceId);
  if (held) return wakeOwned(held, predicate);
  if (runtime && browserLockAvailable()) {
    const scopes = trackedSyncScopes(deviceId).map((entry) => entry.scope)
      .filter((scope) => supported(bridgeCapabilities(deviceId), scope) && predicate(scope));
    if (scopes.length) broadcastScopedSyncWake(deviceId, scopes);
  }
  return Promise.resolve([]);
}

export function wakeScopedSyncChanges(deviceId, items) {
  const keys = new Set(trackedSyncScopes(deviceId).filter((entry) =>
    items.some((item) => scopeDefinitions[entry.scope.kind]?.namedBy(entry, item))).map((entry) => JSON.stringify(entry.scope)));
  if (!keys.size) return Promise.resolve([]);
  return wakeScopedSync(deviceId, (scope) => keys.has(JSON.stringify(scope)));
}

export function startScopedSyncRuntime(options) {
  stopScopedSyncRuntime();
  if (!browserLockAvailable()) return;
  runtime = options;
  subscriptions = [subscribeScopedSyncScopes(reconcileScopedSyncRuntime), onDeviceStateChanged(reconcileScopedSyncRuntime),
    onBridgeGreeted(reconcileScopedSyncRuntime), subscribeCache({}, cacheMoved), subscribeScopedSyncWakes(receiveWake)];
  reconcileScopedSyncRuntime();
}

export function stopScopedSyncRuntime() {
  runtime = null;
  for (const unsubscribe of subscriptions) unsubscribe();
  subscriptions = [];
  for (const deviceId of engines.keys()) retire(deviceId);
}
