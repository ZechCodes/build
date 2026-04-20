import { escapeHtml, escHtml, formatBytes, formatFileSize } from './util/html.js';
import { timeAgo, shortTime, fmtRelativeAgo, fmtClock24 } from './util/time.js';
import { showToast } from './util/toast.js';
import { highlightLine } from './files/syntax.js';
import { effortLabel, modelToFriendlyName, compareVersions } from './util/format.js';
import { agentShortName, describeToolUse, formatToolDetail, formatToolResult, toolTag } from './console/tools.js';
import {
  state,
  MAX_ACTIVITY,
  SORT_STABILITY_MS,
  CHANNEL_STATE_KEY,
  THEME_VERSION_KEY,
  MAX_FILE_SIZE,
  CHAT_OVERLAY_STATE_KEY,
  CONSOLE_RECENT_COUNT,
} from './state.js';
import { closeAllDropdowns, positionDropdownMenu, toggleDropdown } from './shell/dropdown.js';
import { setSidebarSectionOpen, toggleSidebarSection, expandSidebarSection } from './shell/sidebar.js';
import {
  isChatOverlayOpen,
  openChatOverlay,
  closeChatOverlay,
  toggleChatOverlay,
  setChatOverlayPinned,
  toggleChatOverlayExpanded,
  syncChatOverlayHeader,
  applyChannelHarnessInfo,
} from './chat/overlay.js';
import './shell/update-badge.js';
import { renderTasksPanel, updateTasksBadge } from './tasks/panel.js';
import {
  getTerminalCwd,
  shortCwd,
  renderTerminalCwd,
  renderTerminalForChannel,
  terminalExec,
  clearTerminalTimers,
  finishTerminalCommand,
  clearTerminalCompletions,
} from './terminal/terminal.js';
import {
  renderComplications,
  renderGitCompChip,
  toggleCompPopover,
  closeCompPopover,
  sendComplicationAction,
} from './complications/render.js';
import {
  addBrowserTab,
  removeBrowserTab,
  selectBrowserTab,
  createBrowserTabItem,
} from './browser/tabs.js';
import {
  filesLoadRoot,
  renderFileTree,
  renderTreeLevel,
  renderChangesView,
  toggleDirectory,
  selectFile,
  setFilesMode,
  updateFloatingToggle,
  updateFilesModifiedCount,
  onFilesTabActivated,
  renderFileContent,
  renderPreview,
  renderMarkdownFile,
  renderSvgPreview,
  readFileAsync,
  resolveAssetPath,
  urlFetchAsync,
  resolveUrl,
  showBrowserView,
  renderDiffContent,
  onAgentFileChanges,
  navigateToFile,
} from './files/view.js';
import { renderChannelPanel, createChannelItem, updateMobileChannelLabel } from './channels/panel.js';
import { showEditChannelDialog } from './channels/edit-dialog.js';
import { renderDeviceCard } from './devices/render.js';
import { showEditDeviceDialog } from './devices/edit-dialog.js';
import { initNotifications, sseSynced } from './net/notifications.js';
import { getE2EE, getActiveE2EE, anyE2EEConnected } from './e2ee/bridge.js';
import { fileContentBody } from './files/view.js';
import { switchTab } from './shell/tabs.js';
import {
  setConsoleState,
  updateConsoleButtons,
  updateRailButtonStates,
  updateChatRailUnreadBadge,
  renderConsoleTimes,
  toggleConsoleEntry,
  toggleTree,
} from './shell/rail.js';
import './shell/update-badge.js';
import './chat/input.js';
import {
  addPendingFiles,
  clearPendingFiles,
  renderPendingFiles,
  _bindUploadProgress,
} from './chat/uploads.js';
import {
  rebuildChatChannels,
  promoteChannel,
  getOrderedChannelIds,
  pushChannelHistory,
  renderChannelList,
  renderChannelSidebar,
  updateAggregateBadge,
  incrementUnread,
} from './channels/list.js';
import {
  getLastSeen,
  setLastSeen,
  deferMarkRead,
  markUnreadMessages,
  clearUnreadHighlights,
} from './channels/unread.js';
import {
  loadChannelState,
  saveChannelState,
  clearChannelDraft,
} from './channels/state-store.js';
import { selectChannel, selectAgent, dismissStaleSuggestions } from './channels/select.js';
import { sendChatMessage } from './chat/composer.js';
import './chat/new-channel-dialog.js';
import {
  renderMessages,
  appendMessage,
  appendSystemMessage,
  updateStopButton,
} from './chat/messages.js';
import {
  appendInteractionCard,
  appendPlanReviewCard,
  resolvePendingPlanReviews,
  respondToInteraction,
  crossfadeStatus,
  respondToInteractionFreeform,
  respondToMultiselectInteraction,
  updatePlanModeUI,
} from './chat/interactions.js';
import { syncE2EEStatus, connectDeviceE2EE, initE2EE } from './e2ee/connect.js';
import { bindE2EEEvents } from './e2ee/handlers.js';
import {
  clearConsole,
  appendConsoleReasoning,
  appendConsoleEntry,
  markConsoleEntryDone,
  isChatNearBottom,
  isConsoleNearBottom,
  scrollChatToBottom,
  scrollToMessage,
  handleNewMessageScroll,
  handleSentMessageScroll,
  showChatBubble,
  hideChatBubble,
  showActivityBubble,
  hideActivityBubble,
  updateChatScrollArrow,
  updateActivityScrollArrow,
  scrollToFirstUnread,
  createEmptyState,
} from './console/view.js';

