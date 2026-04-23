// Tab-focus re-sync. When the browser tab returns to visibility after
// being hidden, ask each connected E2EE instance to re-list channels
// and re-load the active channel's panels.
//
// Why: background tabs can silently miss streaming events — the browser
// throttles them, SSE drops server-side after inactivity timeouts, etc.
// The client's presenceStore then shows stale "Thinking…" or "not
// running" state until the next live event. A focus-driven listChannels
// arms `is_running` reconciliation via e2ee-dispatcher, and loadChannel
// refreshes the active channel's panels.
//
// A short ignore-window (<1s hidden) skips quick tab switches so we
// don't hammer the bridge for micro-flips in focus.

import { bus } from '../core/bus.js';
import { log } from '../core/log.js';
import { e2eePool } from './e2ee-pool.js';
import { uiStore } from '../domain/ui-store.js';
import { loadChannel } from './channel-loader.js';

const plog = log('focus-sync');
const MIN_HIDDEN_MS = 1000;

let bound = false;
let lastHiddenAt = 0;
let onVisibilityChange = null;

export function bindFocusSync() {
  if (bound) return;
  if (typeof document === 'undefined') return;
  bound = true;
  onVisibilityChange = () => {
    if (document.visibilityState === 'hidden') {
      lastHiddenAt = Date.now();
      return;
    }
    if (document.visibilityState !== 'visible') return;
    if (!lastHiddenAt) return;   // first visible tick on page load
    const hiddenFor = Date.now() - lastHiddenAt;
    lastHiddenAt = 0;
    if (hiddenFor < MIN_HIDDEN_MS) return;
    _refresh(hiddenFor);
  };
  document.addEventListener('visibilitychange', onVisibilityChange);
}

export function unbindFocusSync() {
  if (!bound) return;
  bound = false;
  document.removeEventListener('visibilitychange', onVisibilityChange);
  onVisibilityChange = null;
  lastHiddenAt = 0;
}

function _refresh(hiddenFor) {
  plog.info('tab back — refreshing snapshots after', hiddenFor, 'ms hidden');
  for (const inst of e2eePool.list()) {
    if (!inst.connected) continue;
    try { inst.listChannels(); } catch (err) { plog.debug('listChannels', err); }
  }
  const activeId = uiStore.getActiveChannel?.();
  if (activeId) {
    loadChannel(activeId, { forceFetchMessages: true }).catch(err => {
      if (err?.name !== 'AbortError') plog.error('focus reload failed', err);
    });
  }
  bus.emit('focus.resync', { hiddenFor });
}

// Test hook — lets tests drive the refresh without faking visibility.
export function _resetForTests() {
  bound = false;
  lastHiddenAt = 0;
  onVisibilityChange = null;
}
