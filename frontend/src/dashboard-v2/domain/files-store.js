// Files tree + changeset + current read/diff results per channel.
// See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('files');
const byChannel = new Map();       // channelId → {tree, changes, readResult, diffResult}

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { tree: new Map(), changes: [], readResult: null, diffResult: null };
    byChannel.set(channelId, s);
  }
  return s;
}

export const filesStore = {
  treeFor(channelId) { return byChannel.get(channelId)?.tree ?? new Map(); },
  changesFor(channelId) { return byChannel.get(channelId)?.changes ?? []; },
  readResultFor(channelId) { return byChannel.get(channelId)?.readResult ?? null; },
  diffResultFor(channelId) { return byChannel.get(channelId)?.diffResult ?? null; },

  setTree(channelId, path, entries) {
    getSlot(channelId).tree.set(path, entries);
    notify({ kind: 'tree', channelId, path });
  },

  setChanges(channelId, changes) {
    getSlot(channelId).changes = changes;
    notify({ kind: 'changes', channelId });
  },

  setReadResult(channelId, result) {
    getSlot(channelId).readResult = result;
    notify({ kind: 'read_result', channelId, path: result?.path });
  },

  setDiffResult(channelId, result) {
    getSlot(channelId).diffResult = result;
    notify({ kind: 'diff_result', channelId, path: result?.path });
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('files.list_result', ({ channelId, path, entries, truncated }) => {
  if (!channelId) return;
  filesStore.setTree(channelId, path || '', { entries: entries || [], truncated: !!truncated });
});
bus.on('files.changes_result', ({ channelId, repos }) => {
  if (!channelId) return;
  filesStore.setChanges(channelId, repos || []);
});
bus.on('files.read_result', (d) => {
  if (!d?.channel_id) return;
  filesStore.setReadResult(d.channel_id, d);
});
bus.on('files.diff_result', (d) => {
  if (!d?.channel_id) return;
  filesStore.setDiffResult(d.channel_id, d);
});
