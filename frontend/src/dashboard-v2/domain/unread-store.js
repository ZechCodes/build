// Unread tracking per channel. See planning/dashboard-v2/02-stores.md.
//
// `hasInteraction` latches true until the next markRead — if any incoming
// message was an interaction request, the channel highlights until seen.

import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable('unread');
const byChannel = new Map();       // channelId → {count, hasInteraction, lastSeen}

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { count: 0, hasInteraction: false, lastSeen: null };
    byChannel.set(channelId, s);
  }
  return s;
}

export const unreadStore = {
  get(channelId) { return byChannel.get(channelId) ?? { count: 0, hasInteraction: false, lastSeen: null }; },

  increment(channelId, hasInteraction = false) {
    const s = getSlot(channelId);
    s.count += 1;
    if (hasInteraction) s.hasInteraction = true;
    notify({ kind: 'increment', channelId });
  },

  markRead(channelId, lastSeenIso = new Date().toISOString()) {
    const s = byChannel.get(channelId);
    if (!s) return;
    s.count = 0;
    s.hasInteraction = false;
    s.lastSeen = lastSeenIso;
    notify({ kind: 'read', channelId });
  },

  aggregate() {
    let total = 0;
    let anyInteraction = false;
    for (const s of byChannel.values()) {
      total += s.count;
      if (s.hasInteraction) anyInteraction = true;
    }
    return { total, anyInteraction };
  },

  subscribe,
};
