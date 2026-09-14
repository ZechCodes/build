// A cache scope belongs to one live application/device pairing. Surfaces
// capture the object when they mount; retiring a device retires its object, so
// work resolving late cannot recover a newly-adopted device and write there.
//
// Every paired device keeps its own scope for as long as the app holds a
// context for it: adopting a second device must not invalidate the first
// device's in-flight reads. Only releaseScope (a retired device) retires one.

function createCacheScope(deviceId) {
  const capturedDeviceId = deviceId || null;
  let live = Boolean(capturedDeviceId);

  return Object.freeze({
    deviceId: capturedDeviceId,
    key: `device:${capturedDeviceId || ""}`,

    active() {
      return live;
    },

    /** Complete a cache address only while this captured scope still owns it. */
    address(parts) {
      return live && capturedDeviceId ? { ...parts, deviceId: capturedDeviceId } : null;
    },

    dispose() {
      live = false;
    },
  });
}

const scopes = new Map(); // deviceId → the scope that device's surfaces captured

/** This device's scope, created on the first ask and the same object until it
 *  is released. Falsy device ids have no scope: there is nothing to address. */
export function scopeFor(deviceId) {
  const normalized = deviceId || null;
  if (!normalized) return null;
  const captured = scopes.get(normalized);
  if (captured?.active()) return captured;
  const scope = createCacheScope(normalized);
  scopes.set(normalized, scope);
  return scope;
}

/** Retire one device's scope: every address it still owes answers null. */
export function releaseScope(deviceId) {
  const scope = scopes.get(deviceId);
  if (!scope) return;
  scopes.delete(deviceId);
  scope.dispose();
}

/** Release every device (tests, sign-out). */
export function clearCacheScope() {
  for (const deviceId of [...scopes.keys()]) releaseScope(deviceId);
}
