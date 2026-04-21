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
 *   - Skip messages sent by the client (sender === 'client') on both
 *     counts.
 *   - An unread is any server/agent message with no `read_at`.
 *   - `hasInteraction` flags the channel only when the LATEST
 *     non-client message is an unresolved interaction — i.e. the
 *     agent is currently waiting on the user. Older abandoned
 *     interactions (metadata.interaction_id, no resolved_at) in
 *     the history don't count because the conversation moved on.
 *     Metadata may arrive as a JSON string or an object; accept
 *     either.
 */
export function derive(msgs) {
  let count = 0;
  for (const m of msgs || []) {
    if (m?.sender === 'client') continue;
    if (!m?.read_at) count += 1;
  }

  let hasInteraction = false;
  for (let i = (msgs?.length || 0) - 1; i >= 0; i--) {
    const m = msgs[i];
    if (!m || m.sender === 'client') continue;  // client messages don't supersede
    const meta = parseMetadata(m.metadata);
    if (meta?.interaction_id && !meta?.resolved_at) {
      hasInteraction = true;
    }
    // First non-client message from the tail — whatever its kind
    // — decides. Anything below is "older" and irrelevant.
    break;
  }

  return { count, hasInteraction };
}

function parseMetadata(raw) {
  if (!raw) return null;
  if (typeof raw === 'object') return raw;
  try { return JSON.parse(raw); } catch (_) { return null; }
}
