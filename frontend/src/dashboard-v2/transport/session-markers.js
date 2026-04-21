// Inserts a client-side "divider" message into the chat when the
// agent resets or compacts its session. These markers are not
// persisted server-side — they reappear each time a fresh
// `session.reset` / `session.compacting` event arrives.

import { bus } from '../core/bus.js';
import { messagesStore } from '../domain/messages-store.js';

let bound = false;

function makeDivider(channelId, kind, content) {
  return {
    id: `divider-${kind}-${Date.now()}`,
    channel_id: channelId,
    sender: 'system',
    kind: 'session-divider',
    divider_kind: kind,
    content,
    created_at: new Date().toISOString(),
  };
}

export function bindSessionMarkers() {
  if (bound) return;
  bound = true;

  bus.on('session.reset', ({ channelId }) => {
    if (!channelId) return;
    messagesStore.append(channelId, makeDivider(channelId, 'reset', 'New session started'));
  });

  bus.on('session.compacting', ({ channelId }) => {
    if (!channelId) return;
    messagesStore.append(channelId, makeDivider(channelId, 'compact', 'Session compacted'));
  });
}
