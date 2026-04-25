// Unread tracking per channel. See planning/dashboard/02-stores.md.
//
// `hasInteraction` latches true until the next markRead — if any incoming
// message was an interaction request, the channel highlights until seen.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';
import { isUnresolvedInteractionMessage } from './read-gate.js';

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

  /**
   * Directly set the unread state for a channel, bypassing the
   * active-channel suppression that `increment` respects. Used by
   * the attention-hydrator on load / reconnect to catch up from
   * bulk message history (see transport/attention-hydrator.js).
   */
  hydrate(channelId, count, hasInteraction) {
    const s = getSlot(channelId);
    s.count = count | 0;
    s.hasInteraction = !!hasInteraction;
    notify({ kind: 'hydrate', channelId });
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

// ----- Bus bindings -----
bus.on('message.received', ({ channelId, msg }) => {
  if (!channelId || !msg) return;
  if (msg.sender === 'client') return;
  // interaction.requested owns the unread increment for mirrored
  // interaction cards so they count once while still latching the
  // interaction state.
  if (isUnresolvedInteractionMessage(msg)) return;
  unreadStore.increment(channelId);
});
bus.on('interaction.requested', ({ channelId }) => {
  if (!channelId) return;
  unreadStore.increment(channelId, true);
});
