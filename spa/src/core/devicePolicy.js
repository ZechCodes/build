// Pure device-selection policy, shared by the gate (boot) and resume paths.

/**
 * Honor the user's sticky device choice only when that device is currently
 * online; otherwise return null ("any device"). Pinning a connect/resume to an
 * offline sticky device would ignore every other device's device_key forever.
 */
export function onlineStickyDeviceId(devices, selectedDeviceId) {
  const sticky = devices.find((d) => d.id === selectedDeviceId && d.status === "online");
  return sticky ? sticky.id : null;
}

/** What the account calls a device, or null when the list has never heard of
 *  it — the one place a device id is turned into words. */
export const deviceNameOf = (devices, deviceId) =>
  (devices || []).find((device) => device.id === deviceId)?.name || null;

/**
 * The home device: where creation goes, what the App.* aliases point at, and
 * the device a link that names none is about.
 *
 * The sticky choice while that device can answer, else whichever device can —
 * first in the account's own order, so every surface that falls back falls back
 * to the same machine. Null when nothing is online: there is no home to name.
 */
export function homeDeviceId(devices, selectedDeviceId) {
  const known = devices || [];
  return onlineStickyDeviceId(known, selectedDeviceId) || known.find((d) => d.status === "online")?.id || null;
}
