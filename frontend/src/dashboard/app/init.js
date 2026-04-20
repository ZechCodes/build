// Initial bootstrap, periodic refresh, hash-based routing, dropdown wiring,
// chat-overlay relocate, complications scroll-fade indicators, SSE-reconnect
// session teardown. The thin glue that wires the dashboard together at load.

import { state } from '../state.js';
import { showToast } from '../util/toast.js';
import { switchTab } from '../shell/tabs.js';
import { closeAllDropdowns, toggleDropdown } from '../shell/dropdown.js';
import { renderChannelPanel } from '../channels/panel.js';
import { selectChannel } from '../channels/select.js';
import { initE2EE } from '../e2ee/connect.js';

// ----- Initial device fetch -----

export async function fetchDevices() {
  try {
    const resp = await fetch('/api/devices/');
    if (!resp.ok) return;
    const data = await resp.json();
    const freshIds = new Set(data.map(d => d.id));
    for (const id of state.devices.keys()) {
      if (!freshIds.has(id)) state.devices.delete(id);
    }
    for (const d of data) state.devices.set(d.id, d);
    renderChannelPanel();
  } catch (err) {
    console.error('Failed to fetch devices:', err);
  }
}

// ----- Hash-based routing -----

export function restoreFromHash() {
  const hash = location.hash.replace(/^#/, '');
  if (!hash) return;

  const parts = hash.split('/');
  const tab = parts[0];

  if (tab === 'chat' || tab === 'files' || tab === 'planning') {
    switchTab(tab);
    if (parts[1]) {
      state._pendingChannelId = parts[1];
      if (state.chatChannels.has(state._pendingChannelId)) {
        selectChannel(state._pendingChannelId);
        state._pendingChannelId = null;
      }
    }
  }
}

// ----- Top-bar dropdowns -----

document.getElementById('device-dropdown-trigger')?.addEventListener('click', (e) => {
  e.stopPropagation();
  toggleDropdown('device-dropdown');
});
document.getElementById('sidebar-device-trigger')?.addEventListener('click', (e) => {
  e.stopPropagation();
  toggleDropdown('sidebar-device');
});
document.getElementById('channel-dropdown-trigger')?.addEventListener('click', (e) => {
  e.stopPropagation();
  toggleDropdown('channel-dropdown');
});
document.getElementById('dd-new-channel')?.addEventListener('click', (e) => {
  e.stopPropagation();
  closeAllDropdowns();
  document.getElementById('btn-new-channel')?.click();
});

// ----- Mobile channel dropdown — full-viewport overlay over the panel -----

document.getElementById('mobile-channel-trigger')?.addEventListener('click', (e) => {
  e.stopPropagation();
  const panel = document.getElementById('channel-panel');
  if (panel) panel.classList.toggle('mobile-open');
});
document.getElementById('channel-panel-close')?.addEventListener('click', (e) => {
  e.stopPropagation();
  document.getElementById('channel-panel')?.classList.remove('mobile-open');
});
// Click outside closes the mobile dropdown.
document.addEventListener('click', (e) => {
  const panel = document.getElementById('channel-panel');
  if (panel && panel.classList.contains('mobile-open')) {
    if (!panel.contains(e.target) && !e.target.closest('#mobile-channel-trigger')) {
      panel.classList.remove('mobile-open');
    }
  }
});

// ----- Chat overlay relocate -----
// The chat-layout originally lives inside #tab-chat; move it into the overlay
// body on load. Device-status overlays (skeleton + reconnect pill) are hoisted
// to the body so they sit above the whole app, not just the chat pane — they
// reflect the selected channel's device, not chat state.
(function relocateChatLayout() {
  const src = document.getElementById('tab-chat');
  const dst = document.getElementById('chat-overlay-body');
  if (!src || !dst) return;
  const layout = src.querySelector('.chat-layout');
  if (layout && !dst.contains(layout)) dst.appendChild(layout);
  const waiting = document.getElementById('e2ee-waiting-overlay');
  if (waiting && waiting.parentElement !== document.body) document.body.appendChild(waiting);
  const reconnect = document.getElementById('reconnect-pill');
  if (reconnect && reconnect.parentElement !== document.body) document.body.appendChild(reconnect);
})();

// ----- Review-flow UI stubs (toast for now) -----

document.getElementById('files-approve-all-btn')?.addEventListener('click', () => showToast('Review flow not wired up yet'));
document.getElementById('files-view-pr-btn')?.addEventListener('click', () => showToast('View PR not wired up yet'));

// ----- Complications scroll fade indicators -----

(function() {
  const wrap = document.getElementById('complications');
  const scroll = document.getElementById('comp-scroll');
  if (!wrap || !scroll) return;
  function updateFades() {
    wrap.classList.toggle('fade-left', scroll.scrollLeft > 4);
    wrap.classList.toggle('fade-right', scroll.scrollLeft < scroll.scrollWidth - scroll.clientWidth - 4);
  }
  scroll.addEventListener('scroll', updateFades);
  new ResizeObserver(updateFades).observe(scroll);
  updateFades();
})();

// ----- Periodic refresh: re-render the channel panel every 15s -----

setInterval(() => {
  renderChannelPanel();
}, 15000);

// ----- SSE reconnect: tear down stale E2EE sessions and reconnect -----

let _sseReconnecting = false;
export function isSseReconnecting() { return _sseReconnecting; }

document.addEventListener('sk:notification-status', async (evt) => {
  if (evt.detail.status === 'connected' && state.e2eeConnections.size > 0 && !_sseReconnecting) {
    _sseReconnecting = true;
    console.log('[E2EE] SSE reconnected — tearing down stale sessions');
    try {
      // Channel state is cached — user stays on their current channel.
      for (const [, conn] of state.e2eeConnections) conn.disconnect();
      state.e2eeConnections.clear();
      // Refresh device list so initE2EE has current status/transport key data.
      // Without this, the browser waits for e2ee-ready notifications that never
      // arrive when only the web app restarted (relay + devices stayed up).
      await fetchDevices();
      await initE2EE();
    } finally {
      _sseReconnecting = false;
    }
  }
});

// ----- Initial bootstrap -----

fetchDevices().then(() => {
  restoreFromHash();
  renderChannelPanel();
  initE2EE();
});
