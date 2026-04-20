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
} from './chat/overlay.js';
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
import { renderDeviceCard } from './devices/render.js';
import { initNotifications, sseSynced } from './net/notifications.js';
import { getE2EE, getActiveE2EE, anyE2EEConnected } from './e2ee/bridge.js';

// Electron detection
if (window.buildElectron) {
  document.body.classList.add('electron');
}

// ===== UI State =====
// 'files' is the default/main view. 'browser' is activated when a browser tab is selected.


// ===== Device State =====

// ===== DOM References =====
const consoleBtm = document.getElementById('console-bottom');
const consoleToggle = document.getElementById('console-toggle');
const consoleExpand = document.getElementById('console-expand');

// ===== Tab Navigation (files is default; chat is in overlay; browser is toggled) =====
function switchTab(tab) {
  state.currentTab = tab;
  // Only show panels that belong to the main viewer (not the detached chat).
  document.querySelectorAll('.tab-panel').forEach(p => {
    if (p.dataset.detached === 'true') return;
    p.classList.toggle('active', p.id === 'tab-' + tab);
  });
  if (tab === 'files') onFilesTabActivated();
  window.onTabSwitched?.(tab);
}


async function connectToDevice(device) {
  if (!state.e2eeConnections.has(device.id)) {
    await connectDeviceE2EE(device.id);
  }
  renderChannelPanel();
}

function selectAgent(name, device) {
  state.selectedAgent = { name, device };
  renderChannelPanel();
  switchTab('chat');
  if (state.chatCurrentChannel) {
    location.hash = `chat/${state.chatCurrentChannel}`;
  } else {
    location.hash = 'chat';
  }
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

function setConsoleState(next) {
  state.consoleState = next;
  const isCollapsed = next === 'collapsed';
  consoleBtm.classList.toggle('collapsed', isCollapsed);
  consoleBtm.classList.toggle('rail-only', isCollapsed);
  consoleBtm.classList.toggle('expanded', next === 'expanded');
  const term = document.querySelector('[data-console-panel="terminal"]');
  if (term) term.classList.toggle('hidden', isCollapsed);
  updateConsoleButtons();
  updateRailButtonStates();
}
function updateConsoleButtons() {
  const s = state.consoleState;
  const expandSvg = '<path d="M4 10L10 4M10 4H5M10 4v5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>';
  const restoreSvg = '<path d="M10 4L4 10M4 10h5M4 10V5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>';
  if (s === 'collapsed') {
    consoleExpand.classList.add('hidden');
    consoleToggle.classList.add('hidden');
  } else if (s === 'open') {
    consoleExpand.classList.remove('hidden');
    consoleToggle.classList.remove('hidden');
    consoleExpand.title = 'Expand terminal';
    consoleExpand.querySelector('svg').innerHTML = expandSvg;
    consoleExpand.onclick = () => setConsoleState('expanded');
    consoleToggle.title = 'Close terminal';
    consoleToggle.querySelector('svg').style.transform = '';
    consoleToggle.onclick = () => setConsoleState('collapsed');
  } else {
    consoleExpand.classList.remove('hidden');
    consoleToggle.classList.remove('hidden');
    consoleExpand.title = 'Restore terminal';
    consoleExpand.querySelector('svg').innerHTML = restoreSvg;
    consoleExpand.onclick = () => setConsoleState('open');
    consoleToggle.title = 'Close terminal';
    consoleToggle.querySelector('svg').style.transform = '';
    consoleToggle.onclick = () => setConsoleState('collapsed');
  }
}
function updateRailButtonStates() {
  const tBtn = document.getElementById('console-terminal-toggle');
  if (tBtn) tBtn.classList.toggle('active', state.consoleState !== 'collapsed');
  const cBtn = document.getElementById('console-chat-toggle');
  const overlay = document.getElementById('chat-overlay');
  if (cBtn && overlay) cBtn.classList.toggle('active', overlay.classList.contains('open'));
}

function updateChatRailUnreadBadge() {
  const badge = document.getElementById('chat-rail-badge');
  if (!badge) return;
  let total = 0;
  if (typeof state.unreadCounts !== 'undefined') {
    for (const [chId, uc] of state.unreadCounts) {
      if (chId === state.chatCurrentChannel) continue;
      total += (uc && uc.messages) || 0;
    }
  }
  if (total > 0) {
    badge.textContent = total > 99 ? '99+' : String(total);
    badge.hidden = false;
  } else {
    badge.textContent = '';
    badge.hidden = true;
  }
}
updateConsoleButtons();
updateRailButtonStates();

// Terminal rail: toggle terminal open/closed.
document.getElementById('console-terminal-toggle')?.addEventListener('click', (e) => {
  e.stopPropagation();
  if (state.consoleState === 'collapsed') {
    setConsoleState('open');
    document.getElementById('terminal-input')?.focus();
  } else {
    setConsoleState('collapsed');
  }
});
// Chat rail: toggle chat overlay.
document.getElementById('console-chat-toggle')?.addEventListener('click', (e) => {
  e.stopPropagation();
  toggleChatOverlay();
});

const consoleTerminalPanel = document.querySelector('[data-console-panel="terminal"]');


// ===== Activity timestamps =====
// Last 30 entries show "Ns/Nm/Nh/Nd ago"; older entries show 24h "HH:MM".
function renderConsoleTimes() {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (!body) return;
  const entries = body.querySelectorAll('.console-entry');
  const total = entries.length;
  const recentFloor = Math.max(0, total - CONSOLE_RECENT_COUNT);
  const now = Date.now();
  entries.forEach((entry, idx) => {
    const el = entry.querySelector('.ce-time');
    if (!el) return;
    const ts = parseInt(el.dataset.ts || '0', 10);
    if (!ts) return;
    const when = new Date(ts);
    el.title = when.toLocaleString();
    el.textContent = idx >= recentFloor ? fmtRelativeAgo(now - ts) : fmtClock24(when);
  });
}
setInterval(renderConsoleTimes, 1000);

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


// ===== Chat overlay: combined Model / Effort switcher =====
function openCoMenu() {
  document.getElementById('chat-overlay-ctrl-menu')?.classList.add('open');
}
function closeCoMenus() {
  document.querySelectorAll('.chat-overlay-ctrl-menu.open').forEach(m => m.classList.remove('open'));
}
document.addEventListener('click', (e) => {
  if (!e.target.closest('#chat-overlay-ctrl-pill')) closeCoMenus();
});
function renderCoOptions(container, opts, active, onPick) {
  container.innerHTML = '';
  for (const o of opts) {
    const el = document.createElement('div');
    el.className = 'co-opt' + (o.value === active ? ' active' : '');
    el.textContent = o.label;
    el.addEventListener('click', (e) => {
      e.stopPropagation();
      closeCoMenus();
      onPick(o.value);
    });
    container.appendChild(el);
  }
}
function applyChannelHarnessInfo(channelId) {
  const modelName = document.getElementById('chat-overlay-model-name');
  const effortName = document.getElementById('chat-overlay-effort-name');
  const modelOpts = document.getElementById('chat-overlay-model-opts');
  const effortOpts = document.getElementById('chat-overlay-effort-opts');
  if (!modelName || !effortName) return;
  const ch = (typeof state.chatChannels !== 'undefined' && channelId) ? state.chatChannels.get(channelId) : null;
  if (!ch) {
    modelName.textContent = '—';
    effortName.textContent = '—';
    if (modelOpts) modelOpts.innerHTML = '';
    if (effortOpts) effortOpts.innerHTML = '';
    return;
  }
  const deviceId = state.channelDeviceMap.get(channelId);
  const harnesses = deviceId ? state.deviceHarnesses.get(deviceId) : null;
  const harness = harnesses?.find(h => h.id === ch.harness);
  const currentModel = ch.model || harness?.default_model || '';
  const currentEffort = ch.effort || harness?.default_effort || '';
  modelName.textContent = currentModel || '—';
  effortName.textContent = currentEffort || '—';
  if (modelOpts) {
    const models = (harness?.models || []).map(m => ({ value: m.id, label: m.name || m.id }));
    renderCoOptions(modelOpts, models, currentModel, (v) => {
      modelName.textContent = v;
      if (ch) ch.model = v;
      const conn = getE2EE(channelId);
      if (conn?.connected) {
        conn.updateChannel(channelId, { model: v });
        showToast(`Model set to ${v}`);
      } else {
        showToast('Device not connected');
      }
    });
  }
  if (effortOpts) {
    const efforts = (harness?.effort_levels || []).map(l => ({ value: l, label: l }));
    renderCoOptions(effortOpts, efforts, currentEffort, (v) => {
      effortName.textContent = v;
      if (ch) ch.effort = v;
      const conn = getE2EE(channelId);
      if (conn?.connected) {
        conn.updateChannel(channelId, { effort: v });
        showToast(`Effort set to ${v}`);
      } else {
        showToast('Device not connected');
      }
    });
  }
}
document.getElementById('chat-overlay-ctrl-pill')?.addEventListener('click', (e) => {
  if (e.target.closest('.chat-overlay-ctrl-menu')) return;
  const menu = document.getElementById('chat-overlay-ctrl-menu');
  if (menu?.classList.contains('open')) closeCoMenus();
  else openCoMenu();
});

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


function toggleConsoleEntry(row) {
  const expandIcon = row.querySelector('.ce-expand');
  const detail = row.nextElementSibling;
  if (detail && detail.classList.contains('ce-detail')) {
    if (expandIcon) expandIcon.classList.toggle('open');
    detail.classList.toggle('open');
  }
}

function toggleTree(el) {
  const children = el.nextElementSibling;
  if (children && children.classList.contains('tree-children')) {
    children.classList.toggle('collapsed');
    const arrow = el.querySelector('.tree-arrow');
    if (arrow) arrow.classList.toggle('open');
  }
}

document.getElementById('chat-input')?.addEventListener('input', function(){
  this.style.height = 'auto';
  this.style.height = Math.min(this.scrollHeight, 120) + 'px';
});

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
// Theme version checker — detect deploys via SSE reconnect
// =====================================================================

function showUpdateBadge() {
  const footer = document.querySelector('.channel-panel-footer');
  if (!footer || footer.querySelector('.theme-update-badge')) return;
  const badge = document.createElement('button');
  badge.className = 'theme-update-badge';
  badge.title = 'New version available — click to reload';
  badge.setAttribute('aria-label', 'Update available, click to reload');
  badge.textContent = 'Click to Update';
  badge.addEventListener('click', () => location.reload());
  footer.appendChild(badge);
}

async function checkThemeVersion() {
  try {
    const res = await fetch('/api/theme/version');
    if (!res.ok) return;
    const { version } = await res.json();
    if (!version) return;
    const stored = localStorage.getItem(THEME_VERSION_KEY);
    if (!stored) {
      localStorage.setItem(THEME_VERSION_KEY, version);
      return;
    }
    if (compareVersions(version, stored) > 0) {
      showUpdateBadge();
      localStorage.setItem(THEME_VERSION_KEY, version);
    }
  } catch (err) {
    console.warn('[Theme] version check failed:', err);
  }
}

document.addEventListener('sk:notification-status', (evt) => {
  if (evt.detail.status === 'connected') checkThemeVersion();
});

// =====================================================================
// E2EE Chat — wired to BuildE2EE client
// =====================================================================

// Multi-device E2EE state

function rebuildChatChannels() {
  state.chatChannels.clear();
  for (const [, channels] of state.deviceChannels) {
    for (const [chId, ch] of channels) state.chatChannels.set(chId, ch);
  }
}


function promoteChannel(channelId) {
  const now = Date.now();
  const prev = state.channelSortTs.get(channelId) || 0;
  if (now - prev > SORT_STABILITY_MS) state.channelSortTs.set(channelId, now);
}


// ---- Channel Navigation (Electron only) ----

function getOrderedChannelIds() {
  const ids = [];
  const sortedDevices = [...state.devices.values()].sort((a, b) => a.name.localeCompare(b.name));
  for (const device of sortedDevices) {
    if (!state.e2eeConnections.has(device.id)) continue;
    const devChans = state.deviceChannels.get(device.id) || new Map();
    const sorted = [...devChans.values()].sort((a, b) => {
      const aTs = state.channelSortTs.get(a.id) || 0;
      const bTs = state.channelSortTs.get(b.id) || 0;
      if (aTs !== bTs) return bTs - aTs;
      return (b.created_at || 0) - (a.created_at || 0);
    });
    for (const ch of sorted) ids.push(ch.id);
  }
  return ids;
}

function pushChannelHistory(channelId) {
  if (state._navigatingHistory) return;
  if (state.channelHistory[state.channelHistoryIndex] === channelId) return;
  state.channelHistory.splice(state.channelHistoryIndex + 1);
  state.channelHistory.push(channelId);
  if (state.channelHistory.length > 50) state.channelHistory.shift();
  state.channelHistoryIndex = state.channelHistory.length - 1;
}

if (window.buildElectron) {
  document.addEventListener('keydown', (e) => {
    // Navigation hotkeys all require Alt, which has no useful default
    // behavior in chat inputs (Option+letter inserts special chars; we
    // preventDefault below). So we don't skip when an input is focused —
    // that was making nav unusable while typing.
    if (!e.altKey) return;

    if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      const ids = getOrderedChannelIds();
      if (!ids.length) return;
      const cur = ids.indexOf(state.chatCurrentChannel);

      if (e.shiftKey) {
        // ALT+SHIFT+UP/DOWN: jump to first unread in that direction
        const dir = e.key === 'ArrowUp' ? -1 : 1;
        const start = cur === -1 ? 0 : cur + dir;
        for (let i = start; i >= 0 && i < ids.length; i += dir) {
          const uc = state.unreadCounts.get(ids[i]);
          if (uc && uc.messages > 0) { selectChannel(ids[i]); return; }
        }
      } else {
        // ALT+UP/DOWN: move to adjacent channel
        const next = e.key === 'ArrowUp' ? cur - 1 : cur + 1;
        if (next >= 0 && next < ids.length) selectChannel(ids[next]);
      }
    } else if (e.key === '[' || e.key === ']') {
      e.preventDefault();
      if (e.key === '[' && state.channelHistoryIndex > 0) {
        state.channelHistoryIndex--;
        state._navigatingHistory = true;
        selectChannel(state.channelHistory[state.channelHistoryIndex]);
        state._navigatingHistory = false;
      } else if (e.key === ']' && state.channelHistoryIndex < state.channelHistory.length - 1) {
        state.channelHistoryIndex++;
        state._navigatingHistory = true;
        selectChannel(state.channelHistory[state.channelHistoryIndex]);
        state._navigatingHistory = false;
      }
    }
  });
}


