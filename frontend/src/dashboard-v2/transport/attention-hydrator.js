// On connect / reconnect, populate the Attention sidebar section
// from whatever message history the bridge hands us. The live bus
// events (message.received, interaction.requested) only see things
// happening _now_; they don't backfill state that already existed
// when the page loaded.
//
// Hook points:
//   channel.list  → fires on every E2EE connect (and reconnect).
//                   For each channel in the list, request the
//                   recent message history.
//   message.bulk → fires when the bridge returns a get_messages
//                   response. Derive (unread count, hasInteraction)
//                   from the returned messages and hydrate the
//                   unreadStore accordingly.

import { bus } from '../core/bus.js';
import { unreadStore } from '../domain/unread-store.js';
import { log } from '../core/log.js';

const plog = log('attention-hydrator');
const HISTORY_LIMIT = 200;

let bound = false;

export function bindAttentionHydrator() {
  if (bound) return;
  bound = true;

  bus.on('channel.list', ({ channels }) => {
    if (!Array.isArray(channels)) return;
    for (const ch of channels) {
      if (!ch?.id) continue;
      bus.emit('intent.get_messages', {
        channelId: ch.id,
        limit: HISTORY_LIMIT,
        before: null,
      });
    }
  });

  bus.on('message.bulk', ({ channelId, msgs }) => {
    if (!channelId) return;
    const { count, hasInteraction } = derive(msgs);
    plog.debug('hydrate', channelId, { count, hasInteraction });
    unreadStore.hydrate(channelId, count, hasInteraction);
  });
}

/**
 * Count unreads + detect a pending interaction from a bulk message
 * list. Rules:
 *   - Skip messages sent by the client (sender === 'client').
 *   - An unread is any server/agent message with no `read_at`.
 *   - A pending interaction is any message whose metadata has an
 *     `interaction_id` and no `resolved_at`. Metadata may arrive
 *     as a JSON string or as an object — accept either.
 */
export function derive(msgs) {
  let count = 0;
  let hasInteraction = false;
  for (const m of msgs || []) {
    if (m?.sender === 'client') continue;
    if (!m?.read_at) count += 1;
    if (m?.metadata) {
      let meta;
      try { meta = typeof m.metadata === 'string' ? JSON.parse(m.metadata) : m.metadata; }
      catch (_) { meta = null; }
      if (meta?.interaction_id && !meta?.resolved_at) hasInteraction = true;
    }
  }
  return { count, hasInteraction };
}
