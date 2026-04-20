// Files tree + changeset + current read/diff results per channel.
// See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('files');
const byChannel = new Map();       // channelId → {tree, changes, readResult, diffResult}
const imageChunks = new Map();     // channelId → Map(path → {chunks, total})

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { tree: new Map(), changes: [], readResult: null, diffResult: null };
    byChannel.set(channelId, s);
  }
  return s;
}

function chunkSlot(channelId) {
  let m = imageChunks.get(channelId);
  if (!m) { m = new Map(); imageChunks.set(channelId, m); }
  return m;
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

  setReadResultProgress(channelId, progress) {
    getSlot(channelId).readResult = { ...progress, _progress: true };
    notify({ kind: 'read_result_progress', channelId, path: progress.path });
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
  const channelId = d.channel_id;
  // Non-image or single-chunk → pass through.
  if (!d.is_image || !d.chunk_total || d.chunk_total <= 1) {
    filesStore.setReadResult(channelId, d);
    return;
  }
  // Chunked image accumulation.
  const bucket = chunkSlot(channelId);
  let rec = bucket.get(d.path);
  if (!rec || rec.total !== d.chunk_total) {
    rec = { chunks: new Array(d.chunk_total), total: d.chunk_total };
    bucket.set(d.path, rec);
  }
  if (typeof d.chunk_index === 'number' && d.content) {
    rec.chunks[d.chunk_index] = d.content;
  }
  const received = rec.chunks.filter(Boolean).length;
  if (received < rec.total) {
    filesStore.setReadResultProgress(channelId, {
      path: d.path,
      is_image: true,
      chunk_received: received,
      chunk_total: rec.total,
    });
    return;
  }
  const fullContent = rec.chunks.join('');
  bucket.delete(d.path);
  filesStore.setReadResult(channelId, { ...d, content: fullContent, chunk_total: undefined, chunk_index: undefined });
});
bus.on('files.diff_result', (d) => {
  if (!d?.channel_id) return;
  filesStore.setDiffResult(d.channel_id, d);
});
