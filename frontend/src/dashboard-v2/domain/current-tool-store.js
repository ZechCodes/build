// Tracks the latest in-flight tool use per channel, so the chat
// overlay can show a one-line "what's the agent doing right now?"
// strip without reaching into activityStore's full history.
//
// Cleared on:
//   - agent.tool_result matching the current toolUseId
//   - presenceStore transitioning agentActive → false (defensive
//     reset in case the terminal tool_result never arrives)

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';
import { presenceStore } from './presence-store.js';

const { subscribe, notify } = makeSubscribable('current-tool');
const byChannel = new Map();   // channelId → { toolUseId, name, input, at } | null

export const currentToolStore = {
  get(channelId) { return byChannel.get(channelId) ?? null; },

  set(channelId, entry) {
    byChannel.set(channelId, entry);
    notify({ channelId, entry });
  },

  clear(channelId) {
    if (!byChannel.has(channelId)) return;
    byChannel.delete(channelId);
    notify({ channelId, entry: null });
  },

  subscribe,
};

let bound = false;

export function bindCurrentTool() {
  if (bound) return;
  bound = true;

  bus.on('agent.tool_use', ({ channelId, toolUseId, name, input, at }) => {
    if (!channelId) return;
    currentToolStore.set(channelId, { toolUseId, name, input, at });
  });

  bus.on('agent.tool_result', ({ channelId, toolUseId }) => {
    if (!channelId) return;
    const cur = currentToolStore.get(channelId);
    if (cur && cur.toolUseId === toolUseId) currentToolStore.clear(channelId);
  });

  presenceStore.subscribe((e) => {
    if (e.kind !== 'agent_active') return;
    if (e.active) return;
    currentToolStore.clear(e.channelId);
  });
}
