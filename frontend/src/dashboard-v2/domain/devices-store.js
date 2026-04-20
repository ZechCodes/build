// Device inventory. See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';

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

  subscribe,
};