function getLastSeen(channelId) {
  return state.channelLastSeen.get(channelId) || null;
}
function setLastSeen(channelId) {
  const now = new Date().toISOString();
  state.channelLastSeen.set(channelId, now);
  const conn = getE2EE(channelId);
  if (conn && conn.connected) conn.markSeen(channelId);
}

// Deferred read-marking: queue setLastSeen/markRead until user interacts.
let _deferredReadOps = []; // array of callbacks
let _deferredReadListening = false;
function _flushDeferredReads() {
  const ops = _deferredReadOps.splice(0);
  if (ops.length === 0) return; // no-op if nothing queued
  // Capture the lastSeen for highlight persistence before running ops.
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
function deferMarkRead(fn) {
  _deferredReadOps.push(fn);
  if (!_deferredReadListening) {
    _deferredReadListening = true;
    for (const evt of ['keydown', 'click', 'mousemove', 'scroll']) {
      document.addEventListener(evt, _flushDeferredReads, { capture: true, once: false, passive: true });
    }
  }
}
function markUnreadMessages(container) {
  // Use persisted highlight lastSeen if still within the 60s window, then scroll capture, then live.
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
function clearUnreadHighlights() {
  document.querySelectorAll('.msg.unread').forEach(el => el.classList.remove('unread'));
}

function updateStopButton() {
  const btn = document.getElementById('chat-stop-btn');
  if (!btn) return;
  const active = state.chatCurrentChannel && state.channelAgentActive.get(state.chatCurrentChannel);
  btn.classList.toggle('visible', !!active);
}

// cachedHarnesses per-device stored in state.deviceHarnesses Map

// ----- Per-channel localStorage persistence -----

function loadChannelState(channelId) {
  try {
    const data = JSON.parse(localStorage.getItem(CHANNEL_STATE_KEY) || '{}');
    return data[channelId] || {};
  } catch { return {}; }
}

function saveChannelState(channelId, patch) {
  try {
    const data = JSON.parse(localStorage.getItem(CHANNEL_STATE_KEY) || '{}');
    data[channelId] = { ...(data[channelId] || {}), ...patch };
    localStorage.setItem(CHANNEL_STATE_KEY, JSON.stringify(data));
  } catch {}
}

function clearChannelDraft(channelId) {
  try {
    const data = JSON.parse(localStorage.getItem(CHANNEL_STATE_KEY) || '{}');
    if (data[channelId]) {
      delete data[channelId].draft;
      localStorage.setItem(CHANNEL_STATE_KEY, JSON.stringify(data));
    }
  } catch {}
}

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

// renderChannelList and renderChannelSidebar are replaced by renderChannelPanel (defined above)
function renderChannelList() { renderChannelPanel(); }
function renderChannelSidebar() { /* no-op, merged into renderChannelPanel */ }

function incrementUnread(channelId, isInteraction = false) {
  const uc = state.unreadCounts.get(channelId) || { messages: 0, hasInteraction: false };
  uc.messages++;
  if (isInteraction) uc.hasInteraction = true;
  state.unreadCounts.set(channelId, uc);
  renderChannelList();
}

function updateAggregateBadge() { /* no-op, removed with top bar */ }

function selectChannel(channelId) {
  state.chatCurrentChannel = channelId;
  pushChannelHistory(channelId);
  state.activeBrowserTab = null;
  // Hide browser panel and restore normal tab if it was showing
  document.getElementById('tab-browser')?.classList.remove('active');
  // Restore viewer chrome hidden by browser view
  document.querySelector('.viewer-body')?.classList.remove('hidden');
  document.querySelector('.comp-wrapper')?.classList.remove('hidden');
  document.getElementById('console-bottom')?.classList.remove('hidden');
  // Restore files view as the main panel.
  const filesPanel = document.getElementById('tab-files');
  if (filesPanel) filesPanel.classList.add('active');
  // Sync the chat overlay header + model/effort pills for this channel.
  syncChatOverlayHeader();
  applyChannelHarnessInfo(channelId);
  // Close mobile channel panel if open
  document.getElementById('channel-panel')?.classList.remove('mobile-open');
  updateStopButton();
  hideChatBubble();
  hideActivityBubble();
  // Capture lastSeen for scroll positioning before updating it.
  state.scrollLastSeen = getLastSeen(channelId) || null;
  state._unreadHighlightLastSeen = null;
  state._unreadHighlightUntil = null;
  // Defer marking as seen until user interacts.
  const _chId = channelId;
  deferMarkRead(() => {
    setLastSeen(_chId);
    state.unreadCounts.delete(_chId);
    renderChannelList();
  });
  renderChannelList();
  const _selConn = getE2EE(channelId);
  if (_selConn && _selConn.connected) {
    state.chatLoadingMessages.add(channelId);
    state.chatLoadingActivity.add(channelId);
    _selConn.getMessages(channelId);
    _selConn.getActivity(channelId);
    _selConn.getComplications(channelId);
  } else if (!state.chatMessages.has(channelId) || state.chatMessages.get(channelId).length === 0) {
    // Device disconnected, no cached messages — show loading state until reconnect.
    state.chatLoadingMessages.add(channelId);
  }
  renderMessages();
  clearConsole(state.chatLoadingActivity.has(channelId));
  const chName = state.chatChannels.get(channelId)?.name || '';
  const input = document.getElementById('chat-input');
  input.placeholder = `Message #${chName}...`;
  // Update mobile channel label
  updateMobileChannelLabel();
  location.hash = `${state.currentTab || 'chat'}/${channelId}`;
  // If files tab is active, refresh it for the new channel.
  if (state.currentTab === 'files') onFilesTabActivated();
  // Restore persisted state from localStorage.
  const saved = loadChannelState(channelId);
  // Restore last active tab for this channel.
  if (saved.activeTab && saved.activeTab !== state.currentTab) {
    _origSwitchTab(saved.activeTab);
    if (saved.activeTab === 'files') onFilesTabActivated();
    location.hash = `${saved.activeTab}/${channelId}`;
  }
  if (input) {
    input.value = saved.draft || '';
    input.style.height = 'auto';
  }
  // Plan mode: prefer server state, fall back to localStorage.
  if (!state.channelPlanMode.has(channelId) && saved.planMode !== undefined) {
    state.channelPlanMode.set(channelId, saved.planMode);
  }
  // Sync plan mode toggle.
  updatePlanModeUI(state.channelPlanMode.get(channelId) || false);
  // Update terminal and tasks for the new channel. (Activity/Tasks live in the sidebar accordion now.)
  renderTerminalForChannel(channelId);
  renderTasksPanel(channelId);
  updateTasksBadge(channelId);
  // Render complications for this channel (instant — already in memory).
  closeCompPopover();
  renderComplications();
}

function dismissStaleSuggestions(container, msgs) {
  // For each message with suggested_actions, check if a user message follows it.
  // If so, the suggestions are stale — dismiss them (and mark the matching one as selected).
  const suggestionDivs = container.querySelectorAll('.msg-suggestions');
  if (!suggestionDivs.length) return;

  // Build a map of msg index → next user message content (if any).
  const msgsWithSuggestions = [];
  for (let i = 0; i < msgs.length; i++) {
    if (msgs[i].suggested_actions?.length) {
      // Find the next user message after this one.
      let nextUserContent = null;
      for (let j = i + 1; j < msgs.length; j++) {
        if (msgs[j].sender === 'client') {
          nextUserContent = msgs[j].content;
          break;
        }
      }
      msgsWithSuggestions.push({ msgIndex: i, nextUserContent, actions: msgs[i].suggested_actions });
    }
  }

  // Apply states to the DOM suggestion divs (they appear in the same order).
  suggestionDivs.forEach((div, idx) => {
    const info = msgsWithSuggestions[idx];
    if (!info) return;
    if (info.nextUserContent !== null) {
      // A user message followed — dismiss all, mark the matching one as selected.
      div.querySelectorAll('.suggestion-btn').forEach(btn => {
        if (btn.textContent === info.nextUserContent) {
          btn.classList.add('selected');
        } else {
          btn.classList.add('dismissed');
        }
      });
    }
    // If no user message follows, leave buttons active (it's the latest).
  });
}

function renderMessages() {
  const container = document.getElementById('chat-messages');
  const empty = document.getElementById('chat-empty-state');

  if (!state.chatCurrentChannel) {
    container.innerHTML = '';
    container.appendChild(createEmptyState('Select a channel', 'Choose or create a channel to start chatting.'));
    return;
  }

  const msgs = state.chatMessages.get(state.chatCurrentChannel) || [];

  if (msgs.length === 0) {
    container.innerHTML = '';
    const ch = state.chatChannels.get(state.chatCurrentChannel);
    if (state.chatLoadingMessages.has(state.chatCurrentChannel)) {
      container.appendChild(createEmptyState(
        `#${ch?.name || 'channel'}`,
        'Loading messages…',
      ));
    } else {
      container.appendChild(createEmptyState(
        `#${ch?.name || 'channel'}`,
        'No messages yet. Send one to get started.',
      ));
    }
    return;
  }

  container.innerHTML = '';
  for (const msg of msgs) {
    appendMessage(msg);
  }
  // Mark unread messages with warm background.
  markUnreadMessages(container);
  // Dismiss old suggestion buttons and restore selected state from history.
  dismissStaleSuggestions(container, msgs);
  scrollToFirstUnread(container) || scrollChatToBottom();
}

function appendMessage(msg) {
  // Delegate to interaction card if metadata present.
  if (msg.metadata) {
    const meta = typeof msg.metadata === 'string' ? JSON.parse(msg.metadata) : msg.metadata;
    if (meta.interaction_id) {
      appendInteractionCard(msg, meta);
      return;
    }
  }

  const container = document.getElementById('chat-messages');
  // Remove empty state if present.
  const empty = container.querySelector('.empty-state');
  if (empty) empty.remove();

  const isUser = msg.sender === 'client';
  const avatarClass = isUser ? 'user' : 'agent';
  const displayName = isUser ? 'You' : agentShortName(msg.sender);
  const avatarLabel = isUser ? 'Y' : displayName[0];
  const nameLabel = displayName;
  const timeStr = msg.created_at ? shortTime(
    typeof msg.created_at === 'number'
      ? new Date(msg.created_at * 1000).toISOString()
      : msg.created_at
  ) : '';

  const div = document.createElement('div');
  div.className = 'msg';
  div.dataset.msgId = msg.id || '';
  if (msg.created_at) div.dataset.createdAt = typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at;
  div.innerHTML = `
    <div class="msg-avatar ${avatarClass}">${avatarLabel}</div>
    <div class="msg-body">
      <div class="msg-header">
        <span class="msg-name">${nameLabel}</span>
        <span class="msg-time">${timeStr}</span>
      </div>
      <div class="msg-text">${renderMarkdown(msg.content || '')}</div>
      ${msg.attachments && msg.attachments.length ? `<div class="msg-attachments">${
        msg.attachments.map(att =>
          `<div class="msg-attachment">
            <svg viewBox="0 0 16 16" fill="none"><path d="M14 8.5l-5.5 5.5a3.5 3.5 0 01-5-5L9 3.5a2.5 2.5 0 013.5 3.5L7 12.5a1.5 1.5 0 01-2-2L10.5 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
            <span class="att-name">${escapeHtml(att.filename || att.name || 'file')}</span>
            <span class="att-size">${formatFileSize(att.size || 0)}</span>
          </div>`
        ).join('')
      }</div>` : ''}
      ${isUser && msg.id ? `<div class="msg-status ${msg.read_at ? 'read' : msg.delivered_at ? 'delivered' : 'sending'}"><span class="msg-status-state visible"><span class="check ${msg.read_at || msg.delivered_at ? 'active' : ''}">✓</span><span class="check ${msg.read_at ? 'active' : ''}">✓</span> ${msg.read_at ? 'Read' : msg.delivered_at ? 'Delivered' : 'Sending'}</span></div>` : ''}
      ${msg.suggested_actions?.length ? `<div class="msg-suggestions">${
        msg.suggested_actions.map(a => `<button class="suggestion-btn">${escapeHtml(a)}</button>`).join('')
      }</div>` : ''}
    </div>
  `;
  // Wire up file/diff embed toggles.
  div.querySelectorAll('.build-embed-header').forEach(hdr => {
    hdr.style.cursor = 'pointer';
    hdr.addEventListener('click', (e) => {
      if (e.target.closest('.build-embed-wrap-toggle')) return;
      const embed = hdr.closest('.build-embed');
      embed.classList.toggle('collapsed');
    });
  });
  // Wire up suggestion button clicks.
  if (msg.suggested_actions?.length) {
    div.querySelectorAll('.suggestion-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        if (btn.classList.contains('selected') || btn.classList.contains('dismissed')) return;
        // Highlight selected, dismiss siblings.
        div.querySelectorAll('.suggestion-btn').forEach(b => {
          if (b === btn) b.classList.add('selected');
          else b.classList.add('dismissed');
        });
        // Send as user message.
        const input = document.getElementById('chat-input');
        if (input) { input.value = btn.textContent; sendChatMessage(); }
      });
    });
  }
  container.appendChild(div);
}