// Electron detection
if (window.buildElectron) {
  document.body.classList.add('electron');
}

// ===== UI State =====
// 'files' is the default/main view. 'browser' is activated when a browser tab is selected.


// ===== Device State =====

// ===== DOM References =====



async function connectToDevice(device) {
  if (!state.e2eeConnections.has(device.id)) {
    await connectDeviceE2EE(device.id);
  }
  renderChannelPanel();
}



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

// Mobile channel dropdown — shows channel panel as a full-viewport overlay
document.getElementById('mobile-channel-trigger')?.addEventListener('click', (e) => {
  e.stopPropagation();
  const panel = document.getElementById('channel-panel');
  if (panel) {
    panel.classList.toggle('mobile-open');
  }
});
// Close button (visible in mobile-open state)
document.getElementById('channel-panel-close')?.addEventListener('click', (e) => {
  e.stopPropagation();
  document.getElementById('channel-panel')?.classList.remove('mobile-open');
});
// Click outside closes mobile channel dropdown (no-op on mobile where the
// sidebar fills the viewport; click the close button or pick a channel instead).
document.addEventListener('click', (e) => {
  const panel = document.getElementById('channel-panel');
  if (panel && panel.classList.contains('mobile-open')) {
    if (!panel.contains(e.target) && !e.target.closest('#mobile-channel-trigger')) {
      panel.classList.remove('mobile-open');
    }
  }
});


// ===== Chat overlay =====
// The chat-layout originally lives inside #tab-chat; move it into the overlay body on load.
// Device-status overlays (skeleton + reconnect pill) are hoisted to the body so
// they sit above the whole app, not just the chat pane — they reflect the
// selected channel's device, not chat state.
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


// ===== Review-flow UI stubs =====
document.getElementById('files-approve-all-btn')?.addEventListener('click', () => showToast('Review flow not wired up yet'));
document.getElementById('files-view-pr-btn')?.addEventListener('click', () => showToast('View PR not wired up yet'));



// Complications scroll fade indicators
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



// ----- Helpers -----

// ----- Rendering -----


function renderDeviceGrid() { renderChannelPanel(); }
function updateStats() { /* no-op, dashboard removed */ }
function addActivity(text, timestamp) { /* no-op, dashboard activity feed removed */ }

// ----- Initial data fetch -----

