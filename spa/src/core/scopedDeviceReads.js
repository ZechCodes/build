// Mounted directory readers share the device journal's writer. Old bridges
// retain their pull, with ownership rechecked inside its cache transaction.
import { deviceScopedSyncOwns, wakeScopedSync } from "./scopedSyncRuntime.js";

const wakeDevice = (deviceId) => wakeScopedSync(deviceId, (scope) => scope.kind === "device");
export async function pullDeviceProjection(deviceId, record, fetch) {
  if (deviceScopedSyncOwns(deviceId)) {
    await wakeDevice(deviceId);
    if (deviceScopedSyncOwns(deviceId)) return record.read();
  }
  const accepted = await record.pull(fetch, { accept: () => !deviceScopedSyncOwns(deviceId) });
  if (!accepted && deviceScopedSyncOwns(deviceId)) {
    await wakeDevice(deviceId);
    await record.read();
  }
}
