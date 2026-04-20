import { state } from '../state.js';
import { getE2EE } from '../e2ee/bridge.js';

export function getLastSeen(channelId) {
  return state.channelLastSeen.get(channelId) || null;
}

export function setLastSeen(channelId) {
  const now = new Date().toISOString();
  state.channelLastSeen.set(channelId, now);
  const conn = getE2EE(channelId);
  if (conn && conn.connected) conn.markSeen(channelId);
}

// Deferred read-marking: queue setLastSeen/markRead until user interacts.
let _deferredReadOps = [];
let _deferredReadListening = false;

function _flushDeferredReads() {
  const ops = _deferredReadOps.splice(0);
  if (ops.length === 0) return;
  if (!state._unreadHighlightLastSeen) {
    state._unreadHighlightLastSeen = state.scrollLastSeen || getLastSeen(state.chatCurrentChannel);
  }
  for (const fn of ops) fn();
  // Keep unread highlight visible for 60s after being marked as read.
  state._unreadHighlightUntil = Date.now() + 60000;
  setTimeout(() => {
    if (Date.now() >= state._unreadHighlightUntil) {
      state._unreadHighlightLastSeen = null;
      state._unreadHighlightUntil = null;
      clearUnreadHighlights();
    }
  }, 60000);
  if (_deferredReadOps.length === 0 && _deferredReadListening) {
    _deferredReadListening = false;
    for (const evt of ['keydown', 'click', 'mousemove', 'scroll']) {
      document.removeEventListener(evt, _flushDeferredReads, { capture: true });
    }
  }
}

export function deferMarkRead(fn) {
  _deferredReadOps.push(fn);
  if (!_deferredReadListening) {
    _deferredReadListening = true;
    for (const evt of ['keydown', 'click', 'mousemove', 'scroll']) {
      document.addEventListener(evt, _flushDeferredReads, { capture: true, once: false, passive: true });
    }
  }
}

export function markUnreadMessages(container) {
  // Use persisted highlight lastSeen if still within the 60s window,
  // otherwise scroll-capture, otherwise live.
  const lastSeen = state._unreadHighlightLastSeen || state.scrollLastSeen || getLastSeen(state.chatCurrentChannel);
  if (!lastSeen) return;
  const msgEls = container.querySelectorAll('.msg[data-created-at]');
  for (const el of msgEls) {
    if (el.dataset.createdAt > lastSeen && !el.classList.contains('unread')) {
      // Only highlight agent messages, not user messages.
      if (el.querySelector('.msg-avatar.user')) continue;
      el.classList.add('unread');
    }
  }
}

export function clearUnreadHighlights() {
  document.querySelectorAll('.msg.unread').forEach(el => el.classList.remove('unread'));
}
