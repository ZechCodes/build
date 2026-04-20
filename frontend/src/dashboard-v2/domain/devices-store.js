// Device inventory. See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('devices');
const devices = new Map();

export const devicesStore = {
  get(id) { return devices.get(id); },
  list() { return [...devices.values()]; },

  upsert(device) {
    devices.set(device.id, device);
    notify({ kind: 'upsert', id: device.id });
  },

  remove(id) {
    if (!devices.delete(id)) return;
    notify({ kind: 'remove', id });
  },

  setStatus(id, status) {
    const d = devices.get(id);
    if (!d || d.status === status) return;
    devices.set(id, { ...d, status });
    notify({ kind: 'status', id, status });
  },

  replace(list) {
    const freshIds = new Set(list.map(d => d.id));
    for (const id of [...devices.keys()]) {
      if (!freshIds.has(id)) devices.delete(id);
    }
    for (const d of list) devices.set(d.id, d);
    notify({ kind: 'replace' });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('device.bulk', ({ devices: list }) => {
  if (!Array.isArray(list)) return;
  devicesStore.replace(list);
});
bus.on('device.status_changed', ({ deviceId, status }) => {
  devicesStore.setStatus(deviceId, status);
});