function appendInteractionCard(msg, meta) {
  const container = document.getElementById('chat-messages');
  const empty = container.querySelector('.empty-state');
  if (empty) empty.remove();

  const resolved = !!meta.resolved_at;
  const interactionId = meta.interaction_id;
  const kind = meta.kind || 'question';
  const options = meta.options || [];
  const allowFreeform = meta.allow_freeform !== false;
  const multiselect = !!meta.multiselect;
  const plan = meta.plan || null;
  const timeStr = msg.created_at ? shortTime(
    typeof msg.created_at === 'number'
      ? new Date(msg.created_at * 1000).toISOString()
      : msg.created_at
  ) : '';

  // Plan review cards get a special light-themed layout.
  if (kind === 'plan_review') {
    appendPlanReviewCard(msg, meta, container, resolved, interactionId, options, plan, timeStr);
    return;
  }

  const div = document.createElement('div');
  div.className = `msg ${resolved ? 'interaction-resolved-msg' : ''}`;
  div.dataset.msgId = msg.id || '';
  if (msg.created_at) div.dataset.createdAt = typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at;

  const senderName = agentShortName(msg.sender);
  let cardHtml = `
    <div class="msg-avatar agent">${senderName[0]}</div>
    <div class="msg-body">
      <div class="msg-header">
        <span class="msg-name">${escapeHtml(senderName)}</span>
        <span class="msg-time">${timeStr}</span>
      </div>
      <div class="interaction-card ${resolved ? 'resolved' : ''}" data-interaction-id="${escapeHtml(interactionId)}">
        <div class="interaction-question">${renderMarkdown(msg.content || '')}</div>`;

  // Plan content (fully expanded).
  if (plan) {
    cardHtml += `
        <div class="interaction-plan-content">${renderMarkdown(plan)}</div>`;
  }

  if (resolved) {
    const selectedOpt = meta.selected_option || '';
    const selectedOpts = meta.selected_options || [];
    // Show options as disabled with selected one(s) highlighted.
    if (options.length) {
      cardHtml += `<div class="interaction-options">`;
      for (const opt of options) {
        const sel = (selectedOpts.length ? selectedOpts.includes(opt.id) : opt.id === selectedOpt) ? ' selected' : '';
        cardHtml += `<button class="interaction-opt${sel}" disabled>${escapeHtml(opt.label || opt.id)}</button>`;
      }
      cardHtml += `</div>`;
    }
    // Show freeform response if one was given.
    if (meta.freeform_response) {
      cardHtml += `<div class="interaction-freeform-response">${escapeHtml(meta.freeform_response)}</div>`;
    }
  } else if (multiselect) {
    // Multiselect: toggleable buttons + submit.
    if (options.length) {
      cardHtml += `<div class="interaction-options multiselect">`;
      for (const opt of options) {
        cardHtml += `<button class="interaction-opt" data-opt-id="${escapeHtml(opt.id)}">${escapeHtml(opt.label || opt.id)}</button>`;
      }
      cardHtml += `</div>`;
    }
    cardHtml += `
      <div class="interaction-freeform">
        ${allowFreeform ? '<textarea placeholder="Type a response..." rows="1"></textarea>' : ''}
        <button class="interaction-submit">Submit</button>
      </div>`;
  } else {
    // Single-select: options as instant-submit buttons.
    if (options.length) {
      cardHtml += `<div class="interaction-options">`;
      for (const opt of options) {
        cardHtml += `<button class="interaction-opt" data-opt-id="${escapeHtml(opt.id)}">${escapeHtml(opt.label || opt.id)}</button>`;
      }
      cardHtml += `</div>`;
    }
    // Freeform input.
    if (allowFreeform) {
      cardHtml += `
        <div class="interaction-freeform">
          <textarea placeholder="Type a response..." rows="1"></textarea>
          <button class="interaction-submit">Send</button>
        </div>`;
    }
  }

  cardHtml += `</div></div>`;
  div.innerHTML = cardHtml;
  container.appendChild(div);

  // Bind event listeners (CSP blocks inline onclick handlers).
  const card = div.querySelector('.interaction-card');
  if (!card || resolved) return;

  if (multiselect) {
    // Multiselect: toggle buttons on click, submit collects all selected.
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.addEventListener('click', () => {
        btn.classList.toggle('selected');
      });
    });
    const submitBtn = card.querySelector('.interaction-submit');
    if (submitBtn) {
      submitBtn.addEventListener('click', () => {
        const selected = [...card.querySelectorAll('.interaction-opt.selected')].map(b => b.dataset.optId);
        const textarea = card.querySelector('.interaction-freeform textarea');
        const freeform = textarea ? textarea.value.trim() : null;
        respondToMultiselectInteraction(interactionId, selected, freeform || null);
      });
    }
  } else {
    // Single-select: option buttons submit immediately.
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.addEventListener('click', () => {
        respondToInteraction(interactionId, btn.dataset.optId, null);
      });
    });
    // Freeform submit.
    const submitBtn = card.querySelector('.interaction-submit');
    if (submitBtn) {
      submitBtn.addEventListener('click', () => {
        respondToInteractionFreeform(submitBtn, interactionId);
      });
    }
  }
}

