// Files tree + changeset per channel. See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable('files');
const byChannel = new Map();       // channelId → {tree, changes}

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { tree: new Map(), changes: [] };
    byChannel.set(channelId, s);
  }
  return s;
}

export const filesStore = {
  treeFor(channelId) { return byChannel.get(channelId)?.tree ?? new Map(); },
  changesFor(channelId) { return byChannel.get(channelId)?.changes ?? []; },

  setTree(channelId, path, entries) {
    getSlot(channelId).tree.set(path, entries);
    notify({ kind: 'tree', channelId, path });
  },

  setChanges(channelId, changes) {
    getSlot(channelId).changes = changes;
    notify({ kind: 'changes', channelId });
  },

  subscribe,
};
