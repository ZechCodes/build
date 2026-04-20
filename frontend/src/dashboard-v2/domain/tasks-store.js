// Todo list per channel. See planning/dashboard-v2/02-stores.md.
//
// In v1 todos come exclusively from TodoWrite tool calls, so the API is
// set-the-list (no per-item CRUD yet). Wave 6+ may add individual verbs
// once UI CRUD is wired.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('tasks');
const byChannel = new Map();       // channelId → Todo[]

export const tasksStore = {
  forChannel(channelId) { return byChannel.get(channelId) ?? []; },

  set(channelId, todos) {
    byChannel.set(channelId, todos.slice());
    notify({ kind: 'set', channelId });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('agent.todo_write', ({ channelId, todos }) => {
  if (!channelId || !Array.isArray(todos)) return;
  tasksStore.set(channelId, todos);
});