function appendPlanReviewCard(msg, meta, container, resolved, interactionId, options, plan, timeStr) {
  const channelModel = state.chatChannels.get(state.chatCurrentChannel)?.model || '';
  const displayName = modelToFriendlyName(channelModel);

  const div = document.createElement('div');
  div.className = 'msg';
  div.dataset.msgId = msg.id || '';
  if (msg.created_at) div.dataset.createdAt = typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at;

  // Determine resolved state details.
  const selectedOption = meta.selected_option || '';
  const freeformResponse = meta.freeform_response || '';
  const wasApproved = selectedOption === 'approve';
  const wasDenied = selectedOption === 'reject';
  const gaveFeedback = !!freeformResponse;

  let actionsHtml = '';
  if (resolved) {
    // Show disabled buttons with selected one highlighted.
    const approveClass = `plan-review-btn approve${wasApproved ? ' selected' : ''}`;
    const denyClass = `plan-review-btn deny${wasDenied ? ' selected' : ''}`;
    actionsHtml = `
      <div class="plan-review-actions">
        <button class="${approveClass}" disabled>Approve</button>
        <button class="${denyClass}" disabled>Deny</button>
        ${gaveFeedback ? `<span class="plan-review-note">You provided further instructions</span>` : ''}
      </div>`;
  } else {
    actionsHtml = `
      <div class="plan-review-actions">
        <button class="plan-review-btn approve" data-opt-id="approve">Approve</button>
        <button class="plan-review-btn deny" data-opt-id="reject">Deny</button>
        <span class="plan-review-note">or send a message to provide feedback</span>
      </div>`;
  }

  const senderName = agentShortName(msg.sender) || displayName;
  div.innerHTML = `
    <div class="plan-review-card" data-interaction-id="${escapeHtml(interactionId)}">
      <div class="plan-review-title">
        <span class="plan-review-avatar">${senderName[0]}</span>
        ${escapeHtml(senderName)}'s Plan
        <span class="plan-review-time">${timeStr}</span>
      </div>
      <div class="plan-review-body">
        ${plan ? `<div class="plan-review-content">${renderMarkdown(plan)}</div>` : ''}
        ${actionsHtml}
      </div>
    </div>`;

  container.appendChild(div);

  if (resolved) return;

  // Bind approve/deny buttons.
  const card = div.querySelector('.plan-review-card');
  card.querySelectorAll('.plan-review-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      const optId = btn.dataset.optId;
      respondToInteraction(interactionId, optId, null);
      // Update buttons to disabled state with selection highlighted.
      card.querySelectorAll('.plan-review-btn').forEach(b => {
        b.disabled = true;
        if (b.dataset.optId === optId) b.classList.add('selected');
      });
      // Remove the note.
      const note = card.querySelector('.plan-review-note');
      if (note) note.remove();
    });
  });
}

function resolvePendingPlanReviews() {
  // Find all unresolved plan review cards and mark them as "further instructions".
  document.querySelectorAll('.plan-review-card').forEach(card => {
    const btns = card.querySelectorAll('.plan-review-btn:not(:disabled)');
    if (!btns.length) return; // Already resolved.
    btns.forEach(b => { b.disabled = true; });
    const note = card.querySelector('.plan-review-note');
    if (note) {
      note.textContent = 'You provided further instructions';
    }
  });
  // Also resolve regular interaction cards.
  document.querySelectorAll('.interaction-card:not(.resolved)').forEach(card => {
    card.classList.add('resolved');
    const opts = card.querySelector('.interaction-options');
    const ff = card.querySelector('.interaction-freeform');
    if (opts) opts.remove();
    if (ff) ff.remove();
  });
}

function respondToInteraction(interactionId, selectedOption, freeformResponse) {
  const _conn = getActiveE2EE();
  if (!_conn || !_conn.connected || !state.chatCurrentChannel) return;
  _conn.sendInteractionResponse(state.chatCurrentChannel, interactionId, selectedOption, freeformResponse);
  // Mark card as resolved in UI.
  const card = document.querySelector(`[data-interaction-id="${interactionId}"]`);
  if (card) {
    card.classList.add('resolved');
    // Disable all option buttons and highlight the selected one.
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.disabled = true;
      if (btn.dataset.optId === selectedOption) btn.classList.add('selected');
    });
    // Hide freeform input.
    const ff = card.querySelector('.interaction-freeform');
    if (ff) ff.remove();
  }
  // Clear interaction flag for this channel.
  const uc = state.unreadCounts.get(state.chatCurrentChannel);
  if (uc) {
    uc.hasInteraction = false;
    renderChannelList();
  }
}

function crossfadeStatus(statusEl, newState, newContent) {
  // Remove old state classes and set new one.
  statusEl.classList.remove('sending', 'delivered', 'read', 'failed');
  statusEl.classList.add(newState);
  // Exit every existing state span so stacked states don't pile up.
  const oldSpans = statusEl.querySelectorAll('.msg-status-state');
  const newSpan = document.createElement('span');
  newSpan.className = 'msg-status-state enter';
  newSpan.innerHTML = newContent;
  statusEl.appendChild(newSpan);
  oldSpans.forEach(oldSpan => {
    oldSpan.classList.remove('visible', 'enter');
    oldSpan.classList.add('exit');
    setTimeout(() => oldSpan.remove(), 300);
  });
  // Trigger reflow then make new span visible.
  requestAnimationFrame(() => {
    newSpan.classList.remove('enter');
    newSpan.classList.add('visible');
  });
}

function respondToInteractionFreeform(btn, interactionId) {
  const textarea = btn.parentElement.querySelector('textarea');
  const text = (textarea ? textarea.value : '').trim();
  if (!text) return;
  respondToInteraction(interactionId, null, text);
}

