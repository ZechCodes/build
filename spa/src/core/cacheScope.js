// Whose cache this session reads and writes. Set when a session goes live,
// cleared with it. A module of its own — import-free and side-effect-free —
// so surfaces that cache (the git pane) stay importable in plain node tests
// without dragging the app shell in.

let currentDeviceId = null;

export function setCacheDevice(deviceId) {
  currentDeviceId = deviceId || null;
}

export function cacheDeviceId() {
  return currentDeviceId;
}
