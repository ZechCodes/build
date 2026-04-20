// Agent activity feed (tool use, tool result, reasoning) per channel.
// See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable('activity');
const byChannel = new Map();       // channelId → ActivityEntry[]

function getList(channelId) {
  let arr = byChannel.get(channelId);
  if (!arr) {
    arr = [];
    byChannel.set(channelId, arr);
  }
  return arr;
}

export const activityStore = {
  forChannel(channelId) { return byChannel.get(channelId) ?? []; },

  appendToolUse(channelId, { toolUseId, name, input, at }) {
    getList(channelId).push({ type: 'tool_use', toolUseId, name, input, at });
    notify({ kind: 'tool_use', channelId, toolUseId });
  },

  markToolResult(channelId, toolUseId, { isError, content, at }) {
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const entry = arr.find(e => e.type === 'tool_use' && e.toolUseId === toolUseId);
    if (!entry) return;
    entry.result = { isError, content, at };
    notify({ kind: 'tool_result', channelId, toolUseId });
  },

  appendReasoning(channelId, text, at) {
    const arr = getList(channelId);
    const last = arr[arr.length - 1];
    if (last && last.type === 'reasoning') {
      last.text += text;
      last.at = at ?? last.at;
    } else {
      arr.push({ type: 'reasoning', text, at });
    }
    notify({ kind: 'reasoning', channelId });
  },

  clear(channelId) {
    if (!byChannel.has(channelId)) return;
    byChannel.set(channelId, []);
    notify({ kind: 'clear', channelId });
  },

  subscribe,
};
