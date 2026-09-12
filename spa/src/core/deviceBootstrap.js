// Turn the API's presence snapshot into ordered, strict relay attempts. The API
// is only a hint; trying each id on a fresh socket prevents one stale or stalled
// device from consuming the relay snapshot that also announced healthy peers.

function candidateDeviceIds(devices, selectedDeviceId) {
  const ids = devices
    .slice()
    .sort((left, right) => Number(right.status === "online") - Number(left.status === "online"))
    .map((device) => device.id);
  const selectedIsOnline = devices.some(
    (device) => device.id === selectedDeviceId && device.status === "online",
  );
  if (!selectedIsOnline) return ids;
  return [selectedDeviceId, ...ids.filter((id) => id !== selectedDeviceId)];
}

export async function openFirstReachableDevice({ devices, selectedDeviceId, open }) {
  const candidates = candidateDeviceIds(devices, selectedDeviceId);
  let lastError = new Error("no device online");
  for (const deviceId of candidates) {
    try {
      return await open({ preferDeviceId: deviceId });
    } catch (error) {
      if (error?.securityCritical) throw error;
      lastError = error;
    }
  }
  throw lastError;
}
