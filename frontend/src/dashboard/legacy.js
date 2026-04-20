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
        if (state.chatCurrentChannel) _pendingChannelId = state.chatCurrentChannel;
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
let _pendingChannelId = null;

function restoreFromHash() {
  const hash = location.hash.replace(/^#/, '');
  if (!hash) return;

  const parts = hash.split('/');
  const tab = parts[0];

  if (tab === 'chat' || tab === 'files' || tab === 'planning') {
    // No longer need state.selectedAgent — multi-device handles this.
    switchTab(tab);

    if (parts[1]) {
      _pendingChannelId = parts[1];
      // If channels are already loaded, select immediately.
      if (state.chatChannels.has(_pendingChannelId)) {
        selectChannel(_pendingChannelId);
        _pendingChannelId = null;
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






// cachedHarnesses per-device stored in state.deviceHarnesses Map


// ----- E2EE connection -----

function syncE2EEStatus() {
  const dot = document.getElementById('e2ee-dot');
  const label = document.getElementById('e2ee-label');
  if (!dot || !label) return;
  if (anyE2EEConnected()) {
    const count = [...state.e2eeConnections.values()].filter(c => c.connected).length;
    dot.className = 'e2ee-dot connected';
    label.textContent = count > 1 ? `E2EE active (${count} devices)` : 'E2EE active';
    document.getElementById('e2ee-waiting-overlay').classList.add('hidden');
  } else if (state.e2eeConnections.size > 0) {
    dot.className = 'e2ee-dot connecting';
    label.textContent = 'Connecting...';
  } else {
    dot.className = 'e2ee-dot connecting';
    label.textContent = 'Waiting for device...';
  }
}

async function connectDeviceE2EE(deviceId) {
  if (state.e2eeConnections.has(deviceId)) return;
  console.log('[E2EE] Connecting to device', deviceId);

  const instance = new BuildE2EE();
  try {
    await instance.ready();
  } catch (err) {
    console.error('[E2EE] libsodium not available:', err);
    return;
  }

  bindE2EEEvents(instance, deviceId);
  state.e2eeConnections.set(deviceId, instance);
  syncE2EEStatus();

  try {
    await instance.connect(deviceId);
    console.log('[E2EE] Connected to', deviceId);
  } catch (err) {
    console.error('[E2EE] Failed to connect to', deviceId, err);
    instance.disconnect(); // Clean up leaked notification handler on document.
    state.e2eeConnections.delete(deviceId);
    syncE2EEStatus();
  }
}

async function initE2EE(targetDeviceId = null) {
  console.log('[E2EE] Initializing...');

  if (typeof BuildE2EE === 'undefined') {
    const label = document.getElementById('e2ee-label');
    const dot = document.getElementById('e2ee-dot');
    if (label) label.textContent = 'E2EE script not loaded';
    if (dot) dot.className = 'e2ee-dot error';
    console.error('[E2EE] BuildE2EE class not found — e2ee.js failed to load');
    return;
  }

  if (targetDeviceId) {
    await connectDeviceE2EE(targetDeviceId);
    return;
  }

  // Connect to all online state.devices with transport keys in parallel.
  const candidates = [...state.devices.values()].filter(d => d.status === 'online' && d.has_transport_key);
  if (candidates.length === 0) {
    console.log('[E2EE] No device ready, waiting for e2ee-ready notification');
    syncE2EEStatus();
    const skelText = document.querySelector('.skel-status-text');
    if (skelText) skelText.textContent = 'Waiting for device…';
    // Retry: device may not have sent e2ee-ready yet after SSE reconnect.
    clearTimeout(initE2EE._retryTimer);
    initE2EE._retryTimer = setTimeout(async () => {
      if (state.e2eeConnections.size > 0) return; // Already connected via e2ee-ready
      console.log('[E2EE] Retrying — refreshing devices...');
      await fetchDevices();
      if (state.e2eeConnections.size === 0) initE2EE();
    }, 3000);
    return;
  }
  clearTimeout(initE2EE._retryTimer);

  await Promise.allSettled(candidates.map(d => connectDeviceE2EE(d.id)));
}

function bindE2EEEvents(instance, deviceId) {
  if (!instance) return;
  instance.addEventListener('connected', () => {
    console.log('[E2EE] Session established for device', deviceId);
    state.e2eeHasConnected = true;
    const _enableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
    for (const id of _enableBtns) {
      const el = document.getElementById(id);
      if (el) el.disabled = false;
    }
    document.getElementById('e2ee-waiting-overlay').classList.add('hidden');
    // Hide reconnect pill + cancel timer.
    clearTimeout(state._reconnectPillTimer);
    state._reconnectPillTimer = null;
    document.getElementById('reconnect-pill').classList.remove('visible');
    // Clear device-down banner on successful reconnect.
    state.deviceDown = null;
    const _ddb = document.getElementById('device-down-banner');
    if (_ddb) _ddb.classList.remove('visible');
    if (typeof _bindUploadProgress === 'function') _bindUploadProgress(instance);
    syncE2EEStatus();
    renderChannelPanel();
    instance.listChannels();
    instance.listHarnesses();
    // Re-fetch history for the current channel if it belongs to this device.
    if (state.chatCurrentChannel && state.channelDeviceMap.get(state.chatCurrentChannel) === deviceId) {
      state.chatLoadingMessages.add(state.chatCurrentChannel);
      state.chatLoadingActivity.add(state.chatCurrentChannel);
      instance.getMessages(state.chatCurrentChannel);
      instance.getActivity(state.chatCurrentChannel);
      // If the files tab is active, retry the tree/changes fetch — the initial
      // selectChannel() may have run before this `connected` flag flipped.
      if (state.currentTab === 'files') {
        state.filesChannelId = null;
        if (typeof onFilesTabActivated === 'function') onFilesTabActivated();
      }
    }
  });

  instance.addEventListener('disconnected', () => {
    console.log('[E2EE] Disconnected device', deviceId);
    state.e2eeConnections.delete(deviceId);
    // Keep state.deviceChannels and state.channelDeviceMap cached — only clear on server-reported removal.
    state.deviceHarnesses.delete(deviceId);
    state.deviceAgentCwd.delete(deviceId);

    if (!anyE2EEConnected()) {
      const _disableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
      for (const id of _disableBtns) {
        const el = document.getElementById(id);
        if (el) el.disabled = true;
      }
      if (state.deviceDown) {
        // Device is known to be offline — keep banner visible, skip skeleton/pill.
      } else if (!state.e2eeHasConnected) {
        // First load — show skeleton.
        document.getElementById('e2ee-waiting-overlay').classList.remove('hidden');
      } else {
        // Reconnection — show pill after 2s delay.
        clearTimeout(state._reconnectPillTimer);
        state._reconnectPillTimer = setTimeout(() => {
          if (!anyE2EEConnected()) {
            document.getElementById('reconnect-pill').classList.add('visible');
          }
        }, 2000);
      }
    }
    syncE2EEStatus();
    renderChannelPanel();
  });

  // ----- Channel list -----

  instance.addEventListener('channel_list', (evt) => {
    const { channels, agent_cwd } = evt.detail;
    if (agent_cwd) state.deviceAgentCwd.set(deviceId, agent_cwd);
    // Diff against cached channels to detect removals.
    const oldChans = state.deviceChannels.get(deviceId);
    const oldIds = oldChans ? new Set(oldChans.keys()) : new Set();
    // Update per-device channel map.
    const devChans = new Map();
    for (const ch of channels) {
      devChans.set(ch.id, ch);
      state.channelDeviceMap.set(ch.id, deviceId);
      if (ch.plan_mode != null) state.channelPlanMode.set(ch.id, ch.plan_mode);
      if (ch.last_seen_at) state.channelLastSeen.set(ch.id, ch.last_seen_at);
      // Reset agent active state — real-time events will re-set if agent is mid-turn.
      state.channelAgentActive.set(ch.id, false);
    }
    updateStopButton();
    state.deviceChannels.set(deviceId, devChans);
    // Clean up channels removed by the device.
    for (const oldId of oldIds) {
      if (!devChans.has(oldId)) {
        state.channelDeviceMap.delete(oldId);
        state.chatMessages.delete(oldId);
        state.unreadCounts.delete(oldId);
        state.channelSortTs.delete(oldId);
      }
    }
    rebuildChatChannels();
    renderChannelList();
    // If current channel was removed by the device, auto-select another.
    if (state.chatCurrentChannel && oldIds.has(state.chatCurrentChannel) && !devChans.has(state.chatCurrentChannel)) {
      const remaining = [...state.chatChannels.values()];
      if (remaining.length > 0) selectChannel(remaining[0].id);
      else { state.chatCurrentChannel = null; renderMessages(); }
    }
    // Restore pending channel from hash.
    if (_pendingChannelId && state.chatChannels.has(_pendingChannelId)) {
      selectChannel(_pendingChannelId);
      _pendingChannelId = null;
    } else if (!state.chatCurrentChannel && !_pendingChannelId && channels.length > 0) {
      // Only auto-select first channel if there's no pending channel waiting for another device.
      selectChannel(channels[0].id);
    }
    // Fetch messages for all non-current channels to compute unread counts.
    for (const ch of channels) {
      if (ch.id !== state.chatCurrentChannel && instance.connected) {
        instance.getMessages(ch.id);
      }
    }
  });

  instance.addEventListener('channel_created', (evt) => {
    const ch = evt.detail;
    state.channelDeviceMap.set(ch.id, deviceId);
    const devChans = state.deviceChannels.get(deviceId) || new Map();
    devChans.set(ch.id, ch);
    state.deviceChannels.set(deviceId, devChans);
    rebuildChatChannels();
    renderChannelList();
    selectChannel(ch.id);
  });

  // ----- Harness & Agent events -----

  instance.addEventListener('harness_list', (evt) => {
    state.deviceHarnesses.set(deviceId, evt.detail);
    console.log('[E2EE] Harnesses for', deviceId, ':', evt.detail.map(h => h.name));
  });

  instance.addEventListener('agent_started', (evt) => {
    console.log('[E2EE] Agent started:', evt.detail);
  });

  instance.addEventListener('agent_stopped', (evt) => {
    console.log('[E2EE] Agent stopped:', evt.detail);
    const ch = evt.detail?.channel_id;
    if (ch) {
      state.channelAgentActive.set(ch, false);
      updateStopButton();
    }
  });

  instance.addEventListener('agent_restarted', (evt) => {
    console.log('[E2EE] Agent restarted:', evt.detail);
  });

  instance.addEventListener('channel_renamed', (evt) => {
    const { channel_id, name } = evt.detail;
    const ch = state.chatChannels.get(channel_id);
    if (ch) {
      ch.name = name;
      const devChans = state.deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) devChans.get(channel_id).name = name;
      renderChannelList();
      renderChannelSidebar();
    }
  });

  instance.addEventListener('channel_updated', (evt) => {
    const { channel_id, model, effort, working_directory } = evt.detail;
    const ch = state.chatChannels.get(channel_id);
    if (ch) {
      if (model) ch.model = model;
      if (effort !== undefined) ch.effort = effort;
      if (working_directory !== undefined) ch.working_directory = working_directory;
      const devChans = state.deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) {
        const dc = devChans.get(channel_id);
        if (model) dc.model = model;
        if (effort !== undefined) dc.effort = effort;
        if (working_directory !== undefined) dc.working_directory = working_directory;
      }
    }
    // If this is the active channel, refresh the chat overlay model/effort pills.
    if (state.chatCurrentChannel === channel_id) applyChannelHarnessInfo(channel_id);
  });

  instance.addEventListener('channel_deleted', (evt) => {
    const { channel_id } = evt.detail;
    state.channelDeviceMap.delete(channel_id);
    const devChans = state.deviceChannels.get(deviceId);
    if (devChans) devChans.delete(channel_id);
    rebuildChatChannels();
    state.unreadCounts.delete(channel_id);
    if (state.chatCurrentChannel === channel_id) {
      const remaining = [...state.chatChannels.values()];
      if (remaining.length > 0) {
        selectChannel(remaining[0].id);
      } else {
        state.chatCurrentChannel = null;
        renderMessages();
      }
    }
    renderChannelList();
    renderChannelSidebar();
  });

  instance.addEventListener('e2ee_error', (evt) => {
    console.error('[E2EE] Error:', evt.detail);
  });

// ----- Messages -----

  instance.addEventListener('messages', (evt) => {
    const { channel_id, messages } = evt.detail;
    state.chatLoadingMessages.delete(channel_id);
    state.chatMessages.set(channel_id, messages);
    // Seed sort timestamp from newest message on initial load (don't override live promotions).
    if (!state.channelSortTs.has(channel_id) && messages.length) {
      const newest = messages[messages.length - 1];
      const ts = typeof newest.created_at === 'number' ? newest.created_at * 1000 : Date.parse(newest.created_at);
      if (ts) state.channelSortTs.set(channel_id, ts);
    }
    if (state.chatCurrentChannel === channel_id) {
      renderMessages();
      // Defer marking unread device messages as read until user interacts.
      const unread = messages
        .filter(m => m.sender !== 'client' && !m.read_at)
        .map(m => m.id)
        .filter(Boolean);
      if (unread.length && instance.connected) {
        const _inst = instance;
        deferMarkRead(() => _inst.markRead(unread));
      }
    } else {
      // Compute unread count for background channels using lastSeen timestamp.
      const lastSeen = getLastSeen(channel_id);
      const unreadMsgs = messages.filter(m => {
        if (m.sender === 'client') return false;
        if (!lastSeen) return true; // Never seen — all agent messages are unread.
        const msgTime = typeof m.created_at === 'number'
          ? new Date(m.created_at * 1000).toISOString()
          : m.created_at;
        return msgTime > lastSeen;
      });
      const hasInteraction = unreadMsgs.some(m => {
        try {
          const meta = typeof m.metadata === 'string' ? JSON.parse(m.metadata) : m.metadata;
          return meta && meta.interaction_id && !meta.resolved_at;
        } catch { return false; }
      });
      if (unreadMsgs.length > 0 || hasInteraction) {
        state.unreadCounts.set(channel_id, { messages: unreadMsgs.length, hasInteraction });
      } else {
        state.unreadCounts.delete(channel_id);
      }
      renderChannelList();
    }
  });

  instance.addEventListener('message', (evt) => {
    const msg = evt.detail;
    if (!msg || !msg.channel_id) return;
    const msgs = state.chatMessages.get(msg.channel_id) || [];
    // Dedup: skip if message with same ID already exists.
    if (msg.id && msgs.some(m => m.id === msg.id)) return;
    msgs.push(msg);
    state.chatMessages.set(msg.channel_id, msgs);
    promoteChannel(msg.channel_id);
    if (state.chatCurrentChannel === msg.channel_id) {
      const wasNearBottom = isChatNearBottom();
      appendMessage(msg);
      const _newEl = document.getElementById('chat-messages').lastElementChild;
      handleNewMessageScroll(wasNearBottom, _newEl);
      const _msgChId = msg.channel_id;
      const _msgId = msg.id;
      const _inst = instance;
      deferMarkRead(() => {
        setLastSeen(_msgChId);
        if (_msgId) _inst.markRead([_msgId]);
      });
    } else if (msg.sender !== 'client') {
      incrementUnread(msg.channel_id);
    }
  });

  // Agent events (chat.response, activity.delta, tool.use, etc.) from spawned agents.
  instance.addEventListener('agent_event', (evt) => {
    const { channel_id, event_type, event: agentEvt } = evt.detail;
    if (!channel_id) return;

    if (event_type === 'chat.response') {
      // Agent sent a chat message — also means agent is active.
      if (!state.channelAgentActive.get(channel_id)) {
        state.channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      promoteChannel(channel_id);
      const msg = {
        id: agentEvt.id || '',
        channel_id,
        sender: agentEvt.sender || 'Device',
        content: agentEvt.content || '',
        created_at: new Date().toISOString(),
      };
      if (agentEvt.suggested_actions?.length) msg.suggested_actions = agentEvt.suggested_actions;
      const msgs = state.chatMessages.get(channel_id) || [];
      if (msg.id && msgs.some(m => m.id === msg.id)) return;
      msgs.push(msg);
      state.chatMessages.set(channel_id, msgs);
      if (state.chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(msg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
        const _agChId = channel_id;
        deferMarkRead(() => setLastSeen(_agChId));
      } else {
        incrementUnread(channel_id);
      }
    } else if (event_type === 'activity.delta') {
      if (!state.channelAgentActive.get(channel_id)) {
        state.channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      if (state.chatCurrentChannel !== channel_id) return;
      const delta = agentEvt.delta || {};
      if (delta.type === 'text' && delta.text) {
        appendConsoleReasoning(delta.text, agentEvt.created_at || null);
      }
    } else if (event_type === 'tool.use') {
      if (!state.channelAgentActive.get(channel_id)) {
        state.channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      // Capture TodoWrite for any channel (before early return)
      const name = agentEvt.name || 'tool';
      const input = agentEvt.input || {};
      if (name === 'TodoWrite' && input.todos) {
        state.channelTodos.set(channel_id, input.todos);
        if (state.chatCurrentChannel === channel_id) {
          renderTasksPanel(channel_id);
          updateTasksBadge(channel_id);
        }
      }
      if (state.chatCurrentChannel !== channel_id) return;
      currentReasoningEntry = null;
      const desc = describeToolUse(name, input);
      appendConsoleEntry(agentEvt.tool_use_id, name, desc, input, agentEvt.created_at);
    } else if (event_type === 'tool.result') {
      if (state.chatCurrentChannel !== channel_id) return;
      markConsoleEntryDone(agentEvt.tool_use_id, agentEvt.is_error, agentEvt.content, agentEvt.completed_at);
    } else if (event_type === 'activity.end') {
      state.channelAgentActive.set(channel_id, false);
      updateStopButton();
      if (state.chatCurrentChannel !== channel_id) return;
    } else if (event_type === 'interaction.request') {
      // Agent is asking the user a question — render inline in chat.
      promoteChannel(channel_id);
      const msg = {
        id: agentEvt.interaction_id || '',
        channel_id,
        sender: agentEvt.sender || 'Device',
        content: agentEvt.question || '',
        created_at: new Date().toISOString(),
        metadata: JSON.stringify({
          interaction_id: agentEvt.interaction_id,
          kind: agentEvt.kind || 'question',
          options: agentEvt.options || [],
          allow_freeform: agentEvt.allow_freeform !== false,
          plan: agentEvt.plan || null,
          multiselect: !!agentEvt.multiselect,
        }),
      };
      const msgs = state.chatMessages.get(channel_id) || [];
      msgs.push(msg);
      state.chatMessages.set(channel_id, msgs);
      if (state.chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(msg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
        // On mobile, collapse the console to give more room for plan review cards,
        // then scroll the plan card to the top of the chat area.
        if ((agentEvt.kind || 'question') === 'plan_review' && window.innerWidth <= 768 && state.consoleState !== 'collapsed') {
          setConsoleState('collapsed');
          requestAnimationFrame(() => {
            if (_newEl) _newEl.scrollIntoView({ block: 'start', behavior: 'instant' });
          });
        }
      } else {
        incrementUnread(channel_id, true);
      }
    } else if (event_type === 'agent.error') {
      // Display agent errors as system messages in chat.
      const errMsg = {
        id: 'err-' + Date.now(),
        channel_id,
        sender: 'system',
        content: `**Agent error** — ${agentEvt.message || 'Unknown error'}`,
        created_at: new Date().toISOString(),
      };
      const msgs = state.chatMessages.get(channel_id) || [];
      msgs.push(errMsg);
      state.chatMessages.set(channel_id, msgs);
      if (state.chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(errMsg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
      } else {
        incrementUnread(channel_id, true);
      }
      if (agentEvt.fatal) {
        state.channelAgentActive.set(channel_id, false);
        updateStopButton();
      }
    } else if (event_type === 'agent.state_update') {
      const planMode = agentEvt.plan_mode;
      if (planMode != null) {
        state.channelPlanMode.set(channel_id, planMode);
        if (channel_id === state.chatCurrentChannel) updatePlanModeUI(planMode);
      }
      // Handle read notifications from agent.
      const readIds = agentEvt.read_message_ids;
      if (readIds && readIds.length) {
        for (const mid of readIds) {
          const statusEl = document.querySelector(`[data-msg-id="${mid}"] .msg-status`);
          if (statusEl) {
            crossfadeStatus(statusEl, 'read', '<span class="check active">✓</span><span class="check active">✓</span> Read');
          }
        }
      }
    } else if (event_type === 'agent.file_changes') {
      onAgentFileChanges(channel_id, agentEvt?.paths || []);
    }
  });

  // Activity history — tool use console entries loaded on channel select / reconnect.
  instance.addEventListener('activity_history', (evt) => {
    const { channel_id, entries } = evt.detail;
    state.chatLoadingActivity.delete(channel_id);
    if (state.chatCurrentChannel !== channel_id) return;

    clearConsole();
    for (const entry of entries) {
      if (entry.type === 'tool_use') {
        const d = entry.data || {};
        const name = d.name || 'tool';
        const input = d.input || {};
        // Capture TodoWrite from history (last one wins)
        if (name === 'TodoWrite' && input.todos) {
          state.channelTodos.set(channel_id, input.todos);
        }
        const desc = describeToolUse(name, input);
        appendConsoleEntry(d.id || '', name, desc, input, entry.created_at);
      } else if (entry.type === 'tool_result') {
        const d = entry.data || {};
        markConsoleEntryDone(d.tool_use_id || '', d.is_error || false, d.content, entry.created_at);
      } else if (entry.type === 'text') {
        const content = (entry.data?.text) || '';
        if (content) {
          currentReasoningEntry = null;
          appendConsoleReasoning(content, entry.created_at);
          currentReasoningEntry = null;
        }
      }
    }
    // Render tasks if we captured any TodoWrite calls from history.
    if (state.channelTodos.has(channel_id)) {
      renderTasksPanel(channel_id);
      updateTasksBadge(channel_id);
    }
    // Scroll to bottom so latest tool use is visible.
    const body = document.querySelector('[data-console-panel="activity"]');
    if (body) body.scrollTop = body.scrollHeight;
  });

  instance.addEventListener('delivered', (evt) => {
    const { message_id } = evt.detail;
    const statusEl = document.querySelector(`[data-msg-id="${message_id}"] .msg-status`);
    if (statusEl) {
      crossfadeStatus(statusEl, 'delivered', '<span class="check active">✓</span><span class="check">✓</span> Delivered');
    }
  });

  instance.addEventListener('read', (evt) => {
    const { message_ids } = evt.detail;
    for (const mid of (message_ids || [])) {
      const statusEl = document.querySelector(`[data-msg-id="${mid}"] .msg-status`);
      if (statusEl) {
        crossfadeStatus(statusEl, 'read', '<span class="check active">✓</span><span class="check active">✓</span> Read');
      }
    }
  });
  instance.addEventListener('delivery_failed', (evt) => {
    const { message_id, channel_id } = evt.detail;
    const statusEl = document.querySelector(`[data-msg-id="${message_id}"] .msg-status`);
    if (statusEl) {
      const retryHtml = `<button class="retry-btn" data-retry-msg="${message_id}" data-retry-ch="${channel_id}">Failed to reach agent — tap to retry</button>`;
      crossfadeStatus(statusEl, 'failed', retryHtml);
      statusEl.querySelector('.retry-btn')?.addEventListener('click', async () => {
        const msgEl = document.querySelector(`[data-msg-id="${message_id}"]`);
        const content = msgEl?.querySelector('.msg-text')?.textContent?.trim();
        if (!content || !instance.connected) return;
        crossfadeStatus(statusEl, 'sending', '<span class="check">✓</span><span class="check">✓</span> Sending');
        try {
          await instance.send({ action: 'retry_message', channel_id, message_id });
        } catch (err) {
          console.error('[Chat] Retry failed:', err);
          crossfadeStatus(statusEl, 'failed', retryHtml);
        }
      });
    }
  });

  instance.addEventListener('plan_mode_updated', (evt) => {
    const { channel_id, plan_mode } = evt.detail;
    state.channelPlanMode.set(channel_id, plan_mode);
    if (channel_id === state.chatCurrentChannel) updatePlanModeUI(plan_mode);
  });

  instance.addEventListener('system_message', (evt) => {
    const { channel_id, text } = evt.detail;
    if (state.chatCurrentChannel === channel_id) {
      appendSystemMessage(text);
      scrollChatToBottom();
    }
  });

  instance.addEventListener('session_reset', (evt) => {
    const { channel_id } = evt.detail;
    if (state.chatCurrentChannel === channel_id) {
      // Remove any "Compacting session..." indicator.
      const compacting = document.getElementById('compact-indicator');
      if (compacting) compacting.remove();

      const messagesEl = document.getElementById('chat-messages');
      const divider = document.createElement('div');
      divider.className = 'session-divider';
      divider.textContent = 'New session started';
      messagesEl.appendChild(divider);
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  });

  instance.addEventListener('compact_started', (evt) => {
    const { channel_id } = evt.detail;
    if (state.chatCurrentChannel === channel_id) {
      const messagesEl = document.getElementById('chat-messages');
      const indicator = document.createElement('div');
      indicator.id = 'compact-indicator';
      indicator.className = 'session-divider';
      indicator.textContent = 'Compacting session...';
      messagesEl.appendChild(indicator);
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  });

  // ----- Complication events -----
  instance.addEventListener('complication_update', (evt) => {
    const comp = evt.detail;
    const channelId = comp.channel_id;
    if (!channelId) return;
    if (!state.complicationState.has(channelId)) state.complicationState.set(channelId, new Map());
    state.complicationState.get(channelId).set(comp.id, comp);
    if (state.chatCurrentChannel === channelId) renderComplications();
  });

  instance.addEventListener('complication_remove', (evt) => {
    const { channel_id, id } = evt.detail;
    const channelComps = state.complicationState.get(channel_id);
    if (channelComps) {
      channelComps.delete(id);
      if (state.chatCurrentChannel === channel_id) renderComplications();
    }
  });

  instance.addEventListener('complications', (evt) => {
    const { channel_id, complications } = evt.detail;
    if (!channel_id || !complications) return;
    if (!state.complicationState.has(channel_id)) state.complicationState.set(channel_id, new Map());
    const channelComps = state.complicationState.get(channel_id);
    for (const comp of complications) {
      channelComps.set(comp.id, comp);
    }
    if (state.chatCurrentChannel === channel_id) renderComplications();
  });

  // ----- Terminal Output -----
  instance.addEventListener('terminal_output', (evt) => {
    const { channel_id, data, done, exit_code, cwd } = evt.detail;
    if (!channel_id) return;

    const sentinel = '__BUILD_CWD__';

    if (!done && data) {
      // First output received — clear loading animation and reset no-output timer.
      if (state.terminalCurrentBlock && !state.terminalCurrentBlock.hasOutput) {
        state.terminalCurrentBlock.hasOutput = true;
        clearTerminalTimers();
      }

      // Filter out sentinel line from streaming output.
      let text = data;
      if (text.includes(sentinel)) {
        text = text.split('\n').filter(l => !l.startsWith(sentinel)).join('\n');
        if (!text) return;
      }

      // Append to current streaming block if matching channel.
      if (state.terminalCurrentBlock && state.terminalCurrentBlock.channelId === channel_id) {
        state.terminalCurrentBlock.text += text;
        const span = document.createElement('span');
        span.textContent = text;
        state.terminalCurrentBlock.outputDiv.appendChild(span);
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
      }
      // Update stored history.
      const history = state.terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].output += text;
    }

    if (done) {
      clearTerminalTimers();
      document.getElementById('terminal-kill-btn')?.classList.add('hidden');
      // Update cwd.
      if (cwd) {
        state.terminalCwdMap.set(channel_id, cwd);
        if (state.chatCurrentChannel === channel_id) renderTerminalCwd();
      }
      // Update stored exit code.
      const history = state.terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].exitCode = exit_code;
      // Show exit code if non-zero.
      if (state.terminalCurrentBlock && state.terminalCurrentBlock.channelId === channel_id && exit_code !== 0) {
        const exitDiv = document.createElement('div');
        exitDiv.className = 'terminal-cmd-exit error';
        exitDiv.textContent = `exit ${exit_code}`;
        state.terminalCurrentBlock.block.appendChild(exitDiv);
      }
      if (state.terminalCurrentBlock?.channelId === channel_id) {
        state.terminalCurrentBlock = null;
      }
      state.terminalRunning = false;
      // Show prompt row again with updated cwd.
      if (state.chatCurrentChannel === channel_id) {
        const promptRow = document.getElementById('terminal-prompt-row');
        promptRow.classList.remove('hidden');
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
        document.getElementById('terminal-input')?.focus();
      }
    }
  });

  instance.addEventListener('terminal_completions', (evt) => {
    state.terminalCompletionPending = false;
    const { completions } = evt.detail;
    if (!completions || !completions.length) return;
    const input = document.getElementById('terminal-input');
    if (!input) return;
    // Use the context saved at request time (not the echoed partial)
    const beforePartial = state.terminalCompletionBase;
    const partial = state.terminalCompletionPartial;

    // Remove any previous completion display
    document.querySelectorAll('.terminal-completions').forEach(el => el.remove());

    if (completions.length === 1) {
      // Single match — substitute it in
      const match = completions[0];
      const suffix = match.endsWith('/') ? '' : ' ';
      input.value = beforePartial + match + suffix;
      clearTerminalCompletions();
    } else {
      // Multiple matches — find common prefix and complete that
      let common = completions[0];
      for (let i = 1; i < completions.length; i++) {
        while (common && !completions[i].startsWith(common)) {
          common = common.slice(0, -1);
        }
      }
      if (common.length > partial.length) {
        input.value = beforePartial + common;
      }
      // Store for Tab cycling
      state.terminalCompletions = completions;
      state.terminalCompletionIndex = -1;
      // Show candidates below the prompt
      const output = document.getElementById('terminal-output');
      if (output) {
        const compDiv = document.createElement('div');
        compDiv.className = 'terminal-completions';
        compDiv.textContent = completions.map(c => c.split('/').filter(Boolean).pop() + (c.endsWith('/') ? '/' : '')).join('  ');
        output.appendChild(compDiv);
        output.scrollTop = output.scrollHeight;
      }
    }
  });

  // ----- Files view events -----

  instance.addEventListener('files_list_result', (evt) => {
    const { channel_id, path, entries, error, truncated } = evt.detail;
    if (error) { console.warn('files_list error:', error); return; }
    if (channel_id !== state.filesChannelId) return;

    if (!state.fileTreeData.has(channel_id)) state.fileTreeData.set(channel_id, new Map());
    const data = state.fileTreeData.get(channel_id);
    data.set(path || '', { entries: entries || [], truncated: !!truncated });
    renderFileTree();

    // Restore saved file selection after root listing loads.
    if (state.filesPendingRestore && (path || '') === '') {
      const pendingPath = state.filesPendingRestore;
      const pendingView = state.filesPendingView;
      state.filesPendingRestore = null;
      state.filesPendingView = 'source';
      // Find the entry in the root listing.
      const match = (entries || []).find(e => e.name === pendingPath || e.path === pendingPath);
      if (match && match.type !== 'directory') {
        selectFile(match.path || match.name, match, pendingView);
      } else if (pendingPath.includes('/')) {
        // Nested path — select directly (tree won't highlight but file loads).
        selectFile(pendingPath, null, pendingView);
      }
    }
  });

  instance.addEventListener('files_changes_result', (evt) => {
    const { channel_id, repos } = evt.detail;
    console.debug('[files] files_changes_result', { channel_id, repos_count: repos?.length, filesChannelId: state.filesChannelId, chatCurrentChannel: state.chatCurrentChannel });
    // Accept if it matches state.filesChannelId, OR if state.filesChannelId is unset
    // but this is the currently-active chat channel (self-heal race).
    if (state.filesChannelId) {
      if (channel_id !== state.filesChannelId) return;
    } else if (channel_id !== state.chatCurrentChannel) {
      return;
    } else {
      state.filesChannelId = channel_id;
    }
    state.filesChangesData.set(channel_id, repos || []);
    updateFilesModifiedCount();
    if (state.filesTreeTab === 'changes') renderFileTree();
  });

  let _imageChunks = {};  // path -> { chunks: [], total: N }

  instance.addEventListener('file_read_result', (evt) => {
    const d = evt.detail;
    if (d.channel_id !== state.filesChannelId || d.path !== state.filesCurrentPath) return;
    if (state.filesCurrentView === 'diff') return;

    if (d.error) {
      fileContentBody.innerHTML = `<div class="empty-state"><p>${escapeHtml(d.error)}</p></div>`;
      return;
    }
    if (d.is_image && d.content) {
      // Handle chunked images.
      if (d.chunk_total && d.chunk_total > 1) {
        if (!_imageChunks[d.path] || _imageChunks[d.path].total !== d.chunk_total) {
          _imageChunks[d.path] = { chunks: new Array(d.chunk_total), total: d.chunk_total };
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading image... (0/' + d.chunk_total + ')</p></div>';
        }
        const state = _imageChunks[d.path];
        state.chunks[d.chunk_index] = d.content;
        const received = state.chunks.filter(Boolean).length;
        if (received < state.total) {
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading image... (' + received + '/' + state.total + ')</p></div>';
          return;
        }
        // All chunks received — reassemble.
        const fullDataUri = state.chunks.join('');
        delete _imageChunks[d.path];
        fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + fullDataUri + '" alt="' + escapeHtml(d.path) + '"></div>';
        return;
      }
      fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + d.content + '" alt="' + escapeHtml(d.path) + '"></div>';
      return;
    }
    if (d.is_binary) {
      fileContentBody.innerHTML = '<div class="empty-state"><p>Binary file (' + formatBytes(d.size) + ')</p></div>';
      return;
    }
    renderFileContent(d.content, d.path, d.size, d.truncated);
  });

  instance.addEventListener('file_diff_result', (evt) => {
    const d = evt.detail;
    if (d.channel_id !== state.filesChannelId || d.path !== state.filesCurrentPath) return;
    if (state.filesCurrentView !== 'diff') return;

    if (!d.diff) {
      fileContentBody.innerHTML = '<div class="empty-state"><p>No changes</p></div>';
      return;
    }
    renderDiffContent(d.diff, d.truncated);
  });
} // end bindE2EEEvents














// syncE2EEStatus is now defined above with initE2EE.
// MutationObservers no longer needed — syncE2EEStatus is called directly.





// ---- Expose helpers needed by vendor/markdown.js ----
window.highlightLine = highlightLine;

// ---- Expose helpers needed by feature modules during the transition ----
window.getActiveE2EE = getActiveE2EE;
window.getE2EE = getE2EE;
window.renderChannelPanel = renderChannelPanel;
window.switchTab = switchTab;
window.showBrowserView = showBrowserView;
window.anyE2EEConnected = anyE2EEConnected;
window.connectToDevice = connectToDevice;
window.fetchDevices = fetchDevices;

// ---- Test bridges (Playwright smoke harness only) ----
// Used by frontend/tests/dashboard.smoke.mjs to inject a fake E2EE instance
// and exercise every bindE2EEEvents listener with synthetic envelopes.
window.__test_state = state;
window.__test_bindE2EEEvents = bindE2EEEvents;