async function fetchDevices() {
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

// ----- Skrift notification event handler -----

document.addEventListener('sk:notification', (e) => {
  const data = e.detail;
  const type = data.type || '';

  // Only handle build:device:* events
  if (!type.startsWith('build:device:')) return;

  // Prevent default toast rendering — we handle it ourselves
  e.preventDefault();

  const eventType = type.replace('build:device:', '');
  const deviceId = data.device_id;
  const deviceName = data.device_name || '';

  switch (eventType) {
    case 'authorized': {
      // New device authorized — fetch fresh data to get full record
      fetchDevices();
      addActivity(
        `<strong>${escapeHtml(deviceName)}</strong> authorized`,
        data.created_at ? new Date(data.created_at * 1000).toISOString() : null,
      );
      break;
    }

    case 'online': {
      const device = state.devices.get(deviceId);
      if (device) {
        device.status = 'online';
        device.last_heartbeat_at = new Date().toISOString();
      } else {
        // Unknown device — refetch
        fetchDevices();
      }
      renderDeviceGrid();
      updateStats();
      addActivity(
        `<strong>${escapeHtml(deviceName)}</strong> came <span class="text-green">online</span>`,
        data.created_at ? new Date(data.created_at * 1000).toISOString() : null,
      );
      // If this was the down device, clear banner (e2ee-ready will handle reconnect).
      if (state.deviceDown && state.deviceDown.id === deviceId) {
        state.deviceDown = null;
        const banner = document.getElementById('device-down-banner');
        if (banner) banner.classList.remove('visible');
      }
      break;
    }

    case 'offline': {
      const device = state.devices.get(deviceId);
      if (device) {
        device.status = 'offline';
      }
      renderDeviceGrid();
      updateStats();
      addActivity(
        `<strong>${escapeHtml(deviceName)}</strong> went <span class="text-red">offline</span>`,
        data.created_at ? new Date(data.created_at * 1000).toISOString() : null,
      );
      // If we have an E2EE connection for this device, disconnect it.
      const offlineConn = state.e2eeConnections.get(deviceId);
      if (offlineConn) {
        state.deviceDown = { id: deviceId, name: deviceName };
        const banner = document.getElementById('device-down-banner');
        const bannerText = document.getElementById('device-down-text');
        if (banner) banner.classList.add('visible');
        if (bannerText) bannerText.textContent = `${deviceName} disconnected — waiting for reconnection...`;
        offlineConn.disconnect(); // triggers disconnected handler which cleans up
        syncE2EEStatus();
      }
      break;
    }

    case 'heartbeat-missed': {
      const device = state.devices.get(deviceId);
      addActivity(
        `<strong>${escapeHtml(deviceName || deviceId)}</strong> <span class="text-amber">missed heartbeat</span> (${data.elapsed_s || '?'}s)`,
        data.created_at ? new Date(data.created_at * 1000).toISOString() : null,
      );
      // Refresh to get updated missed windows
      fetchDevices();
      break;
    }

    case 'e2ee-ready': {
      // Device uploaded transport key — update device record and trigger E2EE connect.
      const dev = state.devices.get(deviceId);
      if (dev) {
        dev.has_transport_key = true;
        dev.status = 'online';
      }
      renderDeviceGrid();
      // Clear device-down banner if this device was down.
      if (state.deviceDown && state.deviceDown.id === deviceId) {
        state.deviceDown = null;
        const banner = document.getElementById('device-down-banner');
        if (banner) banner.classList.remove('visible');
      }
      // Skip replayed e2ee-ready events during SSE flush — the SSE reconnect
      // handler will call initE2EE() after sync anyway.
      if (!sseSynced()) {
        console.log('[E2EE] Suppressing replayed e2ee-ready (SSE not yet synced)');
        break;
      }
      // Skip if the SSE reconnect handler is already tearing down and reiniting.
      if (_sseReconnecting) break;
      // Device re-announced — relay likely restarted, old session for this device is dead.
      const existingConn = state.e2eeConnections.get(deviceId);
      if (existingConn) {
        console.log('[E2EE] Device e2ee-ready while connected — relay restarted, reconnecting...');
        // Preserve current channel so it's restored after reconnect.
        if (state.chatCurrentChannel) state._pendingChannelId = state.chatCurrentChannel;
        existingConn.disconnect();
        state.e2eeConnections.delete(deviceId);
      }
      console.log('[E2EE] Device e2ee-ready notification — connecting...');
      connectDeviceE2EE(deviceId);
      break;
    }

    case 'renamed': {
      const device = state.devices.get(deviceId);
      if (device) device.name = deviceName;
      renderChannelPanel();
      break;
    }

    case 'revoked': {
      state.devices.delete(deviceId);
      const revokedConn = state.e2eeConnections.get(deviceId);
      if (revokedConn) revokedConn.disconnect();
      renderChannelPanel();
      break;
    }

    case 'status': {
      break;
    }

    default:
      console.log('Unhandled device event:', eventType, data);
  }
});

// ----- Periodic refresh -----
setInterval(() => {
  renderChannelPanel();
}, 15000);

// ----- Hash-based routing -----

function restoreFromHash() {
  const hash = location.hash.replace(/^#/, '');
  if (!hash) return;

  const parts = hash.split('/');
  const tab = parts[0];

  if (tab === 'chat' || tab === 'files' || tab === 'planning') {
    // No longer need state.selectedAgent — multi-device handles this.
    switchTab(tab);

    if (parts[1]) {
      state._pendingChannelId = parts[1];
      // If channels are already loaded, select immediately.
      if (state.chatChannels.has(state._pendingChannelId)) {
        selectChannel(state._pendingChannelId);
        state._pendingChannelId = null;
      }
    }
  }
}

// ----- Initialize -----
fetchDevices().then(() => {
  restoreFromHash();
  renderChannelPanel();
  initE2EE();
});

// When SSE reconnects (e.g. deploy, relay restart), tear down stale sessions and re-init.
let _sseReconnecting = false;
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
      // arrive when only the web app restarted (relay + state.devices stayed up).
      await fetchDevices();
      await initE2EE();
    } finally {
      _sseReconnecting = false;
    }
  }
});


// =====================================================================
// E2EE Chat — wired to BuildE2EE client
// =====================================================================

// Multi-device E2EE state




















// syncE2EEStatus is now defined above with initE2EE.
// MutationObservers no longer needed — syncE2EEStatus is called directly.





// ---- Expose helpers needed by vendor/markdown.js ----
window.highlightLine = highlightLine;

// ---- Expose helpers needed by feature modules during the transition ----
window.renderChannelPanel = renderChannelPanel;
window.switchTab = switchTab;
window.showBrowserView = showBrowserView;
window.connectToDevice = connectToDevice;
window.fetchDevices = fetchDevices;

// ---- Test bridges (Playwright smoke harness only) ----
// Used by frontend/tests/dashboard.smoke.mjs to inject a fake E2EE instance
// and exercise every bindE2EEEvents listener with synthetic envelopes.
window.__test_state = state;
window.__test_bindE2EEEvents = bindE2EEEvents;
