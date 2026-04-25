// Channels, indexed by id and by deviceId. See planning/dashboard/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

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

  replaceForDevice(deviceId, channels) {
    const fresh = new Set(channels.map(c => c.id));
    for (const [chId, dId] of [...deviceOf]) {
      if (dId === deviceId && !fresh.has(chId)) {
        byId.delete(chId);
        deviceOf.delete(chId);
      }
    }
    for (const ch of channels) {
      byId.set(ch.id, ch);
      deviceOf.set(ch.id, deviceId);
    }
    notify({ kind: 'replace_for_device', deviceId });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('channel.list', ({ deviceId, channels }) => {
  channelsStore.replaceForDevice(deviceId, channels || []);
});
bus.on('channel.upserted', (payload) => {
  channelsStore.upsert(payload);
});
bus.on('channel.removed', (payload) => {
  channelsStore.remove(payload);
});
bus.on('channel.patched', ({ channelId, patch }) => {
  channelsStore.patch(channelId, patch || {});
});
