// Chat-message image cache, keyed by (channelId, path).
//
// The bridge persists `<build-image>` tags in chat_messages with an
// empty body — bytes flow lazily over the chat_image.fetch wire action
// at render time. The chat view asks this store for an image; if it
// isn't ready yet, the store dispatches the intent once and notifies
// subscribers when the data URI arrives.
//
// Records:
//   { status: 'loading' | 'ready' | 'error', dataUri?: string, error?: string }

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('chat-images');
const byChannel = new Map();   // channelId → Map<path, record>

function channelMap(channelId) {
  let m = byChannel.get(channelId);
  if (!m) {
    m = new Map();
    byChannel.set(channelId, m);
  }
  return m;
}

export const chatImagesStore = {
  /** @returns {{status:string,dataUri?:string,error?:string}|null} */
  get(channelId, path) {
    if (!channelId || !path) return null;
    return byChannel.get(channelId)?.get(path) ?? null;
  },

  /**
   * Ensure a fetch is in-flight or done for this (channel, path). Idempotent
   * — repeated calls during a `loading` or `ready` state are no-ops. If a
   * previous attempt errored, retries.
   */
  request(channelId, path) {
    if (!channelId || !path) return;
    const m = channelMap(channelId);
    const existing = m.get(path);
    if (existing?.status === 'loading' || existing?.status === 'ready') return;
    m.set(path, { status: 'loading' });
    notify({ kind: 'requested', channelId, path });
    bus.emit('intent.chat_image_fetch', { channelId, path });
  },

  _setReady(channelId, path, dataUri) {
    const m = channelMap(channelId);
    m.set(path, { status: 'ready', dataUri });
    notify({ kind: 'ready', channelId, path });
  },

  _setError(channelId, path, error) {
    const m = channelMap(channelId);
    m.set(path, { status: 'error', error });
    notify({ kind: 'error', channelId, path });
  },

  __resetForTests__() {
    byChannel.clear();
  },

  subscribe,
};

// Bridge sends back chunked frames; the e2ee dispatcher assembles them
// and emits this single event with the final data URI (or error).
bus.on('chat_image.received', ({ channelId, path, dataUri, error }) => {
  if (!channelId || !path) return;
  if (error) {
    chatImagesStore._setError(channelId, path, error);
    return;
  }
  if (dataUri) {
    chatImagesStore._setReady(channelId, path, dataUri);
  }
});
