// Navigation support and environment rows are read from the cache, never hello
// or connection state. Canonical writers and sync remain their existing owners.
import { readEnvironmentSupport } from "./environmentSupport.js";
import { readCachedEnvironments } from "./environmentCache.js";
import { subscribeCache } from "./localCache.js";

export async function readEnvironmentNavigation(devices) {
  const rows = await Promise.all(devices.map(async (device) => {
    const [support, environments] = await Promise.all([readEnvironmentSupport(device.id), readCachedEnvironments(device.id)]);
    return { deviceId: device.id, supported: support.list, environments: environments.map((row) => ({ ...row, deviceId: device.id })) };
  }));
  return { devices, environments: rows.flatMap((row) => row.environments), support: Object.fromEntries(rows.map((row) => [row.deviceId, row.supported])) };
}

export function followEnvironmentNavigation(devices, onChange) {
  let disposed = false;
  let reading = 0;
  const read = async () => {
    const token = ++reading;
    const context = await readEnvironmentNavigation(devices());
    if (!disposed && token === reading) onChange(context);
  };
  const stop = subscribeCache({}, (address) => {
    if (["environment-support", "environments", "devices"].includes(address.kind)) void read();
  });
  return { ready: read(), dispose() { disposed = true; stop(); } };
}
