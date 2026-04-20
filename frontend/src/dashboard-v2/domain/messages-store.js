// Chat messages per channel. See planning/dashboard-v2/02-stores.md.
//
// Append is idempotent on msg.id. Bulk replaces the per-channel list.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('messages');
const byChannel = new Map();       // channelId → Message[]

function getList(channelId) {
  let arr = byChannel.get(channelId);
  if (!arr) {
    arr = [];
    byChannel.set(channelId, arr);
  }
  return arr;
}

export const messagesStore = {
  forChannel(channelId) { return byChannel.get(channelId) ?? []; },

  append(channelId, msg) {
    const arr = getList(channelId);
    if (msg.id && arr.some(m => m.id === msg.id)) return;  // dedup
    arr.push(msg);
    notify({ kind: 'append', channelId, msg });
  },

  bulk(channelId, msgs) {
    byChannel.set(channelId, msgs.slice());
    notify({ kind: 'bulk', channelId });
  },

  markRead(channelId, msgIds) {
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const set = new Set(msgIds);
    const now = new Date().toISOString();
    let changed = false;
    for (const m of arr) {
      if (set.has(m.id) && !m.read_at) {
        m.read_at = now;
        changed = true;
      }
    }
    if (changed) notify({ kind: 'read', channelId });
  },

  markDelivered(channelId, msgId) {
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const m = arr.find(x => x.id === msgId);
    if (!m || m.delivered_at) return;
    m.delivered_at = new Date().toISOString();
    notify({ kind: 'delivered', channelId, msgId });
  },

  markFailed(channelId, msgId) {
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const m = arr.find(x => x.id === msgId);
    if (!m) return;
    m.delivery_failed = true;
    notify({ kind: 'delivery_failed', channelId, msgId });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('message.bulk', ({ channelId, msgs }) => {
  messagesStore.bulk(channelId, msgs || []);
});
bus.on('message.received', ({ channelId, msg }) => {
  messagesStore.append(channelId, msg);
});
bus.on('message.read', ({ channelId, msgIds }) => {
  messagesStore.markRead(channelId, msgIds || []);
});
bus.on('message.delivered', ({ channelId, msgId }) => {
  messagesStore.markDelivered(channelId, msgId);
});
bus.on('message.delivery_failed', ({ channelId, msgId }) => {
  messagesStore.markFailed(channelId, msgId);
});