function respondToMultiselectInteraction(interactionId, selectedOptions, freeformResponse) {
  const _conn = getActiveE2EE();
  if (!_conn || !_conn.connected || !state.chatCurrentChannel) return;
  _conn.sendInteractionResponse(state.chatCurrentChannel, interactionId, null, freeformResponse, selectedOptions);
  // Mark card as resolved in UI.
  const card = document.querySelector(`[data-interaction-id="${interactionId}"]`);
  if (card) {
    card.classList.add('resolved');
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.disabled = true;
      if (!selectedOptions.includes(btn.dataset.optId)) btn.classList.remove('selected');
    });
    const ff = card.querySelector('.interaction-freeform');
    if (ff) ff.remove();
  }
  const uc = state.unreadCounts.get(state.chatCurrentChannel);
  if (uc) {
    uc.hasInteraction = false;
    renderChannelList();
  }
}

function updatePlanModeUI(active) {
  const btn = document.getElementById('cmd-plan-btn');
  if (!btn) return;
  btn.classList.toggle('active', !!active);
}

function appendSystemMessage(text) {
  const container = document.getElementById('chat-messages');
  const div = document.createElement('div');
  div.className = 'system-msg';
  div.textContent = text;
  container.appendChild(div);
}


let currentReasoningEntry = null;

function clearConsole(loading) {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (body) {
    if (loading) {
      body.innerHTML = '<div class="empty-state"><p>Loading tool uses…</p></div>';
    } else {
      body.innerHTML = '';
    }
  }
  currentReasoningEntry = null;
}

function appendConsoleReasoning(content, timestamp) {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (!body) return;

  // Buffer into existing reasoning entry if one is active.
  if (currentReasoningEntry) {
    currentReasoningEntry._reasoningText += content;
    const desc = currentReasoningEntry.querySelector('.ce-desc');
    if (desc) {
      const firstLine = currentReasoningEntry._reasoningText.split('\n').find(l => l.trim()) || '';
      desc.textContent = firstLine;
    }
    const detail = currentReasoningEntry.querySelector('.ce-detail-reasoning');
    if (detail) detail.innerHTML = renderMarkdown(currentReasoningEntry._reasoningText);
    body.scrollTop = body.scrollHeight;
    return;
  }

  const ts = timestamp ? new Date(timestamp) : new Date();
  const timeStr = ts.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const firstLine = content.split('\n').find(l => l.trim()) || content.slice(0, 100);

  const entry = document.createElement('div');
  entry.className = 'console-entry';
  entry._reasoningText = content;

  entry.innerHTML = `
    <div class="ce-row">
      <span class="ce-tag reasoning">Reasoning</span>
      <span class="ce-desc">${escapeHtml(firstLine)}</span>
      <span class="ce-time" data-ts="${ts.getTime()}"></span>
    </div>
    <div class="ce-detail">
      <div class="ce-detail-reasoning">${renderMarkdown(content)}</div>
    </div>
  `;

  const row = entry.querySelector('.ce-row');
  const detail = entry.querySelector('.ce-detail');
  row.addEventListener('click', () => {
    detail.classList.toggle('open');
    row.classList.toggle('open', detail.classList.contains('open'));
    if (detail.classList.contains('open')) {
      entry.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }
  });

  const wasNearBottom = isConsoleNearBottom();
  body.appendChild(entry);
  renderConsoleTimes();
  if (wasNearBottom) body.scrollTop = body.scrollHeight;
  else showActivityBubble(entry);
  currentReasoningEntry = entry;
}

function appendConsoleEntry(toolId, name, desc, input, timestamp) {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (!body) return;

  const ts = timestamp ? new Date(timestamp) : new Date();
  const timeStr = ts.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const tag = toolTag(name);

  const entry = document.createElement('div');
  entry.className = 'console-entry';
  if (toolId) entry.dataset.toolId = toolId;

  entry.innerHTML = `
    <div class="ce-row">
      <span class="ce-tag ${tag}">${escapeHtml(name)}</span>
      <span class="ce-desc">${escapeHtml(desc)}</span>
      <span class="ce-summary"></span>
      <span class="ce-time" data-ts="${ts.getTime()}"></span>
    </div>
    <div class="ce-detail">
      <div class="ce-detail-input">${formatToolDetail(name, input)}</div>
      <div class="ce-detail-result" id="ce-result-${toolId || ''}"></div>
    </div>
  `;
  entry._toolName = name;

  // Toggle detail on click.
  const row = entry.querySelector('.ce-row');
  const detail = entry.querySelector('.ce-detail');
  row.addEventListener('click', () => {
    detail.classList.toggle('open');
    row.classList.toggle('open', detail.classList.contains('open'));
    if (detail.classList.contains('open')) {
      entry.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }
  });

  const wasNearBottom = isConsoleNearBottom();
  body.appendChild(entry);
  renderConsoleTimes();
  if (wasNearBottom) body.scrollTop = body.scrollHeight;
  else showActivityBubble(entry);
}

function markConsoleEntryDone(toolId, isError, content, completedAt) {
  if (!toolId) return;
  const entry = document.querySelector(`.console-entry[data-tool-id="${toolId}"]`);
  if (!entry) return;
  const summary = entry.querySelector('.ce-summary');
  if (summary) {
    if (isError) {
      summary.innerHTML = '<span class="fail" title="error" aria-label="error">✗</span>';
    } else {
      summary.innerHTML = '<span class="success" title="done" aria-label="done">✓</span>';
    }
  }
  // Update displayed time to completion time.
  if (completedAt) {
    const timeEl = entry.querySelector('.ce-time');
    if (timeEl) {
      const ts = new Date(completedAt);
      timeEl.dataset.ts = String(ts.getTime());
      renderConsoleTimes();
    }
  }
  // Render result content in the expandable detail section.
  if (content) {
    const resultEl = entry.querySelector('.ce-detail-result');
    if (resultEl) {
      resultEl.innerHTML = formatToolResult(entry._toolName || '', content, isError);
    }
  }
}


function isChatNearBottom(threshold = 80) {
  const c = document.getElementById('chat-messages');
  return c.scrollHeight - c.scrollTop - c.clientHeight < threshold;
}

function isConsoleNearBottom(threshold = 80) {
  const c = document.querySelector('[data-console-panel="activity"]');
  if (!c) return true;
  return c.scrollHeight - c.scrollTop - c.clientHeight < threshold;
}

function scrollChatToBottom() {
  const container = document.getElementById('chat-messages');
  container.scrollTop = container.scrollHeight;
  hideChatBubble();
}

// ---- Chat scroll engine ----

/**
 * Scroll so a message element is visible in the chat container.
 * align='auto': short messages → bottom-align (max context above), tall → top-align.
 * align='top': always top-align (for unread targets).
 */
function scrollToMessage(el, behavior = 'instant', align = 'auto') {
  const container = document.getElementById('chat-messages');
  const cRect = container.getBoundingClientRect();
  const eRect = el.getBoundingClientRect();
  const elTop = eRect.top - cRect.top + container.scrollTop;
  const elH = eRect.height;
  const vpH = container.clientHeight;
  let target;
  if (align === 'top' || (align === 'auto' && elH >= vpH)) {
    target = elTop;
  } else {
    target = elTop + elH - vpH;
  }
  target = Math.max(0, Math.min(target, container.scrollHeight - vpH));
  if (behavior === 'smooth') {
    container.scrollTo({ top: target, behavior: 'smooth' });
  } else {
    container.scrollTop = target;
  }
}

let _oldestAutoScrollTarget = null;
let _scrollRAF = null;

/**
 * Called after appending a new incoming message.
 * wasNearBottom: result of isChatNearBottom() captured BEFORE the append.
 * newEl: the newly appended DOM element.
 */
function handleNewMessageScroll(wasNearBottom, newEl) {
  if (!wasNearBottom) {
    showChatBubble(newEl);
    return;
  }
  // Track the oldest unread message that arrived while user was at bottom.
  if (!_oldestAutoScrollTarget) _oldestAutoScrollTarget = newEl;

  // Debounce: if multiple messages arrive in one frame, only scroll once.
  if (_scrollRAF) cancelAnimationFrame(_scrollRAF);
  _scrollRAF = requestAnimationFrame(() => {
    _scrollRAF = null;
    const container = document.getElementById('chat-messages');
    const cRect = container.getBoundingClientRect();

    // Check if the oldest unread target has scrolled above the viewport.
    if (_oldestAutoScrollTarget && _oldestAutoScrollTarget !== newEl) {
      const oldRect = _oldestAutoScrollTarget.getBoundingClientRect();
      if (oldRect.top < cRect.top) {
        scrollToMessage(_oldestAutoScrollTarget, 'instant', 'top');
        return;
      }
    }
    // Otherwise scroll to bottom — new messages are always last, and this
    // shows the container's padding-bottom as breathing room below the message.
    scrollChatToBottom();
  });
}

/** For user's own sent messages — always scroll to show it. */
function handleSentMessageScroll(el) {
  _oldestAutoScrollTarget = null;
  if (_scrollRAF) cancelAnimationFrame(_scrollRAF);
  scrollChatToBottom();
}

// ---- New content bubbles ----

const newChatBubble = document.getElementById('new-chat-bubble');
const newActivityBubble = document.getElementById('new-activity-bubble');
let firstUnseenChatEl = null;
let firstUnseenActivityEl = null;

