// Git complications per channel. See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('complications');
const byChannel = new Map();       // channelId → Complication[]

export const complicationsStore = {
  forChannel(channelId) { return byChannel.get(channelId) ?? []; },

  set(channelId, complications) {
    byChannel.set(channelId, complications.slice());
    notify({ kind: 'set', channelId });
  },

  upsert(channelId, complication) {
    if (!complication?.id) return;
    const arr = (byChannel.get(channelId) ?? []).slice();
    const idx = arr.findIndex(c => c.id === complication.id);
    if (idx >= 0) arr[idx] = complication;
    else arr.push(complication);
    byChannel.set(channelId, arr);
    notify({ kind: 'upsert', channelId, id: complication.id });
  },

  remove(channelId, complicationId) {
    const arr = byChannel.get(channelId);
    if (!arr) return;
    const next = arr.filter(c => c.id !== complicationId);
    if (next.length === arr.length) return;
    byChannel.set(channelId, next);
    notify({ kind: 'remove', channelId, id: complicationId });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('complications.bulk', ({ channelId, complications }) => {
  if (!channelId) return;
  complicationsStore.set(channelId, complications || []);
});
bus.on('complication.upserted', ({ channelId, complication }) => {
  if (!channelId) return;
  complicationsStore.upsert(channelId, complication);
});
bus.on('complication.removed', ({ channelId, complicationId }) => {
  if (!channelId) return;
  complicationsStore.remove(channelId, complicationId);
});
