// Mounted panes tell the sync planner which cached records are on screen.
// These getters only describe visibility; they do not read connection state
// or change how a surface paints its cache.
const trees = new Set();
const diffs = new Set();
const threads = new Set();
const addressOf = (address) => typeof address === "function" ? address() : address;
const alwaysActive = () => true;

const register = (registry, address, value, active) => {
  const scope = { address, value, active };
  registry.add(scope);
  return () => registry.delete(scope);
};

export const registerSyncTree = (address, pathsGetter, activeGetter = alwaysActive) =>
  register(trees, address, pathsGetter, activeGetter);

export const registerSyncDiff = (address, activeGetter = alwaysActive) =>
  register(diffs, address, null, activeGetter);

export const registerSyncThread = (identityGetter, activeGetter = alwaysActive) =>
  register(threads, identityGetter, null, activeGetter);

/** The mounted chat may remember a different agent than the URL names. */
export function visibleSyncThread(deviceId) {
  for (const scope of threads) {
    if (!scope.active()) continue;
    const identity = addressOf(scope.address);
    if (identity?.deviceId === deviceId && identity.entityId && identity.agentId) return identity;
  }
  return null;
}

const matches = (scope, deviceId, entityId, kind) => {
  const address = addressOf(scope.address);
  return address?.deviceId === deviceId && address?.entityId === entityId
    && address?.kind === kind && scope.active();
};

export const visibleTreePaths = (deviceId, entityId) => [...new Set(
  [...trees].filter((scope) => matches(scope, deviceId, entityId, "tree"))
    .flatMap((scope) => scope.value()),
)];

export const visibleWorkingDiff = (deviceId, entityId) =>
  [...diffs].some((scope) => matches(scope, deviceId, entityId, "diff"));

/** A mounted pane can stay connected while its root/tab is folded away. */
export function syncPaneShown(element) {
  if (!element?.isConnected) return false;
  if (element.ownerDocument.hidden) return false;
  const view = element.ownerDocument.defaultView;
  for (let current = element; current; current = current.parentElement) {
    if (current.hidden) return false;
    const style = view.getComputedStyle(current);
    if (style.display === "none" || style.visibility === "hidden") return false;
  }
  return true;
}