function showChatBubble(el) {
  if (!firstUnseenChatEl) firstUnseenChatEl = el;
  newChatBubble.classList.add('visible');
  document.getElementById('chat-scroll-arrow')?.classList.remove('visible');
}
function hideChatBubble() {
  newChatBubble.classList.remove('visible');
  firstUnseenChatEl = null;
  _oldestAutoScrollTarget = null;
}
function showActivityBubble(el) {
  if (!firstUnseenActivityEl) firstUnseenActivityEl = el;
  newActivityBubble?.classList.add('visible');
  document.getElementById('activity-scroll-arrow')?.classList.remove('visible');
}
function hideActivityBubble() {
  newActivityBubble?.classList.remove('visible');
  firstUnseenActivityEl = null;
}

newChatBubble?.addEventListener('click', () => {
  if (firstUnseenChatEl) {
    scrollToMessage(firstUnseenChatEl, 'smooth', 'top');
  } else {
    scrollChatToBottom();
  }
  hideChatBubble();
});

newActivityBubble?.addEventListener('click', () => {
  if (firstUnseenActivityEl) {
    firstUnseenActivityEl.scrollIntoView({ block: 'start', behavior: 'smooth' });
  } else {
    const body = document.querySelector('[data-console-panel="activity"]');
    if (body) body.scrollTop = body.scrollHeight;
  }
  hideActivityBubble();
});

// Scroll-to-bottom arrows (shown when scrolled up and no new-content bubble visible).
const chatScrollArrow = document.getElementById('chat-scroll-arrow');
const activityScrollArrow = document.getElementById('activity-scroll-arrow');

function updateChatScrollArrow() {
  const nearBottom = isChatNearBottom();
  if (nearBottom) { hideChatBubble(); chatScrollArrow.classList.remove('visible'); return; }
  const bubbleVisible = newChatBubble.classList.contains('visible');
  chatScrollArrow.classList.toggle('visible', !bubbleVisible);
}
function updateActivityScrollArrow() {
  if (!activityScrollArrow) return;
  const nearBottom = isConsoleNearBottom();
  if (nearBottom) { hideActivityBubble(); activityScrollArrow.classList.remove('visible'); return; }
  const bubbleVisible = newActivityBubble?.classList.contains('visible') || false;
  activityScrollArrow.classList.toggle('visible', !bubbleVisible);
}

// Code block wrap toggle (event delegation)
document.addEventListener('click', (e) => {
  const wrapBtn = e.target.closest('.md-code-wrap-toggle');
  if (wrapBtn) {
    const block = wrapBtn.closest('.md-code-block');
    if (block) block.classList.toggle('wrap-on');
    return;
  }
  const copyBtn = e.target.closest('.md-code-copy');
  if (copyBtn) {
    const block = copyBtn.closest('.md-code-block');
    if (block) {
      const code = block.querySelector('code');
      if (code) {
        navigator.clipboard.writeText(code.textContent).then(() => {
          copyBtn.textContent = 'copied!';
          setTimeout(() => { copyBtn.innerHTML = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="9" height="9" rx="1.5"/><path d="M5 11H3.5A1.5 1.5 0 012 9.5v-7A1.5 1.5 0 013.5 1h7A1.5 1.5 0 0112 2.5V5"/></svg>copy'; }, 2000);
        });
      }
    }
    return;
  }
  const embedWrapBtn = e.target.closest('.build-embed-wrap-toggle');
  if (embedWrapBtn) {
    e.stopPropagation(); // prevent toggle collapse
    const embed = embedWrapBtn.closest('.build-embed');
    if (embed) embed.classList.toggle('wrap-on');
    return;
  }
});

document.getElementById('chat-messages').addEventListener('scroll', updateChatScrollArrow);
document.querySelector('[data-console-panel="activity"]')?.addEventListener('scroll', updateActivityScrollArrow);

chatScrollArrow?.addEventListener('click', () => { scrollChatToBottom(); chatScrollArrow.classList.remove('visible'); });
activityScrollArrow?.addEventListener('click', () => {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (body) body.scrollTop = body.scrollHeight;
  activityScrollArrow.classList.remove('visible');
});

function scrollToFirstUnread(container) {
  if (!state.chatCurrentChannel) return false;
  // Use captured lastSeen from channel switch, falling back to live value.
  const lastSeen = state.scrollLastSeen || getLastSeen(state.chatCurrentChannel);
  if (!lastSeen) return false;
  const msgEls = container.querySelectorAll('.msg[data-created-at]');
  for (const el of msgEls) {
    if (el.dataset.createdAt > lastSeen) {
      scrollToMessage(el, 'instant', 'top');
      state.scrollLastSeen = null; // consumed
      return true;
    }
  }
  return false;
}

function createEmptyState(title, desc) {
  const div = document.createElement('div');
  div.className = 'empty-state';
  div.innerHTML = `
    <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z"/>
    </svg>
    <h3>${title}</h3>
    <p>${desc}</p>
  `;
  return div;
}

// ----- Send message -----

async function sendChatMessage() {
  const input = document.getElementById('chat-input');
  const content = input.value.trim();
  const hasFiles = state.pendingFiles.length > 0;
  const _sendConn = getActiveE2EE();
  if ((!content && !hasFiles) || !state.chatCurrentChannel || !_sendConn || !_sendConn.connected) return;

  // Capture channel at send time so async uploads don't target the wrong channel.
  const channelId = state.chatCurrentChannel;

  input.value = '';
  input.style.height = 'auto';
  clearChannelDraft(channelId);

  // Upload any staged files first.
  let attachments = null;
  if (hasFiles) {
    const files = [...state.pendingFiles];
    clearPendingFiles();
    attachments = [];
    for (const file of files) {
      try {
        const result = await _sendConn.uploadFile(channelId, file);
        attachments.push({
          file_id: result.file_id,
          filename: result.filename,
          size: result.size,
          mime_type: result.mime_type,
        });
      } catch (err) {
        console.error('[Chat] File upload failed:', err);
      }
    }
    if (!attachments.length) attachments = null;
  }

  const messageContent = content || (attachments ? `Sent ${attachments.length} file(s)` : '');
  if (!messageContent) return;

  // Dismiss all pending suggested action buttons.
  document.querySelectorAll('.msg-suggestions .suggestion-btn:not(.selected):not(.dismissed)').forEach(b => b.classList.add('dismissed'));


  // Optimistically render the message.
  const tempMsg = {
    id: crypto.randomUUID(),
    channel_id: channelId,
    sender: 'client',
    content: messageContent,
    created_at: Date.now() / 1000,
    attachments,
  };
  const msgs = state.chatMessages.get(channelId) || [];
  msgs.push(tempMsg);
  state.chatMessages.set(channelId, msgs);
  appendMessage(tempMsg);
  const _sentEl = document.getElementById('chat-messages').lastElementChild;
  if (_sentEl) handleSentMessageScroll(_sentEl);

  // Auto-resolve any pending plan review cards (agent cancels interactions on new messages).
  resolvePendingPlanReviews();

  try {
    // Send via E2EE with attachment metadata, plan mode, model, and effort.
    const payload = { action: 'message', channel_id: channelId, content: messageContent };
    if (attachments) payload.attachments = attachments;
    if (state.channelPlanMode.get(channelId)) payload.plan_mode = true;
    const _chData = state.chatChannels.get(channelId);
    if (_chData?.model) payload.model = _chData.model;
    if (_chData?.effort) payload.effort = _chData.effort;
    const realMessageId = await _sendConn.send(payload);
    const el = document.querySelector(`[data-msg-id="${tempMsg.id}"]`);
    if (el) el.dataset.msgId = realMessageId;
    tempMsg.id = realMessageId;
  } catch (err) {
    console.error('[Chat] Send failed:', err);
  }
}

document.getElementById('chat-send-btn')?.addEventListener('click', sendChatMessage);
document.getElementById('chat-stop-btn')?.addEventListener('click', () => {
  const _stopConn = getActiveE2EE();
  if (!state.chatCurrentChannel || !_stopConn?.connected) return;

  // Device handles two-phase stop: graceful cancel → 3s → process kill.
  _stopConn.stopAgent(state.chatCurrentChannel).catch(() => {});
  state.channelAgentActive.set(state.chatCurrentChannel, false);
  updateStopButton();
});
const _isMobile = ('ontouchstart' in window || navigator.maxTouchPoints > 0);
document.getElementById('chat-input')?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !_isMobile) {
    e.preventDefault();
    sendChatMessage();
  }
});

// Save draft input to localStorage on every change.
document.getElementById('chat-input')?.addEventListener('input', () => {
  if (!state.chatCurrentChannel) return;
  saveChannelState(state.chatCurrentChannel, { draft: document.getElementById('chat-input').value });
});

// ----- Commands tray toggle -----
// Show/hide commands tray based on input focus. Uses a short delay on blur
// so that clicks/taps on tray buttons have time to fire before it hides.
const cmdTray = document.getElementById('commands-tray');
const chatInputArea = document.querySelector('.chat-input-area');
let trayHideTimeout = null;
chatInputArea?.addEventListener('focusin', () => {
  clearTimeout(trayHideTimeout);
  cmdTray?.classList.add('focus-visible');
});
chatInputArea?.addEventListener('focusout', () => {
  trayHideTimeout = setTimeout(() => cmdTray?.classList.remove('focus-visible'), 200);
});
// Refocus the input after any tray button click so the tray stays visible.
cmdTray?.addEventListener('click', () => {
  document.getElementById('chat-input')?.focus();
});
// Attach button (in tray)
document.getElementById('cmd-attach-btn')?.addEventListener('click', () => {
  document.getElementById('chat-file-input').click();
});

// Plan button (in tray)
document.getElementById('cmd-plan-btn')?.addEventListener('click', () => {
  if (!state.chatCurrentChannel) return;
  const current = state.channelPlanMode.get(state.chatCurrentChannel) || false;
  state.channelPlanMode.set(state.chatCurrentChannel, !current);
  saveChannelState(state.chatCurrentChannel, { planMode: !current });
  updatePlanModeUI(!current);
});

