// The cache scope belongs to one live application/device pairing. Surfaces
// capture the object when they mount; changing devices retires that object, so
// work resolving late cannot recover the newly-current device and write there.

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

let currentScope = null;

/** Adopt a device for this application lifetime. A reconnect to the same
 * device keeps the object surfaces already captured; a real switch retires it. */
export function adoptCacheScope(deviceId) {
  const normalized = deviceId || null;
  if (currentScope?.active() && currentScope.deviceId === normalized) return currentScope;
  currentScope?.dispose();
  currentScope = normalized ? createCacheScope(normalized) : null;
  return currentScope;
}

export function clearCacheScope() {
  currentScope?.dispose();
  currentScope = null;
}

export function currentCacheScope() {
  return currentScope;
}

// Compatibility for surfaces not yet migrated to capture the scope object.
// New code should read currentCacheScope() once while mounting.
export function setCacheDevice(deviceId) {
  return adoptCacheScope(deviceId);
}

export function cacheDeviceId() {
  return currentScope?.deviceId || null;
}
