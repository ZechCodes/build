// Git complications per channel. See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable('complications');
const byChannel = new Map();       // channelId → Complication[]

export const complicationsStore = {
  forChannel(channelId) { return byChannel.get(channelId) ?? []; },

  set(channelId, complications) {
    byChannel.set(channelId, complications.slice());
    notify({ kind: 'set', channelId });
  },

  subscribe,
};