// Compact button (in tray) - double-click confirm pattern
let compactConfirmTimeout = null;
document.getElementById('cmd-compact-btn')?.addEventListener('click', () => {
  const btn = document.getElementById('cmd-compact-btn');
  if (!state.chatCurrentChannel) return;
  if (btn.classList.contains('confirm')) {
    clearTimeout(compactConfirmTimeout);
    btn.classList.remove('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Compact';
    getActiveE2EE()?.compactSession(state.chatCurrentChannel);
  } else {
    btn.classList.add('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Click to confirm';
    compactConfirmTimeout = setTimeout(() => {
      btn.classList.remove('confirm');
      const label = btn.querySelector('span');
      if (label) label.textContent = 'Compact';
    }, 3000);
  }
});

// Clear button (in tray) - double-click confirm pattern
let resetConfirmTimeout = null;
document.getElementById('cmd-reset-btn')?.addEventListener('click', () => {
  const btn = document.getElementById('cmd-reset-btn');
  if (!state.chatCurrentChannel) return;
  if (btn.classList.contains('confirm')) {
    // Second click - do the reset
    clearTimeout(resetConfirmTimeout);
    btn.classList.remove('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Clear';
    getActiveE2EE()?.resetSession(state.chatCurrentChannel);
  } else {
    // First click - enter confirm state
    btn.classList.add('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Click to confirm';
    resetConfirmTimeout = setTimeout(() => {
      btn.classList.remove('confirm');
      const label = btn.querySelector('span');
      if (label) label.textContent = 'Clear';
    }, 3000);
  }
});

// ----- File Upload UI -----


function addPendingFiles(files) {
  for (const file of files) {
    if (file.size > MAX_FILE_SIZE) {
      console.warn(`File too large: ${file.name} (${formatFileSize(file.size)})`);
      continue;
    }
    state.pendingFiles.push(file);
  }
  renderPendingFiles();
}

function removePendingFile(index) {
  state.pendingFiles.splice(index, 1);
  renderPendingFiles();
}

function clearPendingFiles() {
  state.pendingFiles.length = 0;
  renderPendingFiles();
}

function renderPendingFiles() {
  const staging = document.getElementById('upload-staging');
  if (!staging) return;
  if (!state.pendingFiles.length) {
    staging.innerHTML = '';
    staging.classList.remove('has-files');
    return;
  }
  staging.classList.add('has-files');
  staging.innerHTML = state.pendingFiles.map((f, i) =>
    `<div class="upload-pill">
      <span class="up-name">${escapeHtml(f.name)}</span>
      <span class="up-size">${formatFileSize(f.size)}</span>
      <span class="up-remove" data-index="${i}">&times;</span>
    </div>`
  ).join('');
  staging.querySelectorAll('.up-remove').forEach(btn => {
    btn.addEventListener('click', () => removePendingFile(parseInt(btn.dataset.index)));
  });
}

// Attach button opens file picker (handled by cmd-attach-btn in commands tray).

document.getElementById('chat-file-input')?.addEventListener('change', (e) => {
  if (e.target.files.length) {
    addPendingFiles(e.target.files);
    e.target.value = ''; // Reset so same file can be re-selected.
  }
});

// Upload progress: bound per-instance in bindE2EEEvents 'connected' handler.

function _bindUploadProgress(client) {
  client.addEventListener('upload_progress', (evt) => {
    const { filename, progress, total_chunks, chunks_done } = evt.detail;
    const bar = document.getElementById('upload-progress');
    const fill = document.getElementById('upload-progress-fill');
    const label = document.getElementById('upload-progress-label');
    if (!bar) return;
    bar.classList.add('active');
    fill.style.width = (progress * 100) + '%';
    label.textContent = `Uploading ${filename}… ${chunks_done || 0}/${total_chunks}`;
    if (progress >= 1) {
      setTimeout(() => { bar.classList.remove('active'); }, 1500);
    }
  });
}

// Drag and drop on chat area.
const chatPanel = document.querySelector('.chat-main');
if (chatPanel) {
  let dragCounter = 0;
  const overlay = document.getElementById('chat-drop-overlay');

  chatPanel.addEventListener('dragenter', (e) => {
    e.preventDefault();
    dragCounter++;
    if (overlay) overlay.classList.add('visible');
  });

  chatPanel.addEventListener('dragleave', (e) => {
    e.preventDefault();
    dragCounter--;
    if (dragCounter <= 0) {
      dragCounter = 0;
      if (overlay) overlay.classList.remove('visible');
    }
  });

  chatPanel.addEventListener('dragover', (e) => {
    e.preventDefault();
  });

  chatPanel.addEventListener('drop', (e) => {
    e.preventDefault();
    dragCounter = 0;
    if (overlay) overlay.classList.remove('visible');
    if (e.dataTransfer?.files?.length) {
      addPendingFiles(e.dataTransfer.files);
    }
  });
}

// Paste files/images into chat area.
if (chatPanel) {
  chatPanel.addEventListener('paste', (e) => {
    const files = e.clipboardData?.files;
    if (files && files.length) {
      e.preventDefault();
      addPendingFiles(files);
    }
  });
}

// ----- New channel dialog -----

document.getElementById('btn-new-channel')?.addEventListener('click', () => {
  if (!anyE2EEConnected()) {
    alert('E2EE not connected. Waiting for device...');
    return;
  }

  // Determine which device to create the channel on.
  const _targetDeviceId = state.channelDeviceMap.get(state.chatCurrentChannel) || state.e2eeConnections.keys().next().value;
  const _createConn = state.e2eeConnections.get(_targetDeviceId);

  const dialogParent = document.body;
  const overlay = document.createElement('div');
  overlay.className = 'new-channel-overlay';

  // Build harness options from device's cache.
  const cachedHarnesses = state.deviceHarnesses.get(_targetDeviceId) || [];
  let harnessOptions = '<option value="">No agent</option>';
  if (cachedHarnesses.length) {
    harnessOptions = cachedHarnesses.map(h =>
      `<option value="${h.id}">${h.name}</option>`
    ).join('');
  }

  overlay.innerHTML = `
    <div class="new-channel-dialog">
      <h3>New Channel</h3>
      <input type="text" id="new-channel-name" placeholder="Channel name..." autofocus>
      <label for="new-channel-harness">Harness</label>
      <select id="new-channel-harness">${harnessOptions}</select>
      <label for="new-channel-model">Model</label>
      <select id="new-channel-model"><option value="">Select a harness first</option></select>
      <label for="new-channel-effort">Effort</label>
      <select id="new-channel-effort"><option value="">Default</option></select>
      <button class="new-channel-advanced-toggle" type="button" id="new-channel-advanced-toggle">▶ Advanced</button>
      <div class="new-channel-advanced" id="new-channel-advanced">
        <label for="new-channel-workdir">Working Directory</label>
        <input type="text" id="new-channel-workdir" placeholder="/path/to/project">
        <label for="new-channel-prompt">System Prompt</label>
        <textarea id="new-channel-prompt" placeholder="Optional agent instructions..." rows="2"></textarea>
        <label class="new-channel-checkbox">
          <input type="checkbox" id="new-channel-auto-approve">
          Auto-approve all tool uses
        </label>
      </div>
      <div class="dialog-btns">
        <button class="btn btn-cancel" id="new-channel-cancel">Cancel</button>
        <button class="btn btn-create" id="new-channel-create">Create</button>
      </div>
    </div>
  `;
  dialogParent.appendChild(overlay);

  const nameInput = document.getElementById('new-channel-name');
  const harnessSelect = document.getElementById('new-channel-harness');
  const modelSelect = document.getElementById('new-channel-model');
  const advancedToggle = document.getElementById('new-channel-advanced-toggle');
  const advancedSection = document.getElementById('new-channel-advanced');
  nameInput.focus();

  const effortSelect = document.getElementById('new-channel-effort');

  // Populate models and effort options when harness changes.
  function updateHarnessFields() {
    const harnessId = harnessSelect.value;
    modelSelect.innerHTML = '';
    effortSelect.innerHTML = '<option value="">Default</option>';
    if (!harnessId || !cachedHarnesses) {
      modelSelect.innerHTML = '<option value="">Select a harness first</option>';
      return;
    }
    const harness = cachedHarnesses.find(h => h.id === harnessId);
    if (!harness) return;
    for (const m of harness.models) {
      const opt = document.createElement('option');
      opt.value = m.id;
      opt.textContent = m.name;
      if (m.id === harness.default_model) opt.selected = true;
      modelSelect.appendChild(opt);
    }
    for (const lvl of (harness.effort_levels || [])) {
      const opt = document.createElement('option');
      opt.value = lvl;
      opt.textContent = effortLabel(lvl);
      if (lvl === harness.default_effort) opt.selected = true;
      effortSelect.appendChild(opt);
    }
  }
  harnessSelect.addEventListener('change', updateHarnessFields);
  // Init for first harness.
  updateHarnessFields();

  // Advanced toggle.
  advancedToggle.addEventListener('click', () => {
    advancedSection.classList.toggle('is-open');
    advancedToggle.textContent = advancedSection.classList.contains('is-open') ? '▼ Advanced' : '▶ Advanced';
  });

  const close = () => overlay.remove();
  document.getElementById('new-channel-cancel').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });

  const create = () => {
    const name = nameInput.value.trim();
    if (name) {
      const opts = {};
      if (harnessSelect.value) opts.harness = harnessSelect.value;
      if (modelSelect.value) opts.model = modelSelect.value;
      const effortVal = document.getElementById('new-channel-effort').value;
      if (effortVal) opts.effort = effortVal;
      const wd = document.getElementById('new-channel-workdir').value.trim();
      if (wd) opts.working_directory = wd;
      const sp = document.getElementById('new-channel-prompt').value.trim();
      if (sp) opts.system_prompt = sp;
      opts.auto_approve_tools = document.getElementById('new-channel-auto-approve').checked;
      _createConn?.createChannel(name, opts);
      close();
    }
  };
  document.getElementById('new-channel-create').addEventListener('click', create);
  nameInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') create();
    if (e.key === 'Escape') close();
  });
});

