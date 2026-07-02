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
