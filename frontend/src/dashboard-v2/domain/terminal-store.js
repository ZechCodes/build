// Terminal output history + completion state per channel.
// See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('terminal');
const byChannel = new Map();       // channelId → {history, completions, running}

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { history: [], completions: [], running: false };
    byChannel.set(channelId, s);
  }
  return s;
}

export const terminalStore = {
  forChannel(channelId) {
    return byChannel.get(channelId) ?? { history: [], completions: [], running: false };
  },

  appendOutput(channelId, text) {
    const s = getSlot(channelId);
    s.history.push({ type: 'output', text, at: Date.now() });
    s.running = true;
    notify({ kind: 'output', channelId });
  },

  markComplete(channelId, exitCode, cwd) {
    const s = byChannel.get(channelId);
    if (!s) return;
    s.history.push({ type: 'complete', exitCode, at: Date.now() });
    s.running = false;
    if (cwd) s.cwd = cwd;
    notify({ kind: 'complete', channelId, exitCode, cwd });
  },

  setCompletions(channelId, candidates) {
    getSlot(channelId).completions = candidates;
    notify({ kind: 'completions', channelId });
  },

  clear(channelId) {
    if (!byChannel.has(channelId)) return;
    byChannel.set(channelId, { history: [], completions: [], running: false });
    notify({ kind: 'clear', channelId });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('terminal.output', ({ channelId, text }) => {
  if (!channelId || !text) return;
  terminalStore.appendOutput(channelId, text);
});
bus.on('terminal.complete', ({ channelId, exitCode, cwd }) => {
  if (!channelId) return;
  terminalStore.markComplete(channelId, exitCode, cwd);
});
bus.on('terminal.completions', ({ channelId, candidates }) => {
  if (!channelId) return;
  terminalStore.setCompletions(channelId, candidates || []);
});