// syncE2EEStatus is now defined above with initE2EE.
// MutationObservers no longer needed — syncE2EEStatus is called directly.

// ===== Edit Channel Dialog =====
function showEditChannelDialog(ch) {
  const _editConn = getE2EE(ch.id);
  if (!_editConn || !_editConn.connected) return;
  const dialogParent = document.body;
  if (!dialogParent) return;

  // Build model options from the channel's harness.
  const deviceId = state.channelDeviceMap.get(ch.id);
  const cachedHarnesses = deviceId ? state.deviceHarnesses.get(deviceId) : [];
  const harness = cachedHarnesses?.find(h => h.id === ch.harness);
  let modelOptions = '';
  if (harness) {
    modelOptions = harness.models.map(m =>
      `<option value="${m.id}"${m.id === ch.model ? ' selected' : ''}>${m.name}</option>`
    ).join('');
  }

  // Effort options come from the harness definition.
  const harnessEffortLevels = harness?.effort_levels || [];
  const effortOptions =
    `<option value=""${(ch.effort || '') === '' ? ' selected' : ''}>Default</option>` +
    harnessEffortLevels.map(lvl =>
      `<option value="${lvl}"${lvl === (ch.effort || '') ? ' selected' : ''}>${effortLabel(lvl)}</option>`
    ).join('');

  const overlay = document.createElement('div');
  overlay.className = 'new-channel-overlay';
  overlay.innerHTML = `
    <div class="new-channel-dialog">
      <h3>Edit Channel</h3>
      <label for="edit-channel-name">Name</label>
      <input type="text" id="edit-channel-name" value="${escapeHtml(ch.name)}" autofocus>
      ${modelOptions ? `<label for="edit-channel-model">Model</label>
      <select id="edit-channel-model">${modelOptions}</select>` : ''}
      <label for="edit-channel-effort">Effort</label>
      <select id="edit-channel-effort">${effortOptions}</select>
      <label for="edit-channel-workdir">Working Directory</label>
      <input type="text" id="edit-channel-workdir" value="${escapeHtml(ch.working_directory || '')}" placeholder="/path/to/project">
      <label class="new-channel-checkbox">
        <input type="checkbox" id="edit-channel-auto-approve"${ch.auto_approve_tools ? ' checked' : ''}>
        Auto-approve all tool uses
      </label>
      <div class="edit-channel-actions">
        <button class="btn" id="edit-channel-restart">Restart Agent</button>
      </div>
      <div class="edit-channel-danger">
        <div class="edit-channel-danger-label">Danger Zone</div>
        <button class="btn-danger" id="edit-channel-delete">Delete Channel</button>
      </div>
      <div class="dialog-btns">
        <button class="btn btn-cancel" id="edit-channel-cancel">Cancel</button>
        <button class="btn btn-create" id="edit-channel-save">Save</button>
      </div>
    </div>
  `;
  dialogParent.appendChild(overlay);

  const nameInput = document.getElementById('edit-channel-name');
  nameInput.focus();
  nameInput.select();

  const close = () => overlay.remove();

  // Cancel / close
  document.getElementById('edit-channel-cancel').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });

  // Save
  const workdirInput = document.getElementById('edit-channel-workdir');
  const modelSelect = document.getElementById('edit-channel-model');
  const effortSelect = document.getElementById('edit-channel-effort');
  const save = () => {
    const newName = nameInput.value.trim();
    if (newName && newName !== ch.name) {
      _editConn.renameChannel(ch.id, newName);
    }
    const updates = {};
    const newWorkdir = workdirInput.value.trim();
    if (newWorkdir !== (ch.working_directory || '')) updates.working_directory = newWorkdir;
    const newModel = modelSelect?.value;
    if (newModel && newModel !== ch.model) updates.model = newModel;
    const newEffort = effortSelect.value;
    if (newEffort !== (ch.effort || '')) updates.effort = newEffort;
    const newAutoApprove = document.getElementById('edit-channel-auto-approve').checked;
    if (newAutoApprove !== !!ch.auto_approve_tools) updates.auto_approve_tools = newAutoApprove;
    if (Object.keys(updates).length) {
      _editConn.updateChannel(ch.id, updates);
      // Update local channel data so next message picks up new values.
      if (updates.model) ch.model = updates.model;
      if (updates.effort !== undefined) ch.effort = updates.effort;
      if (updates.auto_approve_tools !== undefined) ch.auto_approve_tools = updates.auto_approve_tools;
    }
    close();
  };
  document.getElementById('edit-channel-save').addEventListener('click', save);
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(); });
  workdirInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(); });

  // Restart
  document.getElementById('edit-channel-restart').addEventListener('click', () => {
    _editConn.restartAgent(ch.id);
    close();
  });

  // Delete with confirmation
  const deleteBtn = document.getElementById('edit-channel-delete');
  let deleteConfirm = false;
  deleteBtn.addEventListener('click', () => {
    if (!deleteConfirm) {
      deleteConfirm = true;
      deleteBtn.textContent = 'Click again to confirm deletion';
      deleteBtn.classList.add('btn-danger-confirm');
      setTimeout(() => {
        deleteConfirm = false;
        deleteBtn.textContent = 'Delete Channel';
        deleteBtn.classList.remove('btn-danger-confirm');
      }, 3000);
    } else {
      _editConn.deleteChannel(ch.id);
      close();
    }
  });
}

// ===== Edit Device Dialog =====
function showEditDeviceDialog(device) {
  const dialogParent = document.body;
  if (!dialogParent) return;

  const overlay = document.createElement('div');
  overlay.className = 'new-channel-overlay';
  overlay.innerHTML = `
    <div class="new-channel-dialog">
      <h3>Edit Device</h3>
      <label for="edit-device-name">Name</label>
      <input type="text" id="edit-device-name" value="${escapeHtml(device.name)}" autofocus>
      <div class="edit-channel-actions">
        <button class="btn" id="edit-device-restart">Restart Device</button>
      </div>
      <div class="edit-channel-danger">
        <div class="edit-channel-danger-label">Danger Zone</div>
        <button class="btn-danger" id="edit-device-revoke">Revoke Device</button>
      </div>
      <div class="dialog-btns">
        <button class="btn btn-cancel" id="edit-device-cancel">Cancel</button>
        <button class="btn btn-create" id="edit-device-save">Save</button>
      </div>
    </div>
  `;
  dialogParent.appendChild(overlay);

  const nameInput = document.getElementById('edit-device-name');
  nameInput.focus();
  nameInput.select();

  const close = () => overlay.remove();

  // Cancel / close
  document.getElementById('edit-device-cancel').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });

  // Save (rename)
  const save = async () => {
    const newName = nameInput.value.trim();
    if (newName && newName !== device.name) {
      try {
        await fetch(`/api/devices/${device.id}`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: newName }),
        });
        device.name = newName;
        renderChannelPanel();
      } catch (err) {
        console.error('Failed to rename device:', err);
      }
    }
    close();
  };
  document.getElementById('edit-device-save').addEventListener('click', save);
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(); });

  // Restart
  document.getElementById('edit-device-restart').addEventListener('click', async () => {
    try {
      await fetch(`/api/devices/${device.id}/restart`, { method: 'POST' });
    } catch (err) {
      console.error('Failed to restart device:', err);
    }
    close();
  });

  // Revoke with confirmation
  const revokeBtn = document.getElementById('edit-device-revoke');
  let revokeConfirm = false;
  revokeBtn.addEventListener('click', async () => {
    if (!revokeConfirm) {
      revokeConfirm = true;
      revokeBtn.textContent = 'Click again to confirm revocation';
      revokeBtn.classList.add('btn-danger-confirm');
      setTimeout(() => {
        revokeConfirm = false;
        revokeBtn.textContent = 'Revoke Device';
        revokeBtn.classList.remove('btn-danger-confirm');
      }, 3000);
    } else {
      try {
        await fetch(`/api/devices/${device.id}`, { method: 'DELETE' });
        state.devices.delete(device.id);
        // Disconnect E2EE for this device.
        const _revokeConn = state.e2eeConnections.get(device.id);
        if (_revokeConn) _revokeConn.disconnect();
        renderChannelPanel();
        fetchDevices();
      } catch (err) {
        console.error('Failed to revoke device:', err);
      }
      close();
    }
  });
}




// ---- Expose helpers needed by vendor/markdown.js ----
window.highlightLine = highlightLine;

// ---- Expose helpers needed by feature modules during the transition ----
window.getActiveE2EE = getActiveE2EE;
window.getE2EE = getE2EE;
window.updateRailButtonStates = updateRailButtonStates;
window.renderChannelPanel = renderChannelPanel;
window.renderMessages = renderMessages;
window.selectChannel = selectChannel;
window.switchTab = switchTab;
window.saveChannelState = saveChannelState;
window.loadChannelState = loadChannelState;
window.showBrowserView = showBrowserView;
window.anyE2EEConnected = anyE2EEConnected;
window.showEditChannelDialog = showEditChannelDialog;
window.connectToDevice = connectToDevice;
window.updateChatRailUnreadBadge = updateChatRailUnreadBadge;
