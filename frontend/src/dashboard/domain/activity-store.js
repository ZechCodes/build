// Agent activity feed (tool use, tool result, reasoning) per channel.
// See planning/dashboard/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

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

  replace(channelId, entries) {
    const normalized = [];
    for (const e of entries || []) {
      if (e.type === 'tool_use') {
        const d = e.data || {};
        normalized.push({
          type: 'tool_use',
          toolUseId: d.id || '',
          name: d.name || 'tool',
          input: d.input || {},
          at: e.created_at,
        });
      } else if (e.type === 'tool_result') {
        const d = e.data || {};
        const match = normalized.find(x => x.type === 'tool_use' && x.toolUseId === d.tool_use_id);
        if (match) {
          match.result = { isError: !!d.is_error, content: d.content, at: e.created_at };
        }
      } else if (e.type === 'text') {
        const text = e.data?.text || '';
        if (text) normalized.push({ type: 'reasoning', text, at: e.created_at });
      }
    }
    byChannel.set(channelId, normalized);
    notify({ kind: 'replace', channelId });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('agent.tool_use', ({ channelId, toolUseId, name, input, at }) => {
  activityStore.appendToolUse(channelId, { toolUseId, name, input, at });
});
bus.on('agent.tool_result', ({ channelId, toolUseId, isError, content, at }) => {
  activityStore.markToolResult(channelId, toolUseId, { isError, content, at });
});
bus.on('agent.reasoning', ({ channelId, text, at }) => {
  activityStore.appendReasoning(channelId, text, at);
});
bus.on('agent.activity_history', ({ channelId, entries }) => {
  activityStore.replace(channelId, entries || []);
});
