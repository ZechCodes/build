// Single owner of "load a channel's initial data."
//
// Awaits session readiness for the channel's device, fires the five
// initial-data intents, and resolves when the first user-visible
// responses have arrived (or a timeout elapses).
//
// Consumers:
//   - Channel.load()        — initial mount and channel-switch.
//   - coordinator.js        — reconnect refresh after SSE recovery.
//   - Future: manual refresh, retry-after-error flows.
//
// The "first user-visible response" set is `message.bulk` (if we asked
// for it) plus `files.list_result`. Activity and complications stream
// in via their own bus events and are idempotent, so waiting on them
// isn't necessary for the loading → ready transition.

import { bus } from '../core/bus.js';
import { messagesStore } from '../domain/messages-store.js';
import { sessionStore } from '../core/session-store.js';

const DEFAULT_TIMEOUT_MS = 5000;

/**
 * @param {string} channelId
 * @param {object} [opts]
 * @param {AbortSignal} [opts.signal]
 * @param {number} [opts.timeoutMs=5000]
 * @param {boolean} [opts.forceFetchMessages=false]
 */
export async function loadChannel(channelId, opts = {}) {
  const {
    signal,
    timeoutMs = DEFAULT_TIMEOUT_MS,
    forceFetchMessages = false,
  } = opts;

  await sessionStore.awaitChannelReady(channelId, { signal });
  if (signal?.aborted) return;

  const firstResponse = _awaitFirstResponses(
    channelId, signal, timeoutMs, forceFetchMessages,
  );
  _fireLoadIntents(channelId, forceFetchMessages);

  try { await firstResponse; }
  catch (err) {
    if (err?.name !== 'AbortError') throw err;
  }
}

function _fireLoadIntents(channelId, forceFetchMessages) {
  if (forceFetchMessages || messagesStore.forChannel(channelId).length === 0) {
    bus.emit('intent.get_messages', { channelId });
  }
  bus.emit('intent.get_activity', { channelId });
  bus.emit('intent.get_complications', { channelId });
  bus.emit('intent.files_list', { channelId, path: '' });
  bus.emit('intent.files_changes', { channelId });
}

/**
 * Resolve when `message.bulk` (if requested) and `files.list_result`
 * have arrived for this channel, or when `timeoutMs` elapses. Rejects
 * with AbortError if `signal` aborts.
 */
function _awaitFirstResponses(channelId, signal, timeoutMs, forceFetchMessages) {
  const needMessages = forceFetchMessages
    || messagesStore.forChannel(channelId).length === 0;
  return new Promise((resolve, reject) => {
    let gotMessages = !needMessages;
    let gotFilesList = false;
    const cleanup = () => {
      offMessages();
      offFilesList();
      signal?.removeEventListener?.('abort', onAbort);
      clearTimeout(timer);
    };
    const done = () => { cleanup(); resolve(); };
    const onAbort = () => {
      cleanup();
      const err = new Error('aborted');
      err.name = 'AbortError';
      reject(err);
    };
    const offMessages = bus.on('message.bulk', (e) => {
      if (e?.channelId !== channelId) return;
      gotMessages = true;
      if (gotMessages && gotFilesList) done();
    });
    const offFilesList = bus.on('files.list_result', (e) => {
      if (e?.channelId !== channelId) return;
      gotFilesList = true;
      if (gotMessages && gotFilesList) done();
    });
    const timer = setTimeout(() => { cleanup(); resolve(); }, timeoutMs);
    if (signal?.aborted) return onAbort();
    signal?.addEventListener?.('abort', onAbort);
  });
}
