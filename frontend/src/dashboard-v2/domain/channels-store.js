// Channels, indexed by id and by deviceId. See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable('channels');
const byId = new Map();            // channelId → channel
const deviceOf = new Map();        // channelId → deviceId

export const channelsStore = {
  get(id) { return byId.get(id); },
  list() { return [...byId.values()]; },
  deviceFor(id) { return deviceOf.get(id); },

  listByDevice(deviceId) {
    const out = [];
    for (const [chId, dId] of deviceOf) {
      if (dId !== deviceId) continue;
      const ch = byId.get(chId);
      if (ch) out.push(ch);
    }
    return out;
  },

  upsert({ deviceId, channel }) {
    byId.set(channel.id, channel);
    deviceOf.set(channel.id, deviceId);
    notify({ kind: 'upsert', id: channel.id, deviceId });
  },

  remove({ deviceId, channelId }) {
    if (!byId.delete(channelId)) return;
    deviceOf.delete(channelId);
    notify({ kind: 'remove', id: channelId, deviceId });
  },

  patch(id, patch) {
    const existing = byId.get(id);
    if (!existing) return;
    byId.set(id, { ...existing, ...patch });
    notify({ kind: 'patch', id });
  },

  subscribe,
};
