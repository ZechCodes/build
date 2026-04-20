import { state, CHAT_OVERLAY_STATE_KEY } from '../state.js';

const CHAT_EXPAND_SVG = '<path d="M9 2h5v5M7 14H2V9"/><path d="M14 2l-5 5M2 14l5-5"/>';
const CHAT_SHRINK_SVG = '<path d="M14 7h-5V2M2 9h5v5"/><path d="M14 2l-5 5M2 14l5-5"/>';

function _readChatOverlayState() {
  try {
    const raw = localStorage.getItem(CHAT_OVERLAY_STATE_KEY);
    if (!raw) return null;
    const s = JSON.parse(raw);
    return (s && typeof s === 'object') ? s : null;
  } catch (_) { return null; }
}

function _writeChatOverlayState(patch) {
  try {
    const cur = _readChatOverlayState() || {};
    localStorage.setItem(CHAT_OVERLAY_STATE_KEY, JSON.stringify({ ...cur, ...patch }));
  } catch (_) {}
}

export function isChatOverlayOpen() {
  return !!document.getElementById('chat-overlay')?.classList.contains('open');
}

export function openChatOverlay(opts) {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  ov.classList.add('open');
  ov.setAttribute('aria-hidden', 'false');
  const saved = _readChatOverlayState() || {};
  const pin = (opts && 'pinned' in opts) ? !!opts.pinned : (saved.pinned !== false);
  setChatOverlayPinned(pin, { persist: !opts?.suppressPersist });
  if (saved.mode === 'expanded') _setChatOverlayExpanded(true);
  if (!opts?.suppressPersist) _writeChatOverlayState({ open: true });
  window.updateRailButtonStates?.();
  setTimeout(() => document.getElementById('chat-input')?.focus(), 80);
}

export function closeChatOverlay(opts) {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  ov.classList.remove('open');
  ov.removeAttribute('data-mode');
  ov.setAttribute('aria-hidden', 'true');
  if (!opts?.suppressPersist) _writeChatOverlayState({ open: false, mode: null });
  window.updateRailButtonStates?.();
}

export function toggleChatOverlay() {
  if (isChatOverlayOpen()) closeChatOverlay();
  else openChatOverlay();
}

export function setChatOverlayPinned(pinned, opts) {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  ov.setAttribute('data-pinned', pinned ? 'true' : 'false');
  document.getElementById('chat-overlay-pin')?.classList.toggle('active', !!pinned);
  if (opts?.persist !== false) _writeChatOverlayState({ pinned: !!pinned });
}

function _setChatOverlayExpanded(expanded, opts) {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  const btn = document.getElementById('chat-overlay-expand');
  const svg = btn?.querySelector('svg');
  if (expanded) {
    ov.setAttribute('data-mode', 'expanded');
    btn?.classList.add('active');
    if (btn) btn.title = 'Shrink';
    if (svg) svg.innerHTML = CHAT_SHRINK_SVG;
  } else {
    ov.removeAttribute('data-mode');
    btn?.classList.remove('active');
    if (btn) btn.title = 'Expand';
    if (svg) svg.innerHTML = CHAT_EXPAND_SVG;
  }
  if (opts?.persist !== false) _writeChatOverlayState({ mode: expanded ? 'expanded' : null });
}

export function toggleChatOverlayExpanded() {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  _setChatOverlayExpanded(ov.getAttribute('data-mode') !== 'expanded');
}

export function syncChatOverlayHeader() {
  const nameEl = document.getElementById('chat-overlay-channel-name');
  if (!nameEl) return;
  const ch = (typeof state.chatChannels !== 'undefined' && state.chatCurrentChannel) ? state.chatChannels.get(state.chatCurrentChannel) : null;
  nameEl.textContent = ch ? (ch.name || state.chatCurrentChannel.slice(0, 8)) : 'Select a channel';
}

// Wiring
document.getElementById('chat-overlay-minimize')?.addEventListener('click', () => closeChatOverlay());
document.getElementById('chat-overlay-expand')?.addEventListener('click', toggleChatOverlayExpanded);
document.getElementById('chat-overlay-pin')?.addEventListener('click', () => {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  setChatOverlayPinned(ov.getAttribute('data-pinned') !== 'true');
});

// Restore persisted state on load.
(function initChatOverlayState() {
  const saved = _readChatOverlayState() || {};
  const shouldOpen = saved.open !== false;
  if (shouldOpen) openChatOverlay({ suppressPersist: true });
})();

// Click-outside closes overlay unless pinned or expanded.
document.addEventListener('mousedown', (e) => {
  const ov = document.getElementById('chat-overlay');
  if (!ov || !ov.classList.contains('open')) return;
  if (ov.getAttribute('data-pinned') === 'true') return;
  if (ov.getAttribute('data-mode') === 'expanded') return;
  if (ov.contains(e.target)) return;
  if (e.target.closest('#console-chat-toggle')) return;
  closeChatOverlay();
});
