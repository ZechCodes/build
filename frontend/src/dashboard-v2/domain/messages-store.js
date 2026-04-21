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

  /**
   * Swap an optimistic client-generated id for the real id returned by
   * BuildE2EE.send(). Needed so `delivered` / `read` events (which
   * reference the real id) can find the optimistic row.
   */
  replaceId(channelId, oldId, newId) {
    if (!oldId || !newId || oldId === newId) return;
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const oldIdx = arr.findIndex(x => x.id === oldId);
    if (oldIdx === -1) return;
    // Race: the server echo may have already inserted a row with newId
    // (possible if HTTP POST is slow and the echo wire event arrives
    // first). Merge delivered_at / read_at from whichever row has them
    // and drop the duplicate.
    const newIdx = arr.findIndex(x => x.id === newId);
    if (newIdx !== -1 && newIdx !== oldIdx) {
      const merged = arr[newIdx];
      const optimistic = arr[oldIdx];
      if (optimistic.delivered_at && !merged.delivered_at) merged.delivered_at = optimistic.delivered_at;
      if (optimistic.read_at && !merged.read_at) merged.read_at = optimistic.read_at;
      arr.splice(oldIdx, 1);
    } else {
      arr[oldIdx].id = newId;
    }
    notify({ kind: 'replace_id', channelId, oldId, newId });
  },

  markFailed(channelId, msgId) {
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const m = arr.find(x => x.id === msgId);
    if (!m) return;
    m.delivery_failed = true;
    notify({ kind: 'delivery_failed', channelId, msgId });
  },

  /**
   * Merge `patch` into the target message's metadata. Metadata may
   * arrive from the server as a JSON string or as an object; we
   * respect whichever shape the message already uses so the next
   * render (and any subsequent echo) sees the same format.
   *
   * Used by the chat view to optimistically mark an interaction as
   * resolved on click — setting `resolved_at`, `selected_option`,
   * etc. so `.selected` renders on the chosen button before the
   * server's authoritative echo comes back.
   */
  patchInteractionMeta(channelId, msgId, patch) {
    if (!msgId || !patch) return;
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const m = arr.find(x => x.id === msgId);
    if (!m) return;
    const isString = typeof m.metadata === 'string';
    let meta;
    try {
      meta = isString ? JSON.parse(m.metadata || '{}') : (m.metadata || {});
    } catch (_) {
      meta = {};
    }
    Object.assign(meta, patch);
    m.metadata = isString ? JSON.stringify(meta) : meta;
    notify({ kind: 'interaction_patch', channelId, msgId });
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
