import { escapeHtml, escHtml, formatBytes, formatFileSize } from './util/html.js';
import { timeAgo, shortTime, fmtRelativeAgo, fmtClock24 } from './util/time.js';
import { showToast } from './util/toast.js';

// Electron detection
if (window.buildElectron) {
  document.body.classList.add('electron');
}

// ===== UI State =====
// 'files' is the default/main view. 'browser' is activated when a browser tab is selected.
let currentTab = 'files';
let selectedAgent = null;
let consoleState = 'collapsed'; // 'collapsed' | 'open' | 'expanded'

let deviceDown = null; // {id, name} when connected device is offline

// ===== Device State =====
const devices = new Map(); // id -> device object
const activityItems = [];
const MAX_ACTIVITY = 50;

// ===== DOM References =====
const consoleBtm = document.getElementById('console-bottom');
const consoleToggle = document.getElementById('console-toggle');
const consoleExpand = document.getElementById('console-expand');

// ===== Tab Navigation (files is default; chat is in overlay; browser is toggled) =====
function switchTab(tab) {
  currentTab = tab;
  // Only show panels that belong to the main viewer (not the detached chat).
  document.querySelectorAll('.tab-panel').forEach(p => {
    if (p.dataset.detached === 'true') return;
    p.classList.toggle('active', p.id === 'tab-' + tab);
  });
  if (tab === 'files') onFilesTabActivated();
}

// ===== Channel Panel =====
function renderChannelPanel() {
  const list = document.getElementById('channel-panel-list');
  if (!list) return;
  list.innerHTML = '';

  const sortedDevices = [...devices.values()].sort((a, b) => a.name.localeCompare(b.name));

  for (const device of sortedDevices) {
    const group = document.createElement('div');
    group.className = 'device-group';

    const hasConnection = e2eeConnections.has(device.id);
    const conn = e2eeConnections.get(device.id);
    const isE2eeConnected = hasConnection && conn && conn.connected;
    const isOnline = device.status === 'online';

    // Group header
    const header = document.createElement('div');
    header.className = 'device-group-header';

    const chevronClass = hasConnection ? '' : ' collapsed';
    let statusHtml = '';
    if (isE2eeConnected) {
      statusHtml = '<svg class="status-lock" viewBox="0 0 16 16" fill="none"><path d="M4 7V5a4 4 0 118 0v2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><rect x="3" y="7" width="10" height="7" rx="1.5" fill="currentColor"/></svg>';
    } else if (hasConnection) {
      statusHtml = '<span class="status-dot connecting"></span>';
    } else if (!isOnline) {
      statusHtml = '<span class="status-dot offline"></span>';
    } else {
      statusHtml = '<span class="status-dot"></span>';
    }

    header.innerHTML = `
      <svg class="device-group-chevron${chevronClass}" viewBox="0 0 12 12" fill="none"><path d="M4 2l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
      <span class="device-group-status">${statusHtml}</span>
      <span>${escapeHtml(device.name)}</span>
    `;

    header.addEventListener('click', () => {
      if (hasConnection) {
        // Toggle collapse
        const channels = group.querySelector('.device-group-channels');
        if (channels) channels.classList.toggle('collapsed');
        header.querySelector('.device-group-chevron').classList.toggle('collapsed');
      } else if (isOnline && device.has_transport_key) {
        connectToDevice(device);
      }
    });
    group.appendChild(header);

    // New Session button (below device header, above channels)
    if (hasConnection) {
      const newBtn = document.createElement('button');
      newBtn.className = 'sidebar-new-session';
      const btnRow = document.createElement('div');
      btnRow.className = 'device-group-actions';

      newBtn.title = 'New session';
      newBtn.innerHTML = '<svg viewBox="0 0 12 12" fill="none"><path d="M6 2v8M2 6h8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg> New Session';
      newBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        document.getElementById('btn-new-channel')?.click();
      });
      btnRow.appendChild(newBtn);

      const browseBtn = document.createElement('button');
      browseBtn.className = 'sidebar-browse-btn';
      browseBtn.title = 'Browse localhost';
      browseBtn.innerHTML = '<svg viewBox="0 0 12 12" fill="none"><circle cx="6" cy="6" r="4.5" stroke="currentColor" stroke-width="1.2"/><path d="M1.5 6h9M6 1.5c1.5 1.5 2 3 2 4.5s-.5 3-2 4.5M6 1.5c-1.5 1.5-2 3-2 4.5s.5 3 2 4.5" stroke="currentColor" stroke-width="1"/></svg>';
      browseBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        addBrowserTab(device.id);
      });
      btnRow.appendChild(browseBtn);

      group.appendChild(btnRow);
    }

    // Channels for connected or cached devices
    const hasCachedChannels = deviceChannels.has(device.id) && deviceChannels.get(device.id).size > 0;
    if (hasConnection || hasCachedChannels) {
      const channelsDiv = document.createElement('div');
      channelsDiv.className = 'device-group-channels';
      const devChans = deviceChannels.get(device.id) || new Map();
      const sorted = [...devChans.values()].sort((a, b) => {
        const aTs = channelSortTs.get(a.id) || 0;
        const bTs = channelSortTs.get(b.id) || 0;
        if (aTs !== bTs) return bTs - aTs;
        return (b.created_at || 0) - (a.created_at || 0);
      });
      for (const ch of sorted) {
        const item = createChannelItem(ch);
        channelsDiv.appendChild(item);
      }
      // Browser tabs for this device
      const tabs = browserTabs.get(device.id) || [];
      for (const tab of tabs) {
        const item = createBrowserTabItem(tab, device.id);
        channelsDiv.appendChild(item);
      }
      group.appendChild(channelsDiv);
    }

    list.appendChild(group);
  }

  // Also update mobile channel label
  updateMobileChannelLabel();
}

function createChannelItem(ch) {
  const item = document.createElement('div');
  const isActive = ch.id === chatCurrentChannel;
  const uc = (typeof unreadCounts !== 'undefined') ? unreadCounts.get(ch.id) : null;
  const count = uc?.messages || 0;
  const hasInt = uc?.hasInteraction || false;
  item.className = 'channel-sidebar-item' + (isActive ? ' active' : '') + (hasInt ? ' has-interaction' : '');

  const name = ch.name || ch.id.slice(0, 8);
  const badgeHtml = count > 0 ? `<span class="ch-unread-badge">${count}</span>` : '';
  item.innerHTML = `
    <span class="ch-hash">#</span>
    <span class="ch-name">${escapeHtml(name)}</span>
    ${badgeHtml}
    <button class="ch-edit" title="Edit channel"><svg viewBox="0 0 12 12" fill="none"><path d="M8.5 1.5l2 2M1 11l.7-2.8L9 1l2 2-7.2 7.2L1 11z" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"/></svg></button>
  `;

  item.addEventListener('click', (e) => {
    if (e.target.closest('.ch-edit')) return;
    selectChannel(ch.id);
  });
  const editBtn = item.querySelector('.ch-edit');
  if (editBtn) {
    editBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      showEditChannelDialog(ch);
    });
  }
  return item;
}

function updateMobileChannelLabel() {
  const label = document.getElementById('mobile-channel-label');
  if (!label) return;
  const trigger = document.getElementById('mobile-channel-trigger');
  const hashEl = trigger?.querySelector('.dd-hash');
  if (chatCurrentChannel) {
    const ch = chatChannels.get(chatCurrentChannel);
    label.textContent = ch ? (ch.name || ch.id.slice(0, 8)) : 'Select channel';
    // Show lock instead of hash when e2ee is connected
    const isE2eeConnected = anyE2EEConnected();
    if (hashEl) {
      if (isE2eeConnected) {
        hashEl.innerHTML = '<svg class="mobile-lock" viewBox="0 0 16 16" fill="none"><path d="M4 7V5a4 4 0 118 0v2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><rect x="3" y="7" width="10" height="7" rx="1.5" fill="currentColor"/></svg>';
      } else {
        hashEl.textContent = '#';
      }
    }
  } else {
    label.textContent = 'Select channel';
    if (hashEl) hashEl.textContent = '#';
  }

  // Aggregate unread counter and interaction pulse.
  if (trigger && typeof unreadCounts !== 'undefined') {
    let totalUnread = 0;
    let anyInteraction = false;
    for (const [chId, uc] of unreadCounts) {
      if (chId === chatCurrentChannel) continue;
      totalUnread += uc.messages;
      if (uc.hasInteraction) anyInteraction = true;
    }

    let badge = trigger.querySelector('.ch-unread-badge');
    if (totalUnread > 0) {
      if (!badge) {
        badge = document.createElement('span');
        badge.className = 'ch-unread-badge';
        trigger.insertBefore(badge, trigger.querySelector('.dd-chevron'));
      }
      badge.textContent = totalUnread;
    } else if (badge) {
      badge.remove();
    }

    trigger.classList.toggle('has-interaction', anyInteraction);
  }

  // Keep the bottom-rail Chat badge in sync with the same unread roll-up.
  if (typeof updateChatRailUnreadBadge === 'function') updateChatRailUnreadBadge();
}

async function connectToDevice(device) {
  if (!e2eeConnections.has(device.id)) {
    await connectDeviceE2EE(device.id);
  }
  renderChannelPanel();
}

function selectAgent(name, device) {
  selectedAgent = { name, device };
  renderChannelPanel();
  switchTab('chat');
  if (chatCurrentChannel) {
    location.hash = `chat/${chatCurrentChannel}`;
  } else {
    location.hash = 'chat';
  }
}

// ===== Custom dropdown toggle logic =====
let _openDropdown = null;

function closeAllDropdowns() {
  document.querySelectorAll('.top-dropdown-menu.open').forEach(m => m.classList.remove('open'));
  document.querySelectorAll('.top-dropdown-trigger.open').forEach(t => t.classList.remove('open'));
  _openDropdown = null;
}

function positionDropdownMenu(trigger, menu) {
  const tr = trigger.getBoundingClientRect();
  menu.style.top = (tr.bottom + 4) + 'px';
  menu.style.left = tr.left + 'px';
  requestAnimationFrame(() => {
    const mr = menu.getBoundingClientRect();
    if (mr.right > window.innerWidth - 8) {
      menu.style.left = Math.max(8, window.innerWidth - mr.width - 8) + 'px';
    }
  });
}

function toggleDropdown(dropdownId) {
  const trigger = document.getElementById(dropdownId + '-trigger');
  const menu = document.getElementById(dropdownId + '-menu');
  if (!trigger || !menu) return;
  const isOpen = menu.classList.contains('open');
  closeAllDropdowns();
  if (!isOpen) {
    trigger.classList.add('open');
    menu.classList.add('open');
    positionDropdownMenu(trigger, menu);
    _openDropdown = dropdownId;
  }
}

// Close dropdowns on any click outside
document.addEventListener('click', (e) => {
  if (!_openDropdown) return;
  const dropdown = document.getElementById(_openDropdown);
  if (dropdown && !dropdown.contains(e.target)) {
    closeAllDropdowns();
  }
}, true);

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

function setConsoleState(state) {
  consoleState = state;
  const isCollapsed = state === 'collapsed';
  consoleBtm.classList.toggle('collapsed', isCollapsed);
  consoleBtm.classList.toggle('rail-only', isCollapsed);
  consoleBtm.classList.toggle('expanded', state === 'expanded');
  const term = document.querySelector('[data-console-panel="terminal"]');
  if (term) term.classList.toggle('hidden', isCollapsed);
  updateConsoleButtons();
  updateRailButtonStates();
}
function updateConsoleButtons() {
  const s = consoleState;
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
  if (tBtn) tBtn.classList.toggle('active', consoleState !== 'collapsed');
  const cBtn = document.getElementById('console-chat-toggle');
  const overlay = document.getElementById('chat-overlay');
  if (cBtn && overlay) cBtn.classList.toggle('active', overlay.classList.contains('open'));
}

function updateChatRailUnreadBadge() {
  const badge = document.getElementById('chat-rail-badge');
  if (!badge) return;
  let total = 0;
  if (typeof unreadCounts !== 'undefined') {
    for (const [chId, uc] of unreadCounts) {
      if (chId === chatCurrentChannel) continue;
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
  if (consoleState === 'collapsed') {
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
const channelTodos = new Map(); // channelId -> [{id, content, status}]

// ===== Sidebar accordion =====
function setSidebarSectionOpen(name, open, persist = true) {
  const section = document.querySelector(`.sidebar-section[data-section="${name}"]`);
  if (!section) return;
  const body = section.querySelector('.sidebar-section-body');
  const chevron = section.querySelector('.sidebar-section-chevron');
  body?.classList.toggle('collapsed', !open);
  chevron?.classList.toggle('collapsed', !open);
  section.classList.toggle('collapsed', !open);
  if (persist) {
    try { localStorage.setItem(`sidebar.section.${name}.collapsed`, open ? '0' : '1'); } catch (_) {}
  }
}
function toggleSidebarSection(name) {
  const section = document.querySelector(`.sidebar-section[data-section="${name}"]`);
  if (!section) return;
  const body = section.querySelector('.sidebar-section-body');
  if (!body) return;
  setSidebarSectionOpen(name, body.classList.contains('collapsed'));
}
function expandSidebarSection(name) {
  // Auto-expansion (triggered by arriving activity) does NOT persist, so a
  // page reload still respects the user's default-collapsed preference.
  setSidebarSectionOpen(name, true, false);
}
document.querySelectorAll('.sidebar-section-header[data-sidebar-toggle]').forEach(h => {
  h.addEventListener('click', () => toggleSidebarSection(h.dataset.sidebarToggle));
});
// Sync each section's `.collapsed` class with its body's initial state, then
// apply any saved override. Ensures collapsed sections get flex:0 0 auto.
for (const section of document.querySelectorAll('.sidebar-section')) {
  const body = section.querySelector('.sidebar-section-body');
  section.classList.toggle('collapsed', !!body?.classList.contains('collapsed'));
}
try {
  for (const name of ['tasks', 'activity']) {
    const saved = localStorage.getItem(`sidebar.section.${name}.collapsed`);
    if (saved === '1') setSidebarSectionOpen(name, false);
    else if (saved === '0') setSidebarSectionOpen(name, true);
  }
} catch (_) {}

// ===== Activity timestamps =====
// Last 30 entries show "Ns/Nm/Nh/Nd ago"; older entries show 24h "HH:MM".
const CONSOLE_RECENT_COUNT = 30;
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

const CHAT_OVERLAY_STATE_KEY = 'chat.overlay.state';
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
function isChatOverlayOpen() {
  return !!document.getElementById('chat-overlay')?.classList.contains('open');
}
function openChatOverlay(opts) {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  ov.classList.add('open');
  ov.setAttribute('aria-hidden', 'false');
  // Honor persisted pin/mode if present; otherwise default to pinned.
  const saved = _readChatOverlayState() || {};
  const pin = (opts && 'pinned' in opts) ? !!opts.pinned : (saved.pinned !== false);
  setChatOverlayPinned(pin, { persist: !opts?.suppressPersist });
  if (saved.mode === 'expanded') _setChatOverlayExpanded(true);
  if (!opts?.suppressPersist) _writeChatOverlayState({ open: true });
  updateRailButtonStates();
  setTimeout(() => document.getElementById('chat-input')?.focus(), 80);
}
function closeChatOverlay(opts) {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  ov.classList.remove('open');
  ov.removeAttribute('data-mode');
  ov.setAttribute('aria-hidden', 'true');
  if (!opts?.suppressPersist) _writeChatOverlayState({ open: false, mode: null });
  updateRailButtonStates();
}
function toggleChatOverlay() {
  if (isChatOverlayOpen()) closeChatOverlay();
  else openChatOverlay();
}
function setChatOverlayPinned(pinned, opts) {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  ov.setAttribute('data-pinned', pinned ? 'true' : 'false');
  document.getElementById('chat-overlay-pin')?.classList.toggle('active', !!pinned);
  if (opts?.persist !== false) _writeChatOverlayState({ pinned: !!pinned });
}
const CHAT_EXPAND_SVG = '<path d="M9 2h5v5M7 14H2V9"/><path d="M14 2l-5 5M2 14l5-5"/>';
const CHAT_SHRINK_SVG = '<path d="M14 7h-5V2M2 9h5v5"/><path d="M14 2l-5 5M2 14l5-5"/>';
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
function toggleChatOverlayExpanded() {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  _setChatOverlayExpanded(ov.getAttribute('data-mode') !== 'expanded');
}
document.getElementById('chat-overlay-minimize')?.addEventListener('click', () => closeChatOverlay());
document.getElementById('chat-overlay-expand')?.addEventListener('click', toggleChatOverlayExpanded);
document.getElementById('chat-overlay-pin')?.addEventListener('click', () => {
  const ov = document.getElementById('chat-overlay');
  if (!ov) return;
  setChatOverlayPinned(ov.getAttribute('data-pinned') !== 'true');
});

// On initial load, restore the persisted chat-overlay state (default: open + pinned).
(function initChatOverlayState() {
  const saved = _readChatOverlayState() || {};
  const shouldOpen = saved.open !== false; // default true
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
function syncChatOverlayHeader() {
  const nameEl = document.getElementById('chat-overlay-channel-name');
  if (!nameEl) return;
  const ch = (typeof chatChannels !== 'undefined' && chatCurrentChannel) ? chatChannels.get(chatCurrentChannel) : null;
  nameEl.textContent = ch ? (ch.name || chatCurrentChannel.slice(0, 8)) : 'Select a channel';
}

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
  const ch = (typeof chatChannels !== 'undefined' && channelId) ? chatChannels.get(channelId) : null;
  if (!ch) {
    modelName.textContent = '—';
    effortName.textContent = '—';
    if (modelOpts) modelOpts.innerHTML = '';
    if (effortOpts) effortOpts.innerHTML = '';
    return;
  }
  const deviceId = channelDeviceMap.get(channelId);
  const harnesses = deviceId ? deviceHarnesses.get(deviceId) : null;
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

// ----- Tasks -----
function renderTasksPanel(channelId) {
  const list = document.getElementById('tasks-list');
  const empty = document.getElementById('tasks-empty');
  if (!list) return;
  const todos = channelTodos.get(channelId) || [];
  list.innerHTML = '';
  if (todos.length === 0) {
    if (empty) empty.classList.remove('hidden');
    return;
  }
  if (empty) empty.classList.add('hidden');
  const completed = todos.filter(t => t.status === 'completed').length;
  const summary = document.createElement('div');
  summary.className = 'tasks-summary';
  const pct = todos.length ? Math.round(completed / todos.length * 100) : 0;
  summary.innerHTML = `<span>${completed}/${todos.length} completed</span><div class="tasks-progress-bar"><div class="tasks-progress-fill" style="width:${pct}%"></div></div>`;
  list.appendChild(summary);
  const order = { in_progress: 0, pending: 1, completed: 2 };
  const sorted = [...todos].sort((a, b) => (order[a.status] ?? 1) - (order[b.status] ?? 1));
  for (const todo of sorted) {
    const row = document.createElement('div');
    row.className = `task-item ${todo.status}`;
    const icon = document.createElement('span');
    icon.className = 'task-status-icon';
    if (todo.status === 'in_progress') {
      icon.innerHTML = '<span class="task-pulse-dot"></span>';
    } else {
      icon.textContent = todo.status === 'completed' ? '✓' : '○';
    }
    const content = document.createElement('span');
    content.className = 'task-content';
    content.textContent = todo.content;
    row.appendChild(icon);
    row.appendChild(content);
    list.appendChild(row);
  }
}

function updateTasksBadge(channelId) {
  if (chatCurrentChannel !== channelId) return;
  const badge = document.getElementById('sidebar-tasks-badge');
  if (!badge) return;
  const todos = channelTodos.get(channelId) || [];
  const incomplete = todos.filter(t => t.status !== 'completed').length;
  if (incomplete > 0) {
    badge.textContent = String(incomplete);
    badge.classList.remove('hidden');
    badge.classList.add('accent');
  } else {
    badge.classList.add('hidden');
    badge.classList.remove('accent');
  }
}

// ----- Terminal -----
const deviceAgentCwd = new Map(); // deviceId -> cwd string
const terminalCwdMap = new Map(); // channelId -> cwd
const terminalHistoryMap = new Map(); // channelId -> [{cmd, output}]
const terminalCmdHistory = []; // global command history for up/down
let terminalCmdIndex = -1;
let terminalRunning = false;
let terminalCurrentBlock = null; // current streaming output block
let terminalCompletionPending = false; // waiting for completions response
let terminalCompletions = [];  // current completion candidates
let terminalCompletionIndex = -1; // cycling index (-1 = common prefix)
let terminalCompletionBase = ''; // line before the partial being completed
let terminalCompletionPartial = ''; // the partial word sent in the request

function getTerminalCwd(channelId) {
  if (terminalCwdMap.has(channelId)) return terminalCwdMap.get(channelId);
  const ch = chatChannels.get(channelId);
  const devId = channelDeviceMap.get(channelId);
  return ch?.working_directory || (devId ? deviceAgentCwd.get(devId) : '') || '';
}

function shortCwd(cwd) {
  if (!cwd) return '~';
  const parts = cwd.split('/').filter(Boolean);
  return parts.length > 2 ? '…/' + parts.slice(-2).join('/') : cwd;
}

function renderTerminalCwd() {
  const el = document.getElementById('terminal-cwd');
  const cwd = getTerminalCwd(chatCurrentChannel) || '~';
  el.textContent = shortCwd(cwd) + ' $';
  el.title = cwd;
}

function renderTerminalForChannel(channelId) {
  const output = document.getElementById('terminal-output');
  const promptRow = document.getElementById('terminal-prompt-row');
  // Remove prompt row before clearing, we'll re-append it.
  promptRow?.remove();
  output.innerHTML = '';
  const history = terminalHistoryMap.get(channelId) || [];
  for (const entry of history) {
    const block = document.createElement('div');
    block.className = 'terminal-cmd-block';
    const cwdLabel = entry.cwd ? shortCwd(entry.cwd) + ' $' : '$';
    block.innerHTML = `<div class="terminal-cmd-line">${escapeHtml(cwdLabel)} ${escapeHtml(entry.cmd)}</div>`;
    if (entry.output) {
      block.innerHTML += `<div class="terminal-cmd-output">${escapeHtml(entry.output)}</div>`;
    }
    if (entry.exitCode != null && entry.exitCode !== 0) {
      block.innerHTML += `<div class="terminal-cmd-exit error">exit ${entry.exitCode}</div>`;
    }
    output.appendChild(block);
  }
  // Re-append prompt row and show/hide based on running state.
  if (promptRow) {
    output.appendChild(promptRow);
    promptRow.classList.toggle('hidden', terminalRunning);
  }
  output.scrollTop = output.scrollHeight;
  renderTerminalCwd();
}

function terminalExec(command) {
  if (!command || !chatCurrentChannel || !getActiveE2EE()?.connected || terminalRunning) return;
  terminalRunning = true;

  const input = document.getElementById('terminal-input');
  const promptRow = document.getElementById('terminal-prompt-row');
  input.value = '';

  // Add to command history
  terminalCmdHistory.push(command);
  terminalCmdIndex = terminalCmdHistory.length;

  const channelId = chatCurrentChannel;
  const cwd = getTerminalCwd(channelId);
  const cwdLabel = cwd ? shortCwd(cwd) + ' $' : '$';

  // Hide prompt row while running.
  promptRow.classList.add('hidden');

  // Create output block (inserted before the prompt row).
  const output = document.getElementById('terminal-output');
  const block = document.createElement('div');
  block.className = 'terminal-cmd-block';
  block.innerHTML = `<div class="terminal-cmd-line">${escapeHtml(cwdLabel)} ${escapeHtml(command)}</div>`;
  const outputDiv = document.createElement('div');
  outputDiv.className = 'terminal-cmd-output';
  block.appendChild(outputDiv);
  output.insertBefore(block, promptRow);
  output.scrollTop = output.scrollHeight;
  const commandId = crypto.randomUUID();
  terminalCurrentBlock = { block, outputDiv, channelId, cmd: command, text: '', commandId, hasOutput: false };

  // Track in history
  if (!terminalHistoryMap.has(channelId)) terminalHistoryMap.set(channelId, []);
  terminalHistoryMap.get(channelId).push({ cmd: command, output: '', exitCode: null, cwd });

  getActiveE2EE()?.terminalExec(channelId, command, cwd || undefined, commandId);

  // Show kill button while command is running.
  document.getElementById('terminal-kill-btn')?.classList.remove('hidden');

  // Show braille loading animation if no output within 2 seconds.
  terminalLoadingTimer = setTimeout(() => {
    if (terminalCurrentBlock && !terminalCurrentBlock.hasOutput) {
      const loader = document.createElement('span');
      loader.className = 'terminal-loader';
      terminalCurrentBlock.outputDiv.appendChild(loader);
      terminalCurrentBlock.loader = loader;
      let frame = 0;
      const frames = ['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];
      terminalLoadingInterval = setInterval(() => {
        loader.textContent = frames[frame % frames.length];
        frame++;
      }, 80);
    }
  }, 2000);


  // 30s no-output timeout — if no output arrives, assume command is stuck.
  terminalNoOutputTimer = setTimeout(() => {
    if (terminalCurrentBlock?.commandId === commandId && !terminalCurrentBlock.hasOutput) {
      finishTerminalCommand(
        'Error: Command produced no output for 30 seconds. It may require an interactive terminal.\r\n', 1,
      );
    }
  }, 30000);
}
let terminalLoadingTimer = null;
let terminalLoadingInterval = null;
let terminalNoOutputTimer = null;
let terminalKillTimer = null;

function clearTerminalTimers() {
  if (terminalLoadingTimer) { clearTimeout(terminalLoadingTimer); terminalLoadingTimer = null; }
  if (terminalLoadingInterval) { clearInterval(terminalLoadingInterval); terminalLoadingInterval = null; }
  if (terminalNoOutputTimer) { clearTimeout(terminalNoOutputTimer); terminalNoOutputTimer = null; }
  if (terminalKillTimer) { clearTimeout(terminalKillTimer); terminalKillTimer = null; }
  if (terminalCurrentBlock?.loader) { terminalCurrentBlock.loader.remove(); terminalCurrentBlock.loader = null; }
}

function finishTerminalCommand(errorData, exitCode) {
  clearTerminalTimers();
  if (terminalCurrentBlock) {
    if (errorData) {
      const span = document.createElement('span');
      span.className = 'terminal-cmd-exit error';
      span.textContent = errorData;
      terminalCurrentBlock.outputDiv.appendChild(span);
    }
    if (exitCode !== 0) {
      const exitDiv = document.createElement('div');
      exitDiv.className = 'terminal-cmd-exit error';
      exitDiv.textContent = `exit ${exitCode}`;
      terminalCurrentBlock.block.appendChild(exitDiv);
    }
    terminalCurrentBlock = null;
  }
  terminalRunning = false;
  document.getElementById('terminal-kill-btn')?.classList.add('hidden');
  const promptRow = document.getElementById('terminal-prompt-row');
  if (promptRow) promptRow.classList.remove('hidden');
  const output = document.getElementById('terminal-output');
  if (output) output.scrollTop = output.scrollHeight;
  document.getElementById('terminal-input')?.focus();
}

function clearTerminalCompletions() {
  terminalCompletions = [];
  terminalCompletionIndex = -1;
  terminalCompletionBase = '';
  terminalCompletionPartial = '';
  document.querySelectorAll('.terminal-completions').forEach(el => el.remove());
}

document.getElementById('terminal-input')?.addEventListener('keydown', (e) => {
  if (e.key === 'Tab') {
    e.preventDefault();
    if (terminalCompletionPending || terminalRunning) return;

    // If we already have completions, cycle through them
    if (terminalCompletions.length > 1) {
      terminalCompletionIndex = (terminalCompletionIndex + 1) % terminalCompletions.length;
      const match = terminalCompletions[terminalCompletionIndex];
      const suffix = match.endsWith('/') ? '' : ' ';
      e.target.value = terminalCompletionBase + match + suffix;
      return;
    }

    const line = e.target.value;
    if (!line) return;
    // Extract the partial word (last whitespace-delimited token)
    const words = line.split(/\s+/);
    const partial = words[words.length - 1] || '';
    const cwd = getTerminalCwd(chatCurrentChannel);
    const _conn = getActiveE2EE();
    if (_conn && _conn.connected && chatCurrentChannel) {
      terminalCompletionPending = true;
      terminalCompletionPartial = partial;
      terminalCompletionBase = line.slice(0, line.length - partial.length);
      _conn.terminalComplete(chatCurrentChannel, partial, line, cwd);
    }
  } else if (e.key === 'Enter' && !e.shiftKey) {
    clearTerminalCompletions();
    e.preventDefault();
    terminalExec(e.target.value.trim());
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    if (terminalCmdIndex > 0) {
      terminalCmdIndex--;
      e.target.value = terminalCmdHistory[terminalCmdIndex] || '';
    }
  } else if (e.key === 'ArrowDown') {
    e.preventDefault();
    if (terminalCmdIndex < terminalCmdHistory.length - 1) {
      terminalCmdIndex++;
      e.target.value = terminalCmdHistory[terminalCmdIndex] || '';
    } else {
      terminalCmdIndex = terminalCmdHistory.length;
      e.target.value = '';
    }
  } else if (e.key !== 'Shift' && e.key !== 'Control' && e.key !== 'Alt' && e.key !== 'Meta') {
    // Slash while cycling a dir completion — accept it, don't double the slash
    if (e.key === '/' && terminalCompletions.length && terminalCompletionIndex >= 0) {
      const current = terminalCompletions[terminalCompletionIndex];
      if (current.endsWith('/')) {
        e.preventDefault();
        clearTerminalCompletions();
        return;
      }
    }
    // Any other key clears completion state
    if (terminalCompletions.length) clearTerminalCompletions();
  }
});

document.getElementById('terminal-kill-btn')?.addEventListener('click', () => {
  const _killConn = getActiveE2EE();
  if (_killConn && _killConn.connected && chatCurrentChannel) {
    const cmdId = terminalCurrentBlock?.commandId || '';
    _killConn.terminalKill(chatCurrentChannel, cmdId);
    // If no done frame within 5s, force-reset the terminal locally.
    terminalKillTimer = setTimeout(() => {
      if (terminalCurrentBlock?.commandId === cmdId) {
        finishTerminalCommand('^C (kill timeout)\r\n', 130);
      }
    }, 5000);
  }
});

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

// ----- Complications state & rendering -----
const complicationState = new Map(); // channelId -> Map(complicationId -> data)
let compPopoverOpen = null; // complication id currently expanded, or null

function renderComplications() {
  const scroll = document.getElementById('comp-scroll');
  const bar = document.getElementById('complications');
  if (!scroll || !bar) return;
  const channelComps = complicationState.get(chatCurrentChannel);
  if (!channelComps || channelComps.size === 0) {
    scroll.innerHTML = '';
    bar.classList.add('hidden');
    closeCompPopover();
    return;
  }
  bar.classList.remove('hidden');
  // Filter git complications to only those whose repo is a parent of the channel's working directory.
  const channelWd = getTerminalCwd(chatCurrentChannel);
  // Sort by timestamp descending (most recent first).
  const sorted = [...channelComps.values()]
    .filter(comp => {
      if (comp.kind === 'git-status' && channelWd && comp.data?.repo) {
        const repo = comp.data.repo;
        let wd = channelWd;
        // Expand ~ using home dir inferred from the absolute repo path.
        if (wd.startsWith('~/') && repo.startsWith('/')) {
          const homeMatch = repo.match(/^(\/(?:Users|home)\/[^/]+)/);
          if (homeMatch) wd = homeMatch[1] + wd.slice(1);
        }
        const r = repo.endsWith('/') ? repo : repo + '/';
        const w = wd.endsWith('/') ? wd : wd + '/';
        return w.startsWith(r) || r.startsWith(w);
      }
      return true;
    })
    .sort((a, b) => (b.timestamp || 0) - (a.timestamp || 0));
  if (sorted.length === 0) {
    scroll.innerHTML = '';
    bar.classList.add('hidden');
    closeCompPopover();
    return;
  }
  scroll.innerHTML = sorted.map(comp => {
    if (comp.kind === 'git-status') return renderGitCompChip(comp);
    return '';
  }).join('');
  // Attach click handlers.
  scroll.querySelectorAll('[data-comp-id]').forEach(el => {
    el.addEventListener('click', () => toggleCompPopover(el.dataset.compId));
  });
}

function renderGitCompChip(comp) {
  const d = comp.data || {};
  const branch = escapeHtml(d.branch || '?');
  const remote = d.remote_name ? escapeHtml(d.remote_name) : null;
  const ins = d.insertions || 0;
  const del = d.deletions || 0;
  const gitIcon = `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M6 3v10M10 3v4"/><circle cx="6" cy="14" r="1.5"/><circle cx="10" cy="8" r="1.5"/></svg>`;
  // Line 1: remote/branch
  let line1 = `<span class="comp-line">${gitIcon}`;
  if (remote) line1 += `<span class="val">${remote}</span><span class="v">/</span>`;
  line1 += `<span class="val">${branch}</span></span>`;
  // Line 2: stats (always shown)
  let stats = [];
  stats.push(`<span class="v green">+${ins}</span>`);
  stats.push(`<span class="v red">&minus;${del}</span>`);
  if (d.ahead > 0) stats.push(`<span class="v">&uarr;${d.ahead}</span>`);
  if (d.behind > 0) stats.push(`<span class="v">&darr;${d.behind}</span>`);
  if (d.conflicts > 0) stats.push(`<span class="v red">⚠${d.conflicts}</span>`);
  const line2 = `<span class="comp-line comp-stats">${stats.join('')}</span>`;
  const active = compPopoverOpen === comp.id ? ' active' : '';
  return `<div class="comp clickable${active}" data-comp-id="${escapeHtml(comp.id)}">${line1}${line2}</div>`;
}

function toggleCompPopover(compId) {
  if (compPopoverOpen === compId) {
    closeCompPopover();
    return;
  }
  compPopoverOpen = compId;
  renderComplications(); // re-render chips to show active state
  const popover = document.getElementById('comp-popover');
  if (!popover) return;
  const channelComps = complicationState.get(chatCurrentChannel);
  const comp = channelComps?.get(compId);
  if (!comp || comp.kind !== 'git-status') { closeCompPopover(); return; }
  const d = comp.data || {};
  const staged = d.staged || {};
  const unstaged = d.unstaged || {};
  let html = `<div class="cp-section"><div class="cp-label">Branch</div><div class="cp-row"><span class="cp-stat">${escapeHtml(d.branch || '?')}</span>`;
  if (d.upstream) html += ` <span class="text-muted">&rarr; ${escapeHtml(d.upstream)}</span>`;
  html += `</div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Staged</div><div class="cp-row"><span class="cp-stat green">+${staged.added||0}</span> <span class="cp-stat amber">~${staged.modified||0}</span> <span class="cp-stat red">&minus;${staged.deleted||0}</span></div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Unstaged</div><div class="cp-row"><span class="cp-stat green">+${unstaged.added||0}</span> <span class="cp-stat amber">~${unstaged.modified||0}</span> <span class="cp-stat red">&minus;${unstaged.deleted||0}</span></div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Untracked</div><div class="cp-row"><span class="cp-stat">${d.untracked||0}</span></div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Remote</div><div class="cp-row">`;
  html += `<span class="cp-stat">&uarr;${d.ahead||0} ahead</span>`;
  html += `<span class="cp-stat">&darr;${d.behind||0} behind</span>`;
  html += `</div></div>`;
  if (d.last_fetch) {
    const ago = Math.round((Date.now() - d.last_fetch) / 1000);
    const agoStr = ago < 60 ? `${ago}s ago` : ago < 3600 ? `${Math.round(ago/60)}m ago` : `${Math.round(ago/3600)}h ago`;
    html += `<div class="cp-section"><div class="cp-label">Last fetch</div><div class="cp-row">${agoStr}</div></div>`;
  }
  // Action buttons.
  const options = comp.options || [];
  if (options.length) {
    html += `<div class="cp-actions">`;
    for (const opt of options) {
      html += `<button class="cp-btn" data-action="${escapeHtml(opt.id)}" ${opt.enabled ? '' : 'disabled'}>${escapeHtml(opt.label)}</button>`;
    }
    html += `</div>`;
  }
  popover.innerHTML = html;
  popover.classList.remove('hidden');
  // Attach action handlers.
  popover.querySelectorAll('.cp-btn[data-action]').forEach(btn => {
    btn.addEventListener('click', () => {
      if (btn.disabled) return;
      const actionId = btn.dataset.action;
      btn.disabled = true;
      btn.textContent += '…';
      sendComplicationAction(compId, actionId);
    });
  });
}

function closeCompPopover() {
  compPopoverOpen = null;
  const popover = document.getElementById('comp-popover');
  if (popover) { popover.classList.add('hidden'); popover.innerHTML = ''; }
}

function sendComplicationAction(compId, optionId) {
  const _conn = getActiveE2EE();
  if (!_conn || !_conn.connected || !chatCurrentChannel) return;
  _conn.send({
    action: 'complication:action',
    channel_id: chatCurrentChannel,
    complication_id: compId,
    option_id: optionId,
  });
}

// Close popover when clicking outside.
document.addEventListener('click', (e) => {
  if (compPopoverOpen && !e.target.closest('.comp-popover') && !e.target.closest('.comp[data-comp-id]')) {
    closeCompPopover();
    renderComplications();
  }
});

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

// =====================================================================
// Device Dashboard — live data via Skrift notifications
// =====================================================================

// Configure Skrift notifications for persistent connection
function initNotifications() {
  if (window.__skriftNotifications) {
    window.__skriftNotifications.configure({ persistConnection: true });
    // Patch _healthCheck to clear _hiddenSince in the CLOSED branch,
    // preventing a double reconnect when both visibilitychange and focus fire.
    const sn = window.__skriftNotifications;
    const origHealthCheck = sn._healthCheck.bind(sn);
    sn._healthCheck = function () {
      if (this._es && this._es.readyState === EventSource.CLOSED) {
        this._hiddenSince = null;
      }
      return origHealthCheck();
    }.bind(sn);
  } else {
    // Retry if not yet initialized
    setTimeout(initNotifications, 100);
  }
}
initNotifications();

/** True when the SSE connection has completed sync (replay phase is over). */
function sseSynced() {
  return window.__skriftNotifications?._synced === true;
}

// ----- Helpers -----

// ----- Rendering -----

function renderDeviceCard(device) {
  const isOnline = device.status === 'online';
  const statusClass = isOnline ? '' : 'offline';
  const statusText = isOnline ? 'Online' : 'Offline';
  const heartbeatText = device.last_heartbeat_at
    ? timeAgo(device.last_heartbeat_at)
    : 'never';
  const missedCount = (device.missed_heartbeat_windows || []).length;

  return `
    <div class="device-card glass" data-device-id="${device.id}">
      <div class="device-header">
        <div class="device-icon ${statusClass}">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
            <rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>
          </svg>
        </div>
        <span class="device-name">${escapeHtml(device.name)}</span>
        <div class="device-status">
          <span class="dot ${statusClass}"></span>
          <span>${statusText}</span>
        </div>
      </div>
      <div class="device-meta device-meta-row">
        <span title="Last heartbeat">Last beat: ${heartbeatText}</span>
        <span title="Heartbeat interval">Interval: ${device.heartbeat_interval_s || 30}s</span>
        ${missedCount > 0 ? `<span class="text-amber" title="Missed heartbeat windows">${missedCount} missed</span>` : ''}
      </div>
    </div>
  `;
}

const EFFORT_LABELS = {
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  xhigh: 'Extra High',
  max: 'Max',
};
function effortLabel(value) {
  return EFFORT_LABELS[value] || value;
}

function modelToFriendlyName(modelId) {
  if (!modelId) return 'Agent';
  const m = modelId.toLowerCase();
  if (m.includes('claude')) return 'Claude';
  if (m.includes('codex')) return 'Codex';
  if (m.includes('gpt')) return 'GPT';
  if (m.includes('gemini')) return 'Gemini';
  if (m.includes('llama')) return 'Llama';
  if (m.includes('mistral')) return 'Mistral';
  if (m.includes('command')) return 'Command';
  if (m.includes('deepseek')) return 'DeepSeek';
  // Fallback: capitalize first word
  const first = modelId.split(/[-_\/]/)[0];
  return first.charAt(0).toUpperCase() + first.slice(1);
}

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
    for (const id of devices.keys()) {
      if (!freshIds.has(id)) devices.delete(id);
    }
    for (const d of data) devices.set(d.id, d);
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
      const device = devices.get(deviceId);
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
      if (deviceDown && deviceDown.id === deviceId) {
        deviceDown = null;
        const banner = document.getElementById('device-down-banner');
        if (banner) banner.classList.remove('visible');
      }
      break;
    }

    case 'offline': {
      const device = devices.get(deviceId);
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
      const offlineConn = e2eeConnections.get(deviceId);
      if (offlineConn) {
        deviceDown = { id: deviceId, name: deviceName };
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
      const device = devices.get(deviceId);
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
      const dev = devices.get(deviceId);
      if (dev) {
        dev.has_transport_key = true;
        dev.status = 'online';
      }
      renderDeviceGrid();
      // Clear device-down banner if this device was down.
      if (deviceDown && deviceDown.id === deviceId) {
        deviceDown = null;
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
      const existingConn = e2eeConnections.get(deviceId);
      if (existingConn) {
        console.log('[E2EE] Device e2ee-ready while connected — relay restarted, reconnecting...');
        // Preserve current channel so it's restored after reconnect.
        if (chatCurrentChannel) _pendingChannelId = chatCurrentChannel;
        existingConn.disconnect();
        e2eeConnections.delete(deviceId);
      }
      console.log('[E2EE] Device e2ee-ready notification — connecting...');
      connectDeviceE2EE(deviceId);
      break;
    }

    case 'renamed': {
      const device = devices.get(deviceId);
      if (device) device.name = deviceName;
      renderChannelPanel();
      break;
    }

    case 'revoked': {
      devices.delete(deviceId);
      const revokedConn = e2eeConnections.get(deviceId);
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
    // No longer need selectedAgent — multi-device handles this.
    switchTab(tab);

    if (parts[1]) {
      _pendingChannelId = parts[1];
      // If channels are already loaded, select immediately.
      if (chatChannels.has(_pendingChannelId)) {
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
  if (evt.detail.status === 'connected' && e2eeConnections.size > 0 && !_sseReconnecting) {
    _sseReconnecting = true;
    console.log('[E2EE] SSE reconnected — tearing down stale sessions');
    try {
      // Channel state is cached — user stays on their current channel.
      for (const [, conn] of e2eeConnections) conn.disconnect();
      e2eeConnections.clear();
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

// =====================================================================
// Theme version checker — detect deploys via SSE reconnect
// =====================================================================
const THEME_VERSION_KEY = 'build_theme_version';

function compareVersions(a, b) {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] || 0;
    const nb = pb[i] || 0;
    if (na !== nb) return na - nb;
  }
  return 0;
}

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
let e2eeHasConnected = false;        // true after first successful E2EE connection
let _reconnectPillTimer = null;      // 5s timer to show reconnect pill
const e2eeConnections = new Map();   // deviceId -> BuildE2EE instance
const channelDeviceMap = new Map();  // channelId -> deviceId
const deviceChannels = new Map();    // deviceId -> Map(channelId -> channel)
const deviceHarnesses = new Map();   // deviceId -> harness list

function getE2EE(channelId) {
  const deviceId = channelDeviceMap.get(channelId);
  return deviceId ? e2eeConnections.get(deviceId) : null;
}
function getActiveE2EE() { return getE2EE(chatCurrentChannel); }
function anyE2EEConnected() {
  for (const conn of e2eeConnections.values()) if (conn.connected) return true;
  return false;
}
function rebuildChatChannels() {
  chatChannels.clear();
  for (const [, channels] of deviceChannels) {
    for (const [chId, ch] of channels) chatChannels.set(chId, ch);
  }
}

let chatCurrentChannel = null;
const chatChannels = new Map(); // id -> {id, name, created_at} — merged view
const chatMessages = new Map(); // channelId -> [messages]
const unreadCounts = new Map(); // channelId -> {messages: number, hasInteraction: boolean}
const channelLastSeen = new Map(); // channelId -> ISO timestamp (from device)
const channelSortTs = new Map(); // channelId -> epoch ms when channel was last promoted in sort order

const SORT_STABILITY_MS = 60 * 60 * 1000; // 1 hour
function promoteChannel(channelId) {
  const now = Date.now();
  const prev = channelSortTs.get(channelId) || 0;
  if (now - prev > SORT_STABILITY_MS) channelSortTs.set(channelId, now);
}

let scrollLastSeen = null; // captured lastSeen for scroll positioning during channel switch
let _unreadHighlightUntil = null; // timestamp: highlights persist until this time
let _unreadHighlightLastSeen = null; // the lastSeen value to use for highlighting

// ---- Channel Navigation (Electron only) ----
const channelHistory = [];
let channelHistoryIndex = -1;
let _navigatingHistory = false;

function getOrderedChannelIds() {
  const ids = [];
  const sortedDevices = [...devices.values()].sort((a, b) => a.name.localeCompare(b.name));
  for (const device of sortedDevices) {
    if (!e2eeConnections.has(device.id)) continue;
    const devChans = deviceChannels.get(device.id) || new Map();
    const sorted = [...devChans.values()].sort((a, b) => {
      const aTs = channelSortTs.get(a.id) || 0;
      const bTs = channelSortTs.get(b.id) || 0;
      if (aTs !== bTs) return bTs - aTs;
      return (b.created_at || 0) - (a.created_at || 0);
    });
    for (const ch of sorted) ids.push(ch.id);
  }
  return ids;
}

function pushChannelHistory(channelId) {
  if (_navigatingHistory) return;
  if (channelHistory[channelHistoryIndex] === channelId) return;
  channelHistory.splice(channelHistoryIndex + 1);
  channelHistory.push(channelId);
  if (channelHistory.length > 50) channelHistory.shift();
  channelHistoryIndex = channelHistory.length - 1;
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
      const cur = ids.indexOf(chatCurrentChannel);

      if (e.shiftKey) {
        // ALT+SHIFT+UP/DOWN: jump to first unread in that direction
        const dir = e.key === 'ArrowUp' ? -1 : 1;
        const start = cur === -1 ? 0 : cur + dir;
        for (let i = start; i >= 0 && i < ids.length; i += dir) {
          const uc = unreadCounts.get(ids[i]);
          if (uc && uc.messages > 0) { selectChannel(ids[i]); return; }
        }
      } else {
        // ALT+UP/DOWN: move to adjacent channel
        const next = e.key === 'ArrowUp' ? cur - 1 : cur + 1;
        if (next >= 0 && next < ids.length) selectChannel(ids[next]);
      }
    } else if (e.key === '[' || e.key === ']') {
      e.preventDefault();
      if (e.key === '[' && channelHistoryIndex > 0) {
        channelHistoryIndex--;
        _navigatingHistory = true;
        selectChannel(channelHistory[channelHistoryIndex]);
        _navigatingHistory = false;
      } else if (e.key === ']' && channelHistoryIndex < channelHistory.length - 1) {
        channelHistoryIndex++;
        _navigatingHistory = true;
        selectChannel(channelHistory[channelHistoryIndex]);
        _navigatingHistory = false;
      }
    }
  });
}

// ---- Browser Tabs ----
const browserTabs = new Map(); // deviceId -> [{id, url, deviceId}]
let activeBrowserTab = null; // tab id
let _browserTabCounter = 0;

function addBrowserTab(deviceId) {
  const id = 'browser-' + (++_browserTabCounter);
  const tab = { id, url: 'http://localhost:', deviceId };
  if (!browserTabs.has(deviceId)) browserTabs.set(deviceId, []);
  browserTabs.get(deviceId).push(tab);
  activeBrowserTab = id;
  chatCurrentChannel = null;
  renderChannelPanel();
  showBrowserView(tab);
}

function removeBrowserTab(deviceId, tabId) {
  const tabs = browserTabs.get(deviceId) || [];
  const idx = tabs.findIndex(t => t.id === tabId);
  if (idx !== -1) tabs.splice(idx, 1);
  if (activeBrowserTab === tabId) {
    activeBrowserTab = null;
    // Switch to first channel
    const firstCh = chatChannels.keys().next().value;
    if (firstCh) selectChannel(firstCh);
    else { document.getElementById('viewer-content')?.replaceChildren(); }
  }
  renderChannelPanel();
}

function selectBrowserTab(tab) {
  activeBrowserTab = tab.id;
  chatCurrentChannel = null;
  renderChannelPanel();
  showBrowserView(tab);
}

function createBrowserTabItem(tab, deviceId) {
  const item = document.createElement('div');
  item.className = 'browser-tab-item' + (activeBrowserTab === tab.id ? ' active' : '');
  const displayUrl = tab.url || 'New tab';
  item.innerHTML = `
    <svg viewBox="0 0 12 12" fill="none"><circle cx="6" cy="6" r="4.5" stroke="currentColor" stroke-width="1.2"/><path d="M1.5 6h9" stroke="currentColor" stroke-width="1"/></svg>
    <span class="tab-url">${escapeHtml(displayUrl)}</span>
    <button class="browser-tab-close" title="Close tab"><svg viewBox="0 0 8 8" fill="none"><path d="M1 1l6 6M7 1l-6 6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg></button>
  `;
  item.addEventListener('click', (e) => {
    if (e.target.closest('.browser-tab-close')) {
      e.stopPropagation();
      removeBrowserTab(deviceId, tab.id);
      return;
    }
    selectBrowserTab(tab);
  });
  return item;
}

function getLastSeen(channelId) {
  return channelLastSeen.get(channelId) || null;
}
function setLastSeen(channelId) {
  const now = new Date().toISOString();
  channelLastSeen.set(channelId, now);
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
  if (!_unreadHighlightLastSeen) {
    _unreadHighlightLastSeen = scrollLastSeen || getLastSeen(chatCurrentChannel);
  }
  for (const fn of ops) fn();
  // Keep unread highlight visible for 60s after being marked as read.
  _unreadHighlightUntil = Date.now() + 60000;
  setTimeout(() => {
    if (Date.now() >= _unreadHighlightUntil) {
      _unreadHighlightLastSeen = null;
      _unreadHighlightUntil = null;
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
  const lastSeen = _unreadHighlightLastSeen || scrollLastSeen || getLastSeen(chatCurrentChannel);
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
const channelAgentActive = new Map(); // channelId -> boolean

function updateStopButton() {
  const btn = document.getElementById('chat-stop-btn');
  if (!btn) return;
  const active = chatCurrentChannel && channelAgentActive.get(chatCurrentChannel);
  btn.classList.toggle('visible', !!active);
}

const chatLoadingMessages = new Set(); // channels currently loading messages
const chatLoadingActivity = new Set(); // channels currently loading activity
const channelPlanMode = new Map(); // channelId -> boolean
// cachedHarnesses per-device stored in deviceHarnesses Map

// ----- Per-channel localStorage persistence -----
const CHANNEL_STATE_KEY = 'build_channel_state';

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
    const count = [...e2eeConnections.values()].filter(c => c.connected).length;
    dot.className = 'e2ee-dot connected';
    label.textContent = count > 1 ? `E2EE active (${count} devices)` : 'E2EE active';
    document.getElementById('e2ee-waiting-overlay').classList.add('hidden');
  } else if (e2eeConnections.size > 0) {
    dot.className = 'e2ee-dot connecting';
    label.textContent = 'Connecting...';
  } else {
    dot.className = 'e2ee-dot connecting';
    label.textContent = 'Waiting for device...';
  }
}

async function connectDeviceE2EE(deviceId) {
  if (e2eeConnections.has(deviceId)) return;
  console.log('[E2EE] Connecting to device', deviceId);

  const instance = new BuildE2EE();
  try {
    await instance.ready();
  } catch (err) {
    console.error('[E2EE] libsodium not available:', err);
    return;
  }

  bindE2EEEvents(instance, deviceId);
  e2eeConnections.set(deviceId, instance);
  syncE2EEStatus();

  try {
    await instance.connect(deviceId);
    console.log('[E2EE] Connected to', deviceId);
  } catch (err) {
    console.error('[E2EE] Failed to connect to', deviceId, err);
    instance.disconnect(); // Clean up leaked notification handler on document.
    e2eeConnections.delete(deviceId);
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

  // Connect to all online devices with transport keys in parallel.
  const candidates = [...devices.values()].filter(d => d.status === 'online' && d.has_transport_key);
  if (candidates.length === 0) {
    console.log('[E2EE] No device ready, waiting for e2ee-ready notification');
    syncE2EEStatus();
    const skelText = document.querySelector('.skel-status-text');
    if (skelText) skelText.textContent = 'Waiting for device…';
    // Retry: device may not have sent e2ee-ready yet after SSE reconnect.
    clearTimeout(initE2EE._retryTimer);
    initE2EE._retryTimer = setTimeout(async () => {
      if (e2eeConnections.size > 0) return; // Already connected via e2ee-ready
      console.log('[E2EE] Retrying — refreshing devices...');
      await fetchDevices();
      if (e2eeConnections.size === 0) initE2EE();
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
    e2eeHasConnected = true;
    const _enableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
    for (const id of _enableBtns) {
      const el = document.getElementById(id);
      if (el) el.disabled = false;
    }
    document.getElementById('e2ee-waiting-overlay').classList.add('hidden');
    // Hide reconnect pill + cancel timer.
    clearTimeout(_reconnectPillTimer);
    _reconnectPillTimer = null;
    document.getElementById('reconnect-pill').classList.remove('visible');
    // Clear device-down banner on successful reconnect.
    deviceDown = null;
    const _ddb = document.getElementById('device-down-banner');
    if (_ddb) _ddb.classList.remove('visible');
    if (typeof _bindUploadProgress === 'function') _bindUploadProgress(instance);
    syncE2EEStatus();
    renderChannelPanel();
    instance.listChannels();
    instance.listHarnesses();
    // Re-fetch history for the current channel if it belongs to this device.
    if (chatCurrentChannel && channelDeviceMap.get(chatCurrentChannel) === deviceId) {
      chatLoadingMessages.add(chatCurrentChannel);
      chatLoadingActivity.add(chatCurrentChannel);
      instance.getMessages(chatCurrentChannel);
      instance.getActivity(chatCurrentChannel);
      // If the files tab is active, retry the tree/changes fetch — the initial
      // selectChannel() may have run before this `connected` flag flipped.
      if (currentTab === 'files') {
        filesChannelId = null;
        if (typeof onFilesTabActivated === 'function') onFilesTabActivated();
      }
    }
  });

  instance.addEventListener('disconnected', () => {
    console.log('[E2EE] Disconnected device', deviceId);
    e2eeConnections.delete(deviceId);
    // Keep deviceChannels and channelDeviceMap cached — only clear on server-reported removal.
    deviceHarnesses.delete(deviceId);
    deviceAgentCwd.delete(deviceId);

    if (!anyE2EEConnected()) {
      const _disableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
      for (const id of _disableBtns) {
        const el = document.getElementById(id);
        if (el) el.disabled = true;
      }
      if (deviceDown) {
        // Device is known to be offline — keep banner visible, skip skeleton/pill.
      } else if (!e2eeHasConnected) {
        // First load — show skeleton.
        document.getElementById('e2ee-waiting-overlay').classList.remove('hidden');
      } else {
        // Reconnection — show pill after 2s delay.
        clearTimeout(_reconnectPillTimer);
        _reconnectPillTimer = setTimeout(() => {
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
    if (agent_cwd) deviceAgentCwd.set(deviceId, agent_cwd);
    // Diff against cached channels to detect removals.
    const oldChans = deviceChannels.get(deviceId);
    const oldIds = oldChans ? new Set(oldChans.keys()) : new Set();
    // Update per-device channel map.
    const devChans = new Map();
    for (const ch of channels) {
      devChans.set(ch.id, ch);
      channelDeviceMap.set(ch.id, deviceId);
      if (ch.plan_mode != null) channelPlanMode.set(ch.id, ch.plan_mode);
      if (ch.last_seen_at) channelLastSeen.set(ch.id, ch.last_seen_at);
      // Reset agent active state — real-time events will re-set if agent is mid-turn.
      channelAgentActive.set(ch.id, false);
    }
    updateStopButton();
    deviceChannels.set(deviceId, devChans);
    // Clean up channels removed by the device.
    for (const oldId of oldIds) {
      if (!devChans.has(oldId)) {
        channelDeviceMap.delete(oldId);
        chatMessages.delete(oldId);
        unreadCounts.delete(oldId);
        channelSortTs.delete(oldId);
      }
    }
    rebuildChatChannels();
    renderChannelList();
    // If current channel was removed by the device, auto-select another.
    if (chatCurrentChannel && oldIds.has(chatCurrentChannel) && !devChans.has(chatCurrentChannel)) {
      const remaining = [...chatChannels.values()];
      if (remaining.length > 0) selectChannel(remaining[0].id);
      else { chatCurrentChannel = null; renderMessages(); }
    }
    // Restore pending channel from hash.
    if (_pendingChannelId && chatChannels.has(_pendingChannelId)) {
      selectChannel(_pendingChannelId);
      _pendingChannelId = null;
    } else if (!chatCurrentChannel && !_pendingChannelId && channels.length > 0) {
      // Only auto-select first channel if there's no pending channel waiting for another device.
      selectChannel(channels[0].id);
    }
    // Fetch messages for all non-current channels to compute unread counts.
    for (const ch of channels) {
      if (ch.id !== chatCurrentChannel && instance.connected) {
        instance.getMessages(ch.id);
      }
    }
  });

  instance.addEventListener('channel_created', (evt) => {
    const ch = evt.detail;
    channelDeviceMap.set(ch.id, deviceId);
    const devChans = deviceChannels.get(deviceId) || new Map();
    devChans.set(ch.id, ch);
    deviceChannels.set(deviceId, devChans);
    rebuildChatChannels();
    renderChannelList();
    selectChannel(ch.id);
  });

  // ----- Harness & Agent events -----

  instance.addEventListener('harness_list', (evt) => {
    deviceHarnesses.set(deviceId, evt.detail);
    console.log('[E2EE] Harnesses for', deviceId, ':', evt.detail.map(h => h.name));
  });

  instance.addEventListener('agent_started', (evt) => {
    console.log('[E2EE] Agent started:', evt.detail);
  });

  instance.addEventListener('agent_stopped', (evt) => {
    console.log('[E2EE] Agent stopped:', evt.detail);
    const ch = evt.detail?.channel_id;
    if (ch) {
      channelAgentActive.set(ch, false);
      updateStopButton();
    }
  });

  instance.addEventListener('agent_restarted', (evt) => {
    console.log('[E2EE] Agent restarted:', evt.detail);
  });

  instance.addEventListener('channel_renamed', (evt) => {
    const { channel_id, name } = evt.detail;
    const ch = chatChannels.get(channel_id);
    if (ch) {
      ch.name = name;
      const devChans = deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) devChans.get(channel_id).name = name;
      renderChannelList();
      renderChannelSidebar();
    }
  });

  instance.addEventListener('channel_updated', (evt) => {
    const { channel_id, model, effort, working_directory } = evt.detail;
    const ch = chatChannels.get(channel_id);
    if (ch) {
      if (model) ch.model = model;
      if (effort !== undefined) ch.effort = effort;
      if (working_directory !== undefined) ch.working_directory = working_directory;
      const devChans = deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) {
        const dc = devChans.get(channel_id);
        if (model) dc.model = model;
        if (effort !== undefined) dc.effort = effort;
        if (working_directory !== undefined) dc.working_directory = working_directory;
      }
    }
    // If this is the active channel, refresh the chat overlay model/effort pills.
    if (chatCurrentChannel === channel_id) applyChannelHarnessInfo(channel_id);
  });

  instance.addEventListener('channel_deleted', (evt) => {
    const { channel_id } = evt.detail;
    channelDeviceMap.delete(channel_id);
    const devChans = deviceChannels.get(deviceId);
    if (devChans) devChans.delete(channel_id);
    rebuildChatChannels();
    unreadCounts.delete(channel_id);
    if (chatCurrentChannel === channel_id) {
      const remaining = [...chatChannels.values()];
      if (remaining.length > 0) {
        selectChannel(remaining[0].id);
      } else {
        chatCurrentChannel = null;
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
    chatLoadingMessages.delete(channel_id);
    chatMessages.set(channel_id, messages);
    // Seed sort timestamp from newest message on initial load (don't override live promotions).
    if (!channelSortTs.has(channel_id) && messages.length) {
      const newest = messages[messages.length - 1];
      const ts = typeof newest.created_at === 'number' ? newest.created_at * 1000 : Date.parse(newest.created_at);
      if (ts) channelSortTs.set(channel_id, ts);
    }
    if (chatCurrentChannel === channel_id) {
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
        unreadCounts.set(channel_id, { messages: unreadMsgs.length, hasInteraction });
      } else {
        unreadCounts.delete(channel_id);
      }
      renderChannelList();
    }
  });

  instance.addEventListener('message', (evt) => {
    const msg = evt.detail;
    if (!msg || !msg.channel_id) return;
    const msgs = chatMessages.get(msg.channel_id) || [];
    // Dedup: skip if message with same ID already exists.
    if (msg.id && msgs.some(m => m.id === msg.id)) return;
    msgs.push(msg);
    chatMessages.set(msg.channel_id, msgs);
    promoteChannel(msg.channel_id);
    if (chatCurrentChannel === msg.channel_id) {
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
      if (!channelAgentActive.get(channel_id)) {
        channelAgentActive.set(channel_id, true);
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
      const msgs = chatMessages.get(channel_id) || [];
      if (msg.id && msgs.some(m => m.id === msg.id)) return;
      msgs.push(msg);
      chatMessages.set(channel_id, msgs);
      if (chatCurrentChannel === channel_id) {
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
      if (!channelAgentActive.get(channel_id)) {
        channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      if (chatCurrentChannel !== channel_id) return;
      const delta = agentEvt.delta || {};
      if (delta.type === 'text' && delta.text) {
        appendConsoleReasoning(delta.text, agentEvt.created_at || null);
      }
    } else if (event_type === 'tool.use') {
      if (!channelAgentActive.get(channel_id)) {
        channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      // Capture TodoWrite for any channel (before early return)
      const name = agentEvt.name || 'tool';
      const input = agentEvt.input || {};
      if (name === 'TodoWrite' && input.todos) {
        channelTodos.set(channel_id, input.todos);
        if (chatCurrentChannel === channel_id) {
          renderTasksPanel(channel_id);
          updateTasksBadge(channel_id);
        }
      }
      if (chatCurrentChannel !== channel_id) return;
      currentReasoningEntry = null;
      const desc = describeToolUse(name, input);
      appendConsoleEntry(agentEvt.tool_use_id, name, desc, input, agentEvt.created_at);
    } else if (event_type === 'tool.result') {
      if (chatCurrentChannel !== channel_id) return;
      markConsoleEntryDone(agentEvt.tool_use_id, agentEvt.is_error, agentEvt.content, agentEvt.completed_at);
    } else if (event_type === 'activity.end') {
      channelAgentActive.set(channel_id, false);
      updateStopButton();
      if (chatCurrentChannel !== channel_id) return;
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
      const msgs = chatMessages.get(channel_id) || [];
      msgs.push(msg);
      chatMessages.set(channel_id, msgs);
      if (chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(msg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
        // On mobile, collapse the console to give more room for plan review cards,
        // then scroll the plan card to the top of the chat area.
        if ((agentEvt.kind || 'question') === 'plan_review' && window.innerWidth <= 768 && consoleState !== 'collapsed') {
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
      const msgs = chatMessages.get(channel_id) || [];
      msgs.push(errMsg);
      chatMessages.set(channel_id, msgs);
      if (chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(errMsg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
      } else {
        incrementUnread(channel_id, true);
      }
      if (agentEvt.fatal) {
        channelAgentActive.set(channel_id, false);
        updateStopButton();
      }
    } else if (event_type === 'agent.state_update') {
      const planMode = agentEvt.plan_mode;
      if (planMode != null) {
        channelPlanMode.set(channel_id, planMode);
        if (channel_id === chatCurrentChannel) updatePlanModeUI(planMode);
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
    chatLoadingActivity.delete(channel_id);
    if (chatCurrentChannel !== channel_id) return;

    clearConsole();
    for (const entry of entries) {
      if (entry.type === 'tool_use') {
        const d = entry.data || {};
        const name = d.name || 'tool';
        const input = d.input || {};
        // Capture TodoWrite from history (last one wins)
        if (name === 'TodoWrite' && input.todos) {
          channelTodos.set(channel_id, input.todos);
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
    if (channelTodos.has(channel_id)) {
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
    channelPlanMode.set(channel_id, plan_mode);
    if (channel_id === chatCurrentChannel) updatePlanModeUI(plan_mode);
  });

  instance.addEventListener('system_message', (evt) => {
    const { channel_id, text } = evt.detail;
    if (chatCurrentChannel === channel_id) {
      appendSystemMessage(text);
      scrollChatToBottom();
    }
  });

  instance.addEventListener('session_reset', (evt) => {
    const { channel_id } = evt.detail;
    if (chatCurrentChannel === channel_id) {
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
    if (chatCurrentChannel === channel_id) {
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
    if (!complicationState.has(channelId)) complicationState.set(channelId, new Map());
    complicationState.get(channelId).set(comp.id, comp);
    if (chatCurrentChannel === channelId) renderComplications();
  });

  instance.addEventListener('complication_remove', (evt) => {
    const { channel_id, id } = evt.detail;
    const channelComps = complicationState.get(channel_id);
    if (channelComps) {
      channelComps.delete(id);
      if (chatCurrentChannel === channel_id) renderComplications();
    }
  });

  instance.addEventListener('complications', (evt) => {
    const { channel_id, complications } = evt.detail;
    if (!channel_id || !complications) return;
    if (!complicationState.has(channel_id)) complicationState.set(channel_id, new Map());
    const channelComps = complicationState.get(channel_id);
    for (const comp of complications) {
      channelComps.set(comp.id, comp);
    }
    if (chatCurrentChannel === channel_id) renderComplications();
  });

  // ----- Terminal Output -----
  instance.addEventListener('terminal_output', (evt) => {
    const { channel_id, data, done, exit_code, cwd } = evt.detail;
    if (!channel_id) return;

    const sentinel = '__BUILD_CWD__';

    if (!done && data) {
      // First output received — clear loading animation and reset no-output timer.
      if (terminalCurrentBlock && !terminalCurrentBlock.hasOutput) {
        terminalCurrentBlock.hasOutput = true;
        clearTerminalTimers();
      }

      // Filter out sentinel line from streaming output.
      let text = data;
      if (text.includes(sentinel)) {
        text = text.split('\n').filter(l => !l.startsWith(sentinel)).join('\n');
        if (!text) return;
      }

      // Append to current streaming block if matching channel.
      if (terminalCurrentBlock && terminalCurrentBlock.channelId === channel_id) {
        terminalCurrentBlock.text += text;
        const span = document.createElement('span');
        span.textContent = text;
        terminalCurrentBlock.outputDiv.appendChild(span);
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
      }
      // Update stored history.
      const history = terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].output += text;
    }

    if (done) {
      clearTerminalTimers();
      document.getElementById('terminal-kill-btn')?.classList.add('hidden');
      // Update cwd.
      if (cwd) {
        terminalCwdMap.set(channel_id, cwd);
        if (chatCurrentChannel === channel_id) renderTerminalCwd();
      }
      // Update stored exit code.
      const history = terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].exitCode = exit_code;
      // Show exit code if non-zero.
      if (terminalCurrentBlock && terminalCurrentBlock.channelId === channel_id && exit_code !== 0) {
        const exitDiv = document.createElement('div');
        exitDiv.className = 'terminal-cmd-exit error';
        exitDiv.textContent = `exit ${exit_code}`;
        terminalCurrentBlock.block.appendChild(exitDiv);
      }
      if (terminalCurrentBlock?.channelId === channel_id) {
        terminalCurrentBlock = null;
      }
      terminalRunning = false;
      // Show prompt row again with updated cwd.
      if (chatCurrentChannel === channel_id) {
        const promptRow = document.getElementById('terminal-prompt-row');
        promptRow.classList.remove('hidden');
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
        document.getElementById('terminal-input')?.focus();
      }
    }
  });

  instance.addEventListener('terminal_completions', (evt) => {
    terminalCompletionPending = false;
    const { completions } = evt.detail;
    if (!completions || !completions.length) return;
    const input = document.getElementById('terminal-input');
    if (!input) return;
    // Use the context saved at request time (not the echoed partial)
    const beforePartial = terminalCompletionBase;
    const partial = terminalCompletionPartial;

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
      terminalCompletions = completions;
      terminalCompletionIndex = -1;
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
    if (channel_id !== filesChannelId) return;

    if (!fileTreeData.has(channel_id)) fileTreeData.set(channel_id, new Map());
    const data = fileTreeData.get(channel_id);
    data.set(path || '', { entries: entries || [], truncated: !!truncated });
    renderFileTree();

    // Restore saved file selection after root listing loads.
    if (filesPendingRestore && (path || '') === '') {
      const pendingPath = filesPendingRestore;
      const pendingView = filesPendingView;
      filesPendingRestore = null;
      filesPendingView = 'source';
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
    console.debug('[files] files_changes_result', { channel_id, repos_count: repos?.length, filesChannelId, chatCurrentChannel });
    // Accept if it matches filesChannelId, OR if filesChannelId is unset
    // but this is the currently-active chat channel (self-heal race).
    if (filesChannelId) {
      if (channel_id !== filesChannelId) return;
    } else if (channel_id !== chatCurrentChannel) {
      return;
    } else {
      filesChannelId = channel_id;
    }
    filesChangesData.set(channel_id, repos || []);
    updateFilesModifiedCount();
    if (filesTreeTab === 'changes') renderFileTree();
  });

  let _imageChunks = {};  // path -> { chunks: [], total: N }

  instance.addEventListener('file_read_result', (evt) => {
    const d = evt.detail;
    if (d.channel_id !== filesChannelId || d.path !== filesCurrentPath) return;
    if (filesCurrentView === 'diff') return;

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
    if (d.channel_id !== filesChannelId || d.path !== filesCurrentPath) return;
    if (filesCurrentView !== 'diff') return;

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
  const uc = unreadCounts.get(channelId) || { messages: 0, hasInteraction: false };
  uc.messages++;
  if (isInteraction) uc.hasInteraction = true;
  unreadCounts.set(channelId, uc);
  renderChannelList();
}

function updateAggregateBadge() { /* no-op, removed with top bar */ }

function selectChannel(channelId) {
  chatCurrentChannel = channelId;
  pushChannelHistory(channelId);
  activeBrowserTab = null;
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
  scrollLastSeen = getLastSeen(channelId) || null;
  _unreadHighlightLastSeen = null;
  _unreadHighlightUntil = null;
  // Defer marking as seen until user interacts.
  const _chId = channelId;
  deferMarkRead(() => {
    setLastSeen(_chId);
    unreadCounts.delete(_chId);
    renderChannelList();
  });
  renderChannelList();
  const _selConn = getE2EE(channelId);
  if (_selConn && _selConn.connected) {
    chatLoadingMessages.add(channelId);
    chatLoadingActivity.add(channelId);
    _selConn.getMessages(channelId);
    _selConn.getActivity(channelId);
    _selConn.getComplications(channelId);
  } else if (!chatMessages.has(channelId) || chatMessages.get(channelId).length === 0) {
    // Device disconnected, no cached messages — show loading state until reconnect.
    chatLoadingMessages.add(channelId);
  }
  renderMessages();
  clearConsole(chatLoadingActivity.has(channelId));
  const chName = chatChannels.get(channelId)?.name || '';
  const input = document.getElementById('chat-input');
  input.placeholder = `Message #${chName}...`;
  // Update mobile channel label
  updateMobileChannelLabel();
  location.hash = `${currentTab || 'chat'}/${channelId}`;
  // If files tab is active, refresh it for the new channel.
  if (currentTab === 'files') onFilesTabActivated();
  // Restore persisted state from localStorage.
  const saved = loadChannelState(channelId);
  // Restore last active tab for this channel.
  if (saved.activeTab && saved.activeTab !== currentTab) {
    _origSwitchTab(saved.activeTab);
    if (saved.activeTab === 'files') onFilesTabActivated();
    location.hash = `${saved.activeTab}/${channelId}`;
  }
  if (input) {
    input.value = saved.draft || '';
    input.style.height = 'auto';
  }
  // Plan mode: prefer server state, fall back to localStorage.
  if (!channelPlanMode.has(channelId) && saved.planMode !== undefined) {
    channelPlanMode.set(channelId, saved.planMode);
  }
  // Sync plan mode toggle.
  updatePlanModeUI(channelPlanMode.get(channelId) || false);
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

  if (!chatCurrentChannel) {
    container.innerHTML = '';
    container.appendChild(createEmptyState('Select a channel', 'Choose or create a channel to start chatting.'));
    return;
  }

  const msgs = chatMessages.get(chatCurrentChannel) || [];

  if (msgs.length === 0) {
    container.innerHTML = '';
    const ch = chatChannels.get(chatCurrentChannel);
    if (chatLoadingMessages.has(chatCurrentChannel)) {
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
  const channelModel = chatChannels.get(chatCurrentChannel)?.model || '';
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
  if (!_conn || !_conn.connected || !chatCurrentChannel) return;
  _conn.sendInteractionResponse(chatCurrentChannel, interactionId, selectedOption, freeformResponse);
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
  const uc = unreadCounts.get(chatCurrentChannel);
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
  if (!_conn || !_conn.connected || !chatCurrentChannel) return;
  _conn.sendInteractionResponse(chatCurrentChannel, interactionId, null, freeformResponse, selectedOptions);
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
  const uc = unreadCounts.get(chatCurrentChannel);
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

function agentShortName(sender) {
  const map = {'Claude Code': 'Claude', 'Codex CLI': 'Codex', 'Gemini CLI': 'Gemini'};
  return map[sender] || sender || 'Device';
}

function describeToolUse(name, input) {
  // Prefer explicit description field (e.g. Bash has it)
  if (input.description) return input.description;
  const descs = {
    Read: () => (input.file_path || '').split('/').pop() || 'file',
    Edit: () => (input.file_path || '').split('/').pop() || 'file',
    Write: () => (input.file_path || '').split('/').pop() || 'file',
    Bash: () => (input.command || '').substring(0, 80),
    Glob: () => input.pattern || '',
    Grep: () => `'${input.pattern || ''}'`,
    Agent: () => input.prompt ? input.prompt.substring(0, 80) : '',
    WebFetch: () => input.url || 'web page',
    WebSearch: () => input.query || '',
    ToolSearch: () => input.query || '',
  };
  const fn = descs[name];
  return fn ? fn() : name;
}

function formatToolDetail(name, input) {
  const parts = [];
  if (name === 'Bash' && input.command) {
    parts.push(`<div class="ce-detail-label">Command</div><pre class="ce-detail-code">${escapeHtml(input.command)}</pre>`);
  } else if (name === 'Edit' && input.file_path) {
    parts.push(`<div class="ce-detail-label">File</div><div class="ce-detail-val">${escapeHtml(input.file_path)}</div>`);
    if (input.old_string != null && input.new_string != null) {
      parts.push(`<div class="ce-detail-label">Diff</div><pre class="ce-detail-diff"><span class="ce-diff-del">${escapeHtml(input.old_string)}</span><span class="ce-diff-add">${escapeHtml(input.new_string)}</span></pre>`);
    }
  } else if ((name === 'Read' || name === 'Write') && input.file_path) {
    parts.push(`<div class="ce-detail-label">File</div><div class="ce-detail-val">${escapeHtml(input.file_path)}</div>`);
  } else if (name === 'Grep') {
    parts.push(`<div class="ce-detail-label">Pattern</div><div class="ce-detail-val">${escapeHtml(input.pattern || '')}</div>`);
    if (input.path) parts.push(`<div class="ce-detail-label">Path</div><div class="ce-detail-val">${escapeHtml(input.path)}</div>`);
  } else if (name === 'Glob') {
    parts.push(`<div class="ce-detail-label">Pattern</div><div class="ce-detail-val">${escapeHtml(input.pattern || '')}</div>`);
  } else if (name === 'Agent') {
    if (input.prompt) parts.push(`<div class="ce-detail-label">Prompt</div><pre class="ce-detail-code">${escapeHtml(input.prompt)}</pre>`);
  }
  return parts.join('') || `<pre class="ce-detail-code">${escapeHtml(JSON.stringify(input || {}, null, 2))}</pre>`;
}

function formatToolResult(name, content, isError) {
  if (!content) return '';
  const text = typeof content === 'string' ? content : JSON.stringify(content, null, 2);
  if (isError) {
    return `<div class="ce-detail-label ce-error-label">Error</div><pre class="ce-detail-code ce-error">${escapeHtml(text)}</pre>`;
  }
  // For Edit tools, result is usually short confirmation
  if (name === 'Edit') {
    return `<div class="ce-detail-label">Result</div><div class="ce-detail-val">${escapeHtml(text.substring(0, 200))}</div>`;
  }
  // For Bash, show output
  if (name === 'Bash') {
    return `<div class="ce-detail-label">Output</div><pre class="ce-detail-code">${escapeHtml(text)}</pre>`;
  }
  // For Read, show file content
  if (name === 'Read') {
    return `<div class="ce-detail-label">Content</div><pre class="ce-detail-code">${escapeHtml(text.length > 2000 ? text.substring(0, 2000) + '\n…truncated' : text)}</pre>`;
  }
  // Default
  if (text.length > 500) {
    return `<div class="ce-detail-label">Result</div><pre class="ce-detail-code">${escapeHtml(text.substring(0, 500) + '\n…truncated')}</pre>`;
  }
  return `<div class="ce-detail-label">Result</div><pre class="ce-detail-code">${escapeHtml(text)}</pre>`;
}

function toolTag(name) {
  const map = { Read: 'read', Edit: 'edit', Write: 'write', Bash: 'bash', Grep: 'read', Glob: 'read', Agent: 'bash', ToolSearch: 'read' };
  return map[name] || 'read';
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
  if (!chatCurrentChannel) return false;
  // Use captured lastSeen from channel switch, falling back to live value.
  const lastSeen = scrollLastSeen || getLastSeen(chatCurrentChannel);
  if (!lastSeen) return false;
  const msgEls = container.querySelectorAll('.msg[data-created-at]');
  for (const el of msgEls) {
    if (el.dataset.createdAt > lastSeen) {
      scrollToMessage(el, 'instant', 'top');
      scrollLastSeen = null; // consumed
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
  const hasFiles = pendingFiles.length > 0;
  const _sendConn = getActiveE2EE();
  if ((!content && !hasFiles) || !chatCurrentChannel || !_sendConn || !_sendConn.connected) return;

  // Capture channel at send time so async uploads don't target the wrong channel.
  const channelId = chatCurrentChannel;

  input.value = '';
  input.style.height = 'auto';
  clearChannelDraft(channelId);

  // Upload any staged files first.
  let attachments = null;
  if (hasFiles) {
    const files = [...pendingFiles];
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
  const msgs = chatMessages.get(channelId) || [];
  msgs.push(tempMsg);
  chatMessages.set(channelId, msgs);
  appendMessage(tempMsg);
  const _sentEl = document.getElementById('chat-messages').lastElementChild;
  if (_sentEl) handleSentMessageScroll(_sentEl);

  // Auto-resolve any pending plan review cards (agent cancels interactions on new messages).
  resolvePendingPlanReviews();

  try {
    // Send via E2EE with attachment metadata, plan mode, model, and effort.
    const payload = { action: 'message', channel_id: channelId, content: messageContent };
    if (attachments) payload.attachments = attachments;
    if (channelPlanMode.get(channelId)) payload.plan_mode = true;
    const _chData = chatChannels.get(channelId);
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
  if (!chatCurrentChannel || !_stopConn?.connected) return;

  // Device handles two-phase stop: graceful cancel → 3s → process kill.
  _stopConn.stopAgent(chatCurrentChannel).catch(() => {});
  channelAgentActive.set(chatCurrentChannel, false);
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
  if (!chatCurrentChannel) return;
  saveChannelState(chatCurrentChannel, { draft: document.getElementById('chat-input').value });
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
  if (!chatCurrentChannel) return;
  const current = channelPlanMode.get(chatCurrentChannel) || false;
  channelPlanMode.set(chatCurrentChannel, !current);
  saveChannelState(chatCurrentChannel, { planMode: !current });
  updatePlanModeUI(!current);
});

// Compact button (in tray) - double-click confirm pattern
let compactConfirmTimeout = null;
document.getElementById('cmd-compact-btn')?.addEventListener('click', () => {
  const btn = document.getElementById('cmd-compact-btn');
  if (!chatCurrentChannel) return;
  if (btn.classList.contains('confirm')) {
    clearTimeout(compactConfirmTimeout);
    btn.classList.remove('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Compact';
    getActiveE2EE()?.compactSession(chatCurrentChannel);
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
  if (!chatCurrentChannel) return;
  if (btn.classList.contains('confirm')) {
    // Second click - do the reset
    clearTimeout(resetConfirmTimeout);
    btn.classList.remove('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Clear';
    getActiveE2EE()?.resetSession(chatCurrentChannel);
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

const pendingFiles = [];
const MAX_FILE_SIZE = 50 * 1024 * 1024; // 50 MB

function addPendingFiles(files) {
  for (const file of files) {
    if (file.size > MAX_FILE_SIZE) {
      console.warn(`File too large: ${file.name} (${formatFileSize(file.size)})`);
      continue;
    }
    pendingFiles.push(file);
  }
  renderPendingFiles();
}

function removePendingFile(index) {
  pendingFiles.splice(index, 1);
  renderPendingFiles();
}

function clearPendingFiles() {
  pendingFiles.length = 0;
  renderPendingFiles();
}

function renderPendingFiles() {
  const staging = document.getElementById('upload-staging');
  if (!staging) return;
  if (!pendingFiles.length) {
    staging.innerHTML = '';
    staging.classList.remove('has-files');
    return;
  }
  staging.classList.add('has-files');
  staging.innerHTML = pendingFiles.map((f, i) =>
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
  const _targetDeviceId = channelDeviceMap.get(chatCurrentChannel) || e2eeConnections.keys().next().value;
  const _createConn = e2eeConnections.get(_targetDeviceId);

  const dialogParent = document.body;
  const overlay = document.createElement('div');
  overlay.className = 'new-channel-overlay';

  // Build harness options from device's cache.
  const cachedHarnesses = deviceHarnesses.get(_targetDeviceId) || [];
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
  const deviceId = channelDeviceMap.get(ch.id);
  const cachedHarnesses = deviceId ? deviceHarnesses.get(deviceId) : [];
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
        devices.delete(device.id);
        // Disconnect E2EE for this device.
        const _revokeConn = e2eeConnections.get(device.id);
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


// ===== FILES VIEW =====

const fileTree = document.getElementById('file-tree');
const fileTreeInner = document.getElementById('file-tree-inner');
const fileTreeEmpty = document.getElementById('file-tree-empty');
const filesPathText = document.getElementById('files-path-text');
const filesPathBar = document.getElementById('files-path-bar');
const filesPathChevron = document.getElementById('files-path-chevron');
const fileReloadBtn = document.getElementById('file-reload-btn');
const fileTreePanel = document.getElementById('file-tree-panel');
const fileContentBody = document.getElementById('file-content-body');
const fileFloatToggle = document.getElementById('file-float-toggle');

// Per-channel file state.
const fileTreeData = new Map();     // channelId -> Map(path -> {entries, expanded})
let filesCurrentPath = null;        // currently selected file path
let filesCurrentView = 'source';    // 'source' or 'diff' or 'rendered'
let filesChannelId = null;          // channel the file tree is loaded for
let filesCurrentHasDiff = false;
let filesCurrentIsMarkdown = false;
let filesCurrentIsSvg = false;
let filesCurrentIsHtml = false;
let filesLastContent = null;        // {path, content, size, truncated} cache for toggle
let filesPendingRestore = null;     // path to restore after tree loads
let filesPendingView = 'source';    // view to restore
let filesTreeTab = 'changes';      // 'files' or 'changes' (default: diff view)
const filesChangesData = new Map(); // channelId -> [{path, remote, branch, entries}, ...]

function updateFilesModifiedCount() {
  const el = document.getElementById('files-mode-modified-count');
  if (!el) return;
  const repos = filesChangesData.get(filesChannelId) || [];
  let total = 0;
  for (const repo of repos) total += (repo.entries || []).length;
  el.textContent = String(total);
  el.classList.toggle('hidden', total === 0);
}

function setFilesMode(mode) {
  if (mode !== 'files' && mode !== 'changes') return;
  if (mode === filesTreeTab) return;
  filesTreeTab = mode;
  // Sync both the new mode-switch and the legacy tree-tab buttons.
  document.querySelectorAll('.files-mode-btn').forEach(b => b.classList.toggle('active', b.dataset.filesMode === mode));
  document.querySelectorAll('.tree-tab').forEach(b => b.classList.toggle('active', b.dataset.treeTab === mode));
  const _ftConn = getE2EE(filesChannelId);
  if (mode === 'changes' && filesChannelId && _ftConn && _ftConn.connected) {
    _ftConn.filesChanges(filesChannelId);
  }
  renderFileTree();
}
document.querySelectorAll('.tree-tab').forEach(btn => {
  btn.addEventListener('click', () => setFilesMode(btn.dataset.treeTab));
});
document.querySelectorAll('.files-mode-btn').forEach(btn => {
  btn.addEventListener('click', () => setFilesMode(btn.dataset.filesMode));
});

function filesLoadRoot() {
  const _flConn = getActiveE2EE();
  if (!chatCurrentChannel || !_flConn || !_flConn.connected) return;
  filesChannelId = chatCurrentChannel;
  _flConn.filesList(chatCurrentChannel, '');
}

// ---- Tree Rendering ----

let _filesSelfHealed = false;
function renderFileTree() {
  fileTreeInner.querySelectorAll('.tree-item, .tree-children, .changes-empty, .changes-repo-group').forEach(n => n.remove());

  // If the tab is open with a channel selected but filesChannelId wasn't
  // set (E2EE timing race), try once to kick off the load so the view
  // self-heals on re-render instead of showing the "Select a channel" stub.
  if (!filesChannelId && chatCurrentChannel && typeof onFilesTabActivated === 'function' && !_filesSelfHealed) {
    _filesSelfHealed = true;
    onFilesTabActivated();
  }

  if (filesTreeTab === 'changes') {
    // Changes view has its own "No modified files" / "Select a channel" empty
    // states inside renderChangesView; hide the tree-specific placeholder.
    fileTreeEmpty.style.display = 'none';
    renderChangesView(fileTreeInner, filesChannelId ? filesChangesData.get(filesChannelId) : null);
    return;
  }

  // Files tree mode — requires a channel + root listing in fileTreeData.
  if (!filesChannelId) { fileTreeEmpty.style.display = ''; return; }
  const data = fileTreeData.get(filesChannelId);
  if (!data || !data.has('')) {
    fileTreeEmpty.style.display = '';
    return;
  }
  fileTreeEmpty.style.display = 'none';
  const rootEntries = data.get('');
  if (rootEntries) renderTreeLevel(fileTreeInner, rootEntries.entries, 0, data);
}

function renderTreeLevel(container, entries, depth, data) {
  for (const entry of entries) {
    const item = document.createElement('div');
    item.className = 'tree-item';
    if (depth > 0) item.setAttribute('data-depth', Math.min(depth, 5));
    item.dataset.path = entry.path;
    item.dataset.type = entry.type;

    // Git status classes.
    if (entry.is_gitignored) item.classList.add('gitignored');
    if (entry.git_status === '?' || entry.staged_status === '?') item.classList.add('untracked');
    if (entry.path === filesCurrentPath) item.classList.add('active');

    // Arrow for directories.
    const arrow = document.createElement('span');
    arrow.className = 'tree-arrow';
    if (entry.type === 'dir') {
      const expanded = data.has(entry.path);
      arrow.innerHTML = '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
      if (expanded) arrow.classList.add('open');
    } else {
      arrow.classList.add('hidden');
    }
    item.appendChild(arrow);

    // Icon.
    const icon = document.createElement('span');
    icon.className = 'tree-icon' + (entry.type === 'dir' ? ' folder' : '');
    icon.innerHTML = entry.type === 'dir'
      ? '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M1.5 3.5v7a1 1 0 001 1h9a1 1 0 001-1v-5a1 1 0 00-1-1H7L5.5 2.5h-3a1 1 0 00-1 1z" stroke="currentColor" stroke-width="1.2"/></svg>'
      : '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3.5 1.5h4l3 3v7a1 1 0 01-1 1h-6a1 1 0 01-1-1v-9a1 1 0 011-1z" stroke="currentColor" stroke-width="1.2"/><path d="M7.5 1.5v3h3" stroke="currentColor" stroke-width="1.2"/></svg>';
    item.appendChild(icon);

    // Label.
    const label = document.createElement('span');
    label.className = 'tree-label';
    label.textContent = entry.name;
    label.title = entry.path || entry.name;
    item.appendChild(label);

    // Git badge + stats — wrapped in sticky container.
    const hasGitBadge = entry.git_status || entry.staged_status;
    const hasStats = entry.insertions > 0 || entry.deletions > 0;
    if (hasGitBadge || hasStats) {
      const gitInfo = document.createElement('span');
      gitInfo.className = 'tree-git-info';
      if (hasGitBadge) {
        const st = entry.staged_status || entry.git_status;
        if (st === '?' || st === 'A') {
          const badge = document.createElement('span');
          badge.className = 'tree-badge added';
          badge.textContent = st === '?' ? 'U' : 'A';
          gitInfo.appendChild(badge);
        } else if (st === 'M') {
          const badge = document.createElement('span');
          badge.className = 'tree-badge modified';
          badge.textContent = 'M';
          gitInfo.appendChild(badge);
        } else if (st === 'D') {
          const badge = document.createElement('span');
          badge.className = 'tree-badge modified';
          badge.textContent = 'D';
          gitInfo.appendChild(badge);
        }
      }
      if (hasStats) {
        const stats = document.createElement('span');
        stats.className = 'tree-stats';
        if (entry.insertions > 0) {
          const add = document.createElement('span');
          add.className = 'stat-add';
          add.textContent = '+' + entry.insertions;
          stats.appendChild(add);
        }
        if (entry.deletions > 0) {
          const del = document.createElement('span');
          del.className = 'stat-del';
          del.textContent = '-' + entry.deletions;
          stats.appendChild(del);
        }
        gitInfo.appendChild(stats);
      }
      item.appendChild(gitInfo);
    }

    container.appendChild(item);

    // Click handler.
    item.addEventListener('click', (ev) => {
      ev.stopPropagation();
      if (entry.type === 'dir') {
        toggleDirectory(entry.path, arrow, item);
      } else {
        selectFile(entry.path, entry);
      }
    });

    // Render children if expanded.
    if (entry.type === 'dir' && data.has(entry.path)) {
      const childContainer = document.createElement('div');
      childContainer.className = 'tree-children open';
      childContainer.dataset.parentPath = entry.path;
      renderTreeLevel(childContainer, data.get(entry.path).entries, depth + 1, data);
      container.appendChild(childContainer);
    }
  }
}

function renderChangesView(container, repos) {
  // repos is an array of {path, remote, branch, entries[]} from the backend (or undefined if not yet loaded).
  if (!repos) {
    const loadingDiv = document.createElement('div');
    loadingDiv.className = 'empty-state pad-1-5 changes-empty';
    loadingDiv.innerHTML = '<div class="loading-spinner"></div><p class="text-xs">Loading changes…</p>';
    container.appendChild(loadingDiv);
    return;
  }
  if (repos.length === 0) {
    const emptyDiv = document.createElement('div');
    emptyDiv.className = 'empty-state pad-1-5 changes-empty';
    emptyDiv.innerHTML = '<p class="text-xs">No changes</p>';
    container.appendChild(emptyDiv);
    return;
  }

  // Sort repos by most recently modified file (descending).
  const sortedRepos = repos.slice().sort((a, b) => {
    const aMax = Math.max(0, ...a.entries.map(e => e.modified || 0));
    const bMax = Math.max(0, ...b.entries.map(e => e.modified || 0));
    return bMax - aMax;
  });

  for (const repo of sortedRepos) {
    const group = document.createElement('div');
    group.className = 'changes-repo-group';

    // Repo header.
    const header = document.createElement('div');
    header.className = 'changes-repo-header';

    const chevron = document.createElement('span');
    chevron.className = 'changes-repo-chevron';
    chevron.innerHTML = '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
    header.appendChild(chevron);

    const nameSpan = document.createElement('span');
    nameSpan.className = 'changes-repo-name';
    nameSpan.textContent = repo.path === '.' ? '(root)' : repo.path;
    header.appendChild(nameSpan);

    const meta = document.createElement('span');
    meta.className = 'changes-repo-meta';
    if (repo.remote && repo.branch) {
      meta.textContent = repo.remote + ' @ ' + repo.branch;
    } else {
      meta.textContent = repo.remote || repo.branch || '';
    }
    header.appendChild(meta);

    // Aggregate stats.
    let totalIns = 0, totalDels = 0;
    for (const e of repo.entries) { totalIns += e.insertions || 0; totalDels += e.deletions || 0; }
    const statsSpan = document.createElement('span');
    statsSpan.className = 'changes-repo-stats';
    let statsHTML = repo.entries.length + ' file' + (repo.entries.length !== 1 ? 's' : '');
    if (totalIns > 0) statsHTML += '&ensp;<span class="stat-add">+' + totalIns + '</span>';
    if (totalDels > 0) statsHTML += '&ensp;<span class="stat-del">\u2212' + totalDels + '</span>';
    statsSpan.innerHTML = statsHTML;
    header.appendChild(statsSpan);

    group.appendChild(header);

    // File list.
    const fileList = document.createElement('div');
    fileList.className = 'changes-repo-files';

    const sorted = repo.entries.slice().sort((a, b) => a.path.localeCompare(b.path));
    for (const entry of sorted) {
      const item = document.createElement('div');
      item.className = 'tree-item';
      if (entry.git_status === '?' || entry.staged_status === '?') item.classList.add('untracked');
      if (entry.path === filesCurrentPath) item.classList.add('active');
      item.dataset.path = entry.path;
      item.dataset.type = entry.type;

      const arrow = document.createElement('span');
      arrow.className = 'tree-arrow hidden';
      item.appendChild(arrow);

      const icon = document.createElement('span');
      icon.className = 'tree-icon';
      icon.innerHTML = '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3.5 1.5h4l3 3v7a1 1 0 01-1 1h-6a1 1 0 01-1-1v-9a1 1 0 011-1z" stroke="currentColor" stroke-width="1.2"/><path d="M7.5 1.5v3h3" stroke="currentColor" stroke-width="1.2"/></svg>';
      item.appendChild(icon);

      // Show path relative to repo (strip repo prefix).
      const label = document.createElement('span');
      label.className = 'tree-label';
      let displayPath = entry.path;
      if (repo.path !== '.' && displayPath.startsWith(repo.path + '/')) {
        displayPath = displayPath.slice(repo.path.length + 1);
      }
      label.textContent = displayPath;
      label.title = entry.path;
      item.appendChild(label);

      const hasGitBadge = entry.git_status || entry.staged_status;
      const hasStats = entry.insertions > 0 || entry.deletions > 0;
      if (hasGitBadge || hasStats) {
        const gitInfo = document.createElement('span');
        gitInfo.className = 'tree-git-info';
        if (hasGitBadge) {
          const st = entry.staged_status || entry.git_status;
          const badge = document.createElement('span');
          if (st === '?' || st === 'A') {
            badge.className = 'tree-badge added';
            badge.textContent = st === '?' ? 'U' : 'A';
          } else if (st === 'M') {
            badge.className = 'tree-badge modified';
            badge.textContent = 'M';
          } else if (st === 'D') {
            badge.className = 'tree-badge modified';
            badge.textContent = 'D';
          }
          if (badge.textContent) gitInfo.appendChild(badge);
        }
        if (hasStats) {
          const stats = document.createElement('span');
          stats.className = 'tree-stats';
          if (entry.insertions > 0) {
            const add = document.createElement('span');
            add.className = 'stat-add';
            add.textContent = '+' + entry.insertions;
            stats.appendChild(add);
          }
          if (entry.deletions > 0) {
            const del = document.createElement('span');
            del.className = 'stat-del';
            del.textContent = '-' + entry.deletions;
            stats.appendChild(del);
          }
          gitInfo.appendChild(stats);
        }
        item.appendChild(gitInfo);
      }

      item.addEventListener('click', () => selectFile(entry.path, entry, 'diff'));
      fileList.appendChild(item);
    }

    group.appendChild(fileList);

    // Toggle collapse on header click.
    header.addEventListener('click', () => {
      group.classList.toggle('collapsed');
    });

    container.appendChild(group);
  }
}

function toggleDirectory(path, arrowEl, itemEl) {
  const data = fileTreeData.get(filesChannelId);
  if (!data) return;

  if (data.has(path)) {
    // Collapse: remove cached children.
    data.delete(path);
    renderFileTree();
  } else {
    // Expand: request listing.
    getE2EE(filesChannelId)?.filesList(filesChannelId, path);
  }
}

function selectFile(path, entry, initialView) {
  const ext = (path.split('.').pop() || '').toLowerCase();
  filesCurrentIsMarkdown = (ext === 'md' || ext === 'markdown');
  filesCurrentIsSvg = (ext === 'svg');
  filesCurrentIsHtml = (ext === 'html' || ext === 'htm');
  filesCurrentHasDiff = !!(entry && (entry.git_status || entry.staged_status || entry.insertions || entry.deletions));
  filesCurrentPath = path;
  filesLastContent = null;

  // Determine initial view.
  if (initialView === 'diff' && filesCurrentHasDiff) {
    filesCurrentView = 'diff';
  } else if (filesCurrentIsMarkdown || filesCurrentIsSvg || filesCurrentIsHtml) {
    filesCurrentView = 'rendered';
  } else {
    filesCurrentView = 'source';
  }

  // Update active state in tree.
  fileTree.querySelectorAll('.tree-item.active').forEach(el => el.classList.remove('active'));
  const active = fileTree.querySelector(`.tree-item[data-path="${CSS.escape(path)}"]`);
  if (active) active.classList.add('active');

  // Update path bar.
  filesPathText.textContent = path;
  filesPathText.classList.remove('empty');
  fileReloadBtn.classList.remove('hc-hidden');

  // Close mobile dropdown.
  fileTreePanel.classList.remove('mobile-open');
  filesPathChevron.classList.remove('open');

  updateFloatingToggle();

  // Save to localStorage.
  saveChannelState(filesChannelId, { filesPath: path, filesView: filesCurrentView });

  // Load file content.
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
  const _fileConn = getE2EE(filesChannelId);
  if (filesCurrentView === 'diff') {
    _fileConn?.fileDiff(filesChannelId, path, false);
  } else {
    _fileConn?.fileRead(filesChannelId, path);
  }
}

function updateFloatingToggle() {
  if (!filesCurrentPath) {
    fileFloatToggle.style.display = 'none';
    return;
  }
  const tabs = [];
  if (filesCurrentIsMarkdown || filesCurrentIsSvg || filesCurrentIsHtml) tabs.push({ id: 'rendered', label: 'Preview' });
  if (filesCurrentHasDiff) tabs.push({ id: 'diff', label: 'Diff' });
  tabs.push({ id: 'source', label: 'Source' });

  if (tabs.length <= 1) {
    fileFloatToggle.style.display = 'none';
    return;
  }
  fileFloatToggle.style.display = 'flex';
  fileFloatToggle.innerHTML = tabs.map(t =>
    `<button data-view="${t.id}" class="${filesCurrentView === t.id ? 'active' : ''}">${t.label}</button>`
  ).join('');
}

// ---- Floating toggle ----
fileFloatToggle.addEventListener('click', (ev) => {
  const btn = ev.target.closest('button[data-view]');
  if (!btn || !filesCurrentPath) return;
  const view = btn.dataset.view;
  if (view === filesCurrentView) return;

  filesCurrentView = view;
  const _fvConn = getE2EE(filesChannelId);
  if (view === 'diff') {
    fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
    _fvConn?.fileDiff(filesChannelId, filesCurrentPath, false);
  } else if (view === 'rendered') {
    if (filesLastContent) {
      renderPreview(filesLastContent.content, filesLastContent.truncated, filesLastContent.size);
    } else {
      fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
      _fvConn?.fileRead(filesChannelId, filesCurrentPath);
    }
  } else {
    if (filesLastContent) {
      renderFileContent(filesLastContent.content, filesLastContent.path, filesLastContent.size, filesLastContent.truncated);
    } else {
      fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
      _fvConn?.fileRead(filesChannelId, filesCurrentPath);
    }
  }
  updateFloatingToggle();
  saveChannelState(filesChannelId, { filesView: filesCurrentView });
});

// ---- Line-wrap toggle ----
const fileWrapToggle = document.getElementById('file-wrap-toggle');
let filesLineWrap = localStorage.getItem('filesLineWrap') === '1';
if (filesLineWrap) {
  fileContentBody.classList.add('line-wrap');
  fileWrapToggle.classList.add('active');
}
fileWrapToggle.addEventListener('click', () => {
  filesLineWrap = !filesLineWrap;
  fileContentBody.classList.toggle('line-wrap', filesLineWrap);
  fileWrapToggle.classList.toggle('active', filesLineWrap);
  localStorage.setItem('filesLineWrap', filesLineWrap ? '1' : '0');
});

// ---- Mobile file tree dropdown toggle ----
filesPathBar.addEventListener('click', (ev) => {
  // Only act on mobile (chevron visible).
  if (getComputedStyle(filesPathChevron).display === 'none') return;
  const isOpen = fileTreePanel.classList.toggle('mobile-open');
  filesPathChevron.classList.toggle('open', isOpen);
});

// ---- Reload button ----
fileReloadBtn.addEventListener('click', (ev) => {
  ev.stopPropagation(); // Don't trigger path bar mobile toggle.
  if (!filesCurrentPath || !filesChannelId) return;
  filesLastContent = null;
  const conn = getE2EE(filesChannelId);
  if (!conn || !conn.connected) return;
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
  if (filesCurrentView === 'diff') {
    conn.fileDiff(filesChannelId, filesCurrentPath, false);
  } else {
    conn.fileRead(filesChannelId, filesCurrentPath);
  }
});

// ---- Live file-change refresh (fed by AGENT_FILE_CHANGES) ----
let _fileChangesRefreshTimer = null;
function onAgentFileChanges(channelId, paths) {
  console.debug('[files] agent.file_changes', { channelId, paths, filesChannelId, chatCurrentChannel });
  // Only react if this channel is the one currently displayed in the files view.
  if (filesChannelId && filesChannelId !== channelId) return;
  // Adopt the channel so the files_changes_result handler doesn't drop the
  // response if filesChannelId was still null at this point.
  if (!filesChannelId && chatCurrentChannel === channelId) {
    filesChannelId = channelId;
  }
  // Debounce bursts: coalesce multiple events within 150ms.
  clearTimeout(_fileChangesRefreshTimer);
  _fileChangesRefreshTimer = setTimeout(() => {
    const conn = getE2EE(filesChannelId || channelId);
    if (!conn || !conn.connected) return;
    // Always refresh the changes list so the Modified count stays live.
    conn.filesChanges(filesChannelId || channelId);
    // If the tree is showing, invalidate cached path entries so stale items are re-fetched.
    if (filesTreeTab === 'files') {
      const data = fileTreeData.get(filesChannelId);
      if (data) {
        for (const p of paths) {
          const parent = p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '';
          data.delete(parent);
        }
      }
      conn.filesList(filesChannelId || channelId, '');
    }
    // If the currently open file was touched, re-fetch its content/diff.
    if (filesCurrentPath) {
      const cur = filesCurrentPath.replace(/\\/g, '/');
      const touched = paths.some(p => {
        const pp = (p || '').replace(/\\/g, '/');
        if (!pp) return false;
        return pp === cur
          || cur.endsWith('/' + pp)
          || pp.endsWith('/' + cur)
          || pp.split('/').pop() === cur.split('/').pop();
      });
      if (touched) {
        if (filesCurrentView === 'diff') {
          conn.fileDiff(filesChannelId || channelId, filesCurrentPath, false);
        } else {
          conn.fileRead(filesChannelId || channelId, filesCurrentPath);
        }
      }
    }
  }, 150);
}

// ---- File Content Renderer ----

function renderFileContent(content, path, size, truncated) {
  // Cache for toggle without re-fetch.
  filesLastContent = { content, path, size, truncated };
  fileContentBody.style.padding = '';

  // Preview mode for markdown/SVG/HTML.
  if (filesCurrentView === 'rendered' && (filesCurrentIsMarkdown || filesCurrentIsSvg || filesCurrentIsHtml)) {
    renderPreview(content, truncated, size);
    return;
  }

  const extMatch = path.match(/\.([^./]+)$/);
  const ext = extMatch ? extMatch[1].toLowerCase() : path.split('/').pop().toLowerCase();
  const lines = content.split('\n');
  // Remove trailing empty line from split.
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();

  const viewer = document.createElement('div');
  viewer.className = 'file-viewer';

  for (let i = 0; i < lines.length; i++) {
    const row = document.createElement('div');
    row.className = 'file-line';

    const numEl = document.createElement('span');
    numEl.className = 'fl-num';
    numEl.textContent = String(i + 1);

    const codeEl = document.createElement('span');
    codeEl.className = 'fl-content';
    codeEl.innerHTML = highlightLine(lines[i], ext);

    row.appendChild(numEl);
    row.appendChild(codeEl);
    viewer.appendChild(row);
  }

  fileContentBody.innerHTML = '';
  fileContentBody.appendChild(viewer);

  if (truncated) {
    const note = document.createElement('div');
    note.style.cssText = 'padding:.5rem .75rem;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border)';
    note.textContent = `File truncated (${formatBytes(size)} total)`;
    fileContentBody.appendChild(note);
  }
}

function renderPreview(content, truncated, size) {
  if (filesCurrentIsSvg) {
    renderSvgPreview(content);
  } else if (filesCurrentIsHtml) {
    renderHtmlPreview(content, filesCurrentPath);
  } else {
    renderMarkdownFile(content, truncated, size);
  }
}

function renderMarkdownFile(content, truncated, size) {
  const wrapper = document.createElement('div');
  wrapper.className = 'file-markdown-view msg-text';
  wrapper.innerHTML = renderMarkdown(content);
  fileContentBody.innerHTML = '';
  fileContentBody.appendChild(wrapper);
  if (truncated) {
    const note = document.createElement('div');
    note.style.cssText = 'padding:.5rem .75rem;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border)';
    note.textContent = `File truncated (${formatBytes(size)} total)`;
    fileContentBody.appendChild(note);
  }
}

function renderSvgPreview(content) {
  const blob = new Blob([content], { type: 'image/svg+xml' });
  const url = URL.createObjectURL(blob);
  fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + url + '" alt="SVG preview"></div>';
}

// ---- HTML Preview with Asset Inlining ----

// Promise-based file read for fetching assets without conflicting with main file viewer.
function readFileAsync(channelId, path) {
  return new Promise((resolve, reject) => {
    const conn = getE2EE(channelId);
    if (!conn || !conn.connected) return reject(new Error('not connected'));
    const chunks = {};
    function handler(evt) {
      const d = evt.detail;
      if (d.path !== path) return;
      if (d.error) { conn.removeEventListener('file_read_result', handler); return reject(new Error(d.error)); }
      // Handle chunked images.
      if (d.is_image && d.chunk_total && d.chunk_total > 1) {
        if (!chunks.arr) { chunks.arr = new Array(d.chunk_total); chunks.total = d.chunk_total; }
        chunks.arr[d.chunk_index] = d.content;
        if (chunks.arr.filter(Boolean).length < chunks.total) return;
        conn.removeEventListener('file_read_result', handler);
        return resolve({ ...d, content: chunks.arr.join('') });
      }
      conn.removeEventListener('file_read_result', handler);
      resolve(d);
    }
    conn.addEventListener('file_read_result', handler);
    conn.fileRead(channelId, path);
  });
}

const MIME_TYPES = {
  css: 'text/css', js: 'application/javascript', mjs: 'application/javascript',
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  webp: 'image/webp', ico: 'image/x-icon', svg: 'image/svg+xml',
  woff: 'font/woff', woff2: 'font/woff2', ttf: 'font/ttf', eot: 'application/vnd.ms-fontobject',
  json: 'application/json',
};

function resolveAssetPath(htmlFilePath, assetHref) {
  if (!assetHref || assetHref.startsWith('data:') || assetHref.startsWith('http:') || assetHref.startsWith('https:') || assetHref.startsWith('//')) return null;
  // Strip query/hash.
  const clean = assetHref.split('?')[0].split('#')[0];
  // Resolve relative to HTML file's directory.
  const dir = htmlFilePath.substring(0, htmlFilePath.lastIndexOf('/') + 1);
  // Simple path resolution (handles ../ and ./).
  const parts = (dir + clean).split('/');
  const resolved = [];
  for (const p of parts) {
    if (p === '.' || p === '') continue;
    if (p === '..') { resolved.pop(); continue; }
    resolved.push(p);
  }
  return resolved.join('/');
}

// Console capture JS (raw code, no <script> tags — injected via DOM).
const CONSOLE_CAPTURE_JS = `(function(){
  function send(level, args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) {
      try { parts.push(typeof args[i] === 'string' ? args[i] : JSON.stringify(args[i], null, 2)); }
      catch(e) { parts.push(String(args[i])); }
    }
    parent.postMessage({ type: '__build_console', entry: { level: level, text: parts.join(' '), ts: Date.now() } }, '*');
  }
  var orig = {};
  ['log','warn','error','info','debug'].forEach(function(m){
    orig[m] = console[m];
    console[m] = function(){ send(m, arguments); if(orig[m]) orig[m].apply(console, arguments); };
  });
  window.onerror = function(msg, src, line, col) {
    send('error', [msg + (src ? ' at ' + src + ':' + line + ':' + col : '')]);
  };
  window.addEventListener('unhandledrejection', function(e) {
    send('error', ['Unhandled rejection: ' + (e.reason && e.reason.message || e.reason || 'unknown')]);
  });
  window.addEventListener('error', function(e) {
    if (e.target && e.target !== window) {
      var tag = e.target.tagName || '';
      var src = e.target.src || e.target.href || '';
      send('error', ['Failed to load ' + tag.toLowerCase() + (src ? ': ' + src : '')]);
    }
  }, true);
})();`;

// Script to intercept relative link clicks and navigate via parent.
const NAV_INTERCEPT_JS = `(function(){
  document.addEventListener('click', function(e) {
    var a = e.target.closest('a[href]');
    if (!a) return;
    var href = a.getAttribute('href');
    if (!href || href.startsWith('#') || href.startsWith('javascript:')) return;
    if (href.startsWith('http:') || href.startsWith('https:') || href.startsWith('//') || href.startsWith('mailto:')) return;
    e.preventDefault();
    parent.postMessage({ type: '__build_preview_navigate', href: href }, '*');
  });
})();`;

// ---- Browser Proxy View ----

function urlFetchAsync(deviceId, url, tabId, method, body, contentType) {
  return new Promise((resolve, reject) => {
    const conn = e2eeConnections.get(deviceId);
    if (!conn || !conn.connected) return reject(new Error('not connected'));
    const requestId = 'rf-' + Date.now() + '-' + Math.random().toString(36).slice(2, 6);
    function handler(evt) {
      const d = evt.detail;
      if (d.request_id !== requestId) return;
      conn.removeEventListener('url_fetch_result', handler);
      if (d.error) return reject(new Error(d.error));
      resolve(d);
    }
    conn.addEventListener('url_fetch_result', handler);
    conn.urlFetch(url, requestId, tabId || '', method, body, contentType);
  });
}

function resolveUrl(baseUrl, href) {
  if (!href || href.startsWith('data:') || href.startsWith('#') || href.startsWith('javascript:')) return null;
  try { return new URL(href, baseUrl).href; } catch { return null; }
}

async function fetchAndRenderBrowserPage(tab) {
  const browserContent = document.getElementById('browser-content');
  if (!browserContent) return;
  browserContent.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';

  const deviceId = tab.deviceId;
  const url = tab.url;
  let errors = [];
  const _dbg = (text) => errors.push({ level: 'debug', text });

  try {
    const method = tab._method || 'GET';
    const body = tab._body || undefined;
    const contentType = tab._contentType || undefined;
    // Clear one-shot POST data after use
    tab._method = undefined;
    tab._body = undefined;
    tab._contentType = undefined;
    _dbg('Fetching ' + method + ' ' + url);
    const result = await urlFetchAsync(deviceId, url, tab.id, method, body, contentType);
    // Update URL bar if server redirected us
    if (result.final_url && result.final_url !== url) {
      tab.url = result.final_url;
      const urlInput = document.getElementById('browser-url-input');
      if (urlInput) urlInput.value = result.final_url;
      _dbg('Redirected to ' + result.final_url);
    }
    if (result.is_binary) {
      browserContent.innerHTML = '<div class="empty-state"><p>Cannot display binary content</p></div>';
      return;
    }
    if (result.status && result.status >= 400) {
      _dbg('HTTP ' + result.status);
    }

    let html = result.content;
    const baseUrl = result.final_url || url;
    _dbg('Got ' + html.length + ' bytes');

    // Resolve and inline local assets (CSS, JS, images)
    const replacements = [];
    const isExternal = (u) => {
      if (!u) return true;
      try { const p = new URL(u, baseUrl); return p.origin !== new URL(baseUrl).origin; } catch { return true; }
    };

    // CSS links
    const linkRe = /<link\b[^>]*\brel\s*=\s*["']stylesheet["'][^>]*>|<link\b[^>]*\bhref\s*=\s*["'][^"']+["'][^>]*\brel\s*=\s*["']stylesheet["'][^>]*>/gi;
    const hrefRe = /\bhref\s*=\s*["']([^"']+)["']/i;
    for (const m of html.matchAll(linkRe)) {
      const tag = m[0];
      const hm = tag.match(hrefRe);
      if (!hm) continue;
      const href = hm[1];
      const resolved = resolveUrl(baseUrl, href);
      if (!resolved || isExternal(resolved)) { _dbg('CSS (ext): ' + href); continue; }
      _dbg('CSS: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.content && !r.is_binary) {
          // Resolve url() references inside CSS
          let css = r.content;
          css = css.replace(/url\(\s*["']?(?!data:|https?:|\/\/)([^"')]+)["']?\s*\)/g, (match, ref) => {
            const absRef = resolveUrl(resolved, ref);
            return absRef ? `url(${absRef})` : match;
          });
          return { original: tag, replacement: '<style>' + css + '</style>' };
        }
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'CSS failed: ' + href + ' (' + err.message + ')' }); return null; }));
    }

    // Script tags
    const scriptRe = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>\s*<\/script>/gi;
    for (const m of html.matchAll(scriptRe)) {
      const tag = m[0];
      const src = m[1];
      const resolved = resolveUrl(baseUrl, src);
      if (!resolved || isExternal(resolved)) { _dbg('JS (ext): ' + src); continue; }
      _dbg('JS: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.content && !r.is_binary) return { original: tag, replacement: '<script>' + r.content + '<\/script>' };
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'JS failed: ' + src + ' (' + err.message + ')' }); return null; }));
    }

    // Images
    const imgRe = /(<img\b[^>]*\bsrc\s*=\s*["'])([^"']+)(["'][^>]*>)/gi;
    for (const m of html.matchAll(imgRe)) {
      const tag = m[0];
      const src = m[2];
      if (src.startsWith('data:')) continue;
      const resolved = resolveUrl(baseUrl, src);
      if (!resolved || isExternal(resolved)) continue;
      _dbg('IMG: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.is_binary && r.content) {
          const ct = r.content_type || 'image/png';
          const mime = ct.split(';')[0].trim();
          return { original: tag, replacement: m[1] + 'data:' + mime + ';base64,' + r.content + m[3] };
        }
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'IMG failed: ' + src + ' (' + err.message + ')' }); return null; }));
    }

    _dbg('Fetching ' + replacements.length + ' assets...');
    const results = await Promise.all(replacements);
    for (const r of results) {
      if (r) html = html.replace(r.original, r.replacement);
    }

    // Bail if user navigated away
    if (activeBrowserTab !== tab.id) return;

    // Inject console capture + navigation intercept
    const headMatch = html.match(/<head[^>]*>/i);
    if (headMatch) {
      const idx = html.indexOf(headMatch[0]) + headMatch[0].length;
      const navJs = `(function(){
        // --- Patch fetch to proxy through Build ---
        var _origFetch = window.fetch;
        var _reqId = 0;
        var _pending = {};
        window.addEventListener('message', function(evt) {
          if (evt.data && evt.data.type === '__build_fetch_response' && _pending[evt.data.reqId]) {
            _pending[evt.data.reqId](evt.data);
            delete _pending[evt.data.reqId];
          }
        });
        window.fetch = function(input, init) {
          init = init || {};
          var url = typeof input === 'string' ? input : (input && input.url ? input.url : String(input));
          var method = (init.method || (input && input.method) || 'GET').toUpperCase();
          var body = init.body || null;
          var contentType = null;
          var headers = init.headers;
          if (headers) {
            if (typeof headers.get === 'function') contentType = headers.get('content-type');
            else if (headers['Content-Type']) contentType = headers['Content-Type'];
            else if (headers['content-type']) contentType = headers['content-type'];
          }
          if (body && typeof body !== 'string') {
            try { body = new URLSearchParams(body).toString(); if (!contentType) contentType = 'application/x-www-form-urlencoded'; } catch(e) { body = String(body); }
          }
          var id = '__bf_' + (++_reqId);
          return new Promise(function(resolve) {
            _pending[id] = function(data) {
              var respInit = { status: data.status || 200, headers: { 'Content-Type': data.contentType || 'text/plain' } };
              resolve(new Response(data.body || '', respInit));
            };
            parent.postMessage({ type: '__build_browser_fetch', reqId: id, url: url, method: method, body: body, contentType: contentType }, '*');
          });
        };

        // --- Patch XMLHttpRequest to proxy through Build ---
        var _OrigXHR = XMLHttpRequest;
        function ProxyXHR() {
          this._method = 'GET'; this._url = ''; this._headers = {}; this._async = true;
          this.readyState = 0; this.status = 0; this.statusText = '';
          this.responseText = ''; this.response = ''; this.responseType = '';
          this.onreadystatechange = null; this.onload = null; this.onerror = null;
          this._listeners = {};
        }
        ProxyXHR.prototype.open = function(method, url, async) { this._method = method; this._url = url; this._async = async !== false; this.readyState = 1; };
        ProxyXHR.prototype.setRequestHeader = function(k, v) { this._headers[k.toLowerCase()] = v; };
        ProxyXHR.prototype.getResponseHeader = function(k) { return this._responseHeaders ? (this._responseHeaders[k.toLowerCase()] || null) : null; };
        ProxyXHR.prototype.getAllResponseHeaders = function() { return ''; };
        ProxyXHR.prototype.addEventListener = function(e, fn) { if (!this._listeners[e]) this._listeners[e] = []; this._listeners[e].push(fn); };
        ProxyXHR.prototype.removeEventListener = function(e, fn) { if (this._listeners[e]) this._listeners[e] = this._listeners[e].filter(function(f){return f !== fn;}); };
        ProxyXHR.prototype._fire = function(e) { var fns = this._listeners[e] || []; for (var i = 0; i < fns.length; i++) fns[i].call(this, {}); };
        ProxyXHR.prototype.send = function(body) {
          var self = this;
          var id = '__bf_' + (++_reqId);
          _pending[id] = function(data) {
            self.status = data.status || 200;
            self.statusText = data.status ? String(data.status) : 'OK';
            self.responseText = data.body || '';
            self.response = data.body || '';
            self._responseHeaders = { 'content-type': data.contentType || 'text/plain' };
            self.readyState = 4;
            if (self.onreadystatechange) self.onreadystatechange();
            if (self.onload) self.onload();
            self._fire('readystatechange');
            self._fire('load');
            self._fire('loadend');
          };
          parent.postMessage({ type: '__build_browser_fetch', reqId: id, url: self._url, method: self._method, body: body || null, contentType: self._headers['content-type'] || null }, '*');
        };
        ProxyXHR.prototype.abort = function() {};
        ProxyXHR.prototype.overrideMimeType = function() {};
        window.XMLHttpRequest = ProxyXHR;

        // --- Navigation intercept (lower priority: skip if page already handled) ---
        document.addEventListener('click', function(e) {
          if (e.defaultPrevented) return;
          var a = e.target.closest('a[href]');
          if (!a) return;
          var href = a.getAttribute('href');
          if (!href || href.startsWith('#') || href.startsWith('javascript:')) return;
          e.preventDefault();
          parent.postMessage({ type: '__build_browser_navigate', href: href }, '*');
        });
        document.addEventListener('submit', function(e) {
          if (e.defaultPrevented) return;
          var form = e.target;
          if (!form || form.tagName !== 'FORM') return;
          e.preventDefault();
          var action = form.getAttribute('action') || window.location.href;
          var method = (form.getAttribute('method') || 'GET').toUpperCase();
          var fd = new FormData(form);
          if (method === 'GET') {
            var params = new URLSearchParams(fd).toString();
            var sep = action.indexOf('?') === -1 ? '?' : '&';
            parent.postMessage({ type: '__build_browser_navigate', href: action + sep + params }, '*');
          } else {
            parent.postMessage({ type: '__build_browser_form_submit', action: action, method: method, body: new URLSearchParams(fd).toString(), contentType: 'application/x-www-form-urlencoded' }, '*');
          }
        });
      })();`;
      html = html.slice(0, idx) + '<script>' + CONSOLE_CAPTURE_JS + navJs + '<\/script>' + html.slice(idx);
    }

    const inlinedHtml = html;

    // Build wrapper
    browserContent.innerHTML = '';
    const wrapper = document.createElement('div');
    wrapper.style.cssText = 'position:relative;width:100%;height:100%;display:flex;flex-direction:column';

    const iframe = document.createElement('iframe');
    iframe.style.cssText = 'width:100%;flex:1;border:none;background:#fff;min-height:0';
    wrapper.appendChild(iframe);

    // Console overlay (matches activity/terminal console style)
    const consoleBar = document.createElement('div');
    consoleBar.className = 'html-console-bar';
    consoleBar.innerHTML = '<span class="html-console-title">Console</span><span class="html-console-badge hc-hidden">0</span><span class="html-console-toggle"><svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 9l4-4 4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg></span>';
    wrapper.appendChild(consoleBar);

    const consolePanel = document.createElement('div');
    consolePanel.className = 'html-console-panel hc-hidden';
    wrapper.appendChild(consolePanel);

    browserContent.appendChild(wrapper);

    const badge = consoleBar.querySelector('.html-console-badge');
    const toggleIcon = consoleBar.querySelector('.html-console-toggle');
    let consoleOpen = false;
    let entryCount = 0;

    consoleBar.addEventListener('click', () => {
      consoleOpen = !consoleOpen;
      consolePanel.classList.toggle('hc-hidden', !consoleOpen);
      toggleIcon.innerHTML = consoleOpen
        ? '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 5l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>'
        : '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 9l4-4 4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
      if (consoleOpen) { badge.classList.add('hc-hidden'); consolePanel.scrollTop = consolePanel.scrollHeight; }
    });

    function addConsoleEntry(level, text) {
      entryCount++;
      if (!consoleOpen) { badge.classList.remove('hc-hidden'); badge.textContent = String(entryCount); }
      const row = document.createElement('div');
      row.className = 'html-console-entry html-console-' + (level || 'log');
      const levelSpan = document.createElement('span');
      levelSpan.className = 'html-console-level';
      levelSpan.textContent = level || 'log';
      row.appendChild(levelSpan);
      const textSpan = document.createElement('span');
      textSpan.textContent = text;
      row.appendChild(textSpan);
      consolePanel.appendChild(row);
      if (consoleOpen) consolePanel.scrollTop = consolePanel.scrollHeight;
    }

    // Populate errors but keep console collapsed by default
    for (const err of errors) addConsoleEntry(err.level, err.text);

    function onMsg(evt) {
      if (!evt.data) return;
      if (evt.data.type === '__build_console') {
        addConsoleEntry(evt.data.entry.level, evt.data.entry.text);
      } else if (evt.data.type === '__build_preview_ready') {
        iframe.contentWindow.postMessage({ type: '__build_preview', html: inlinedHtml }, '*');
      } else if (evt.data.type === '__build_browser_navigate') {
        const target = resolveUrl(baseUrl, evt.data.href);
        if (target && new URL(target).origin === new URL(baseUrl).origin) {
          tab.url = target;
          tab._method = undefined;
          tab._body = undefined;
          tab._contentType = undefined;
          document.getElementById('browser-url-input').value = target;
          renderChannelPanel();
          fetchAndRenderBrowserPage(tab);
        }
      } else if (evt.data.type === '__build_browser_form_submit') {
        const target = resolveUrl(baseUrl, evt.data.action);
        if (target && new URL(target).origin === new URL(baseUrl).origin) {
          tab.url = target;
          tab._method = evt.data.method;
          tab._body = evt.data.body;
          tab._contentType = evt.data.contentType;
          document.getElementById('browser-url-input').value = target;
          renderChannelPanel();
          fetchAndRenderBrowserPage(tab);
        }
      } else if (evt.data.type === '__build_browser_fetch') {
        const reqId = evt.data.reqId;
        const fetchUrl = resolveUrl(baseUrl, evt.data.url);
        if (!fetchUrl) {
          iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: 0, body: 'Invalid URL', contentType: 'text/plain' }, '*');
          return;
        }
        urlFetchAsync(deviceId, fetchUrl, tab.id, evt.data.method || 'GET', evt.data.body || undefined, evt.data.contentType || undefined)
          .then(r => {
            iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: r.status || 200, body: r.content || '', contentType: r.content_type || 'text/plain' }, '*');
          })
          .catch(err => {
            iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: 0, body: err.message, contentType: 'text/plain' }, '*');
          });
      }
    }
    window.addEventListener('message', onMsg);

    const observer = new MutationObserver(() => {
      if (!browserContent.contains(wrapper)) {
        window.removeEventListener('message', onMsg);
        observer.disconnect();
      }
    });
    observer.observe(browserContent, { childList: true });

    iframe.src = '/preview-frame';

  } catch (err) {
    browserContent.innerHTML = '<div class="empty-state"><p>Error: ' + escapeHtml(err.message) + '</p></div>';
  }
}

function showBrowserView(tab) {
  // Hide all tab panels, show browser panel
  document.querySelectorAll('.tab-panel').forEach(p => p.classList.remove('active'));
  document.getElementById('tab-browser')?.classList.add('active');
  // Hide tab buttons active state
  document.querySelectorAll('.viewer-tab[data-tab]').forEach(btn => btn.classList.remove('active'));
  // Browser owns the full right column — hide everything else
  document.getElementById('viewer-tabs')?.classList.add('hidden');
  document.querySelector('.viewer-body')?.classList.add('hidden');
  document.querySelector('.comp-wrapper')?.classList.add('hidden');
  document.getElementById('console-bottom')?.classList.add('hidden');

  const urlInput = document.getElementById('browser-url-input');
  if (urlInput) urlInput.value = tab.url || '';

  // If URL looks valid, load it
  if (tab.url && tab.url.startsWith('http')) {
    fetchAndRenderBrowserPage(tab);
  } else {
    const browserContent = document.getElementById('browser-content');
    if (browserContent) browserContent.innerHTML = '<div class="empty-state"><p>Enter a localhost address and press Go</p></div>';
  }
}

// Browser URL bar handlers
document.getElementById('browser-url-go')?.addEventListener('click', () => {
  if (!activeBrowserTab) return;
  const urlInput = document.getElementById('browser-url-input');
  const url = urlInput?.value?.trim();
  if (!url) return;
  // Find the active tab and update its URL
  for (const [, tabs] of browserTabs) {
    const tab = tabs.find(t => t.id === activeBrowserTab);
    if (tab) {
      tab.url = url;
      renderChannelPanel();
      fetchAndRenderBrowserPage(tab);
      break;
    }
  }
});

document.getElementById('browser-url-input')?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    e.preventDefault();
    document.getElementById('browser-url-go')?.click();
  }
});

// Tracks asset-fetch errors to surface in the console overlay.
let _htmlPreviewErrors = [];

async function renderHtmlPreview(content, htmlPath) {
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading preview...</p></div>';
  _htmlPreviewErrors = [];
  const _dbg = (text) => _htmlPreviewErrors.push({ level: 'debug', text });

  const channelId = filesChannelId;
  let html = content;

  _dbg('Rendering ' + htmlPath + ' (' + content.length + ' bytes)');

  const isExternal = (url) => url && (url.startsWith('http:') || url.startsWith('https:') || url.startsWith('//'));

  // Use string-based asset resolution (avoids DOMParser mangling style/script content).
  const replacements = [];

  // Find local CSS <link> tags (external left as-is — blob iframe has no CSP restrictions).
  const linkRe = /<link\b[^>]*\brel\s*=\s*["']stylesheet["'][^>]*>|<link\b[^>]*\bhref\s*=\s*["'][^"']+["'][^>]*\brel\s*=\s*["']stylesheet["'][^>]*>/gi;
  const hrefRe = /\bhref\s*=\s*["']([^"']+)["']/i;
  for (const m of html.matchAll(linkRe)) {
    const tag = m[0];
    const hm = tag.match(hrefRe);
    if (!hm) continue;
    const href = hm[1];
    if (isExternal(href)) { _dbg('CSS (ext, kept): ' + href); continue; }
    const resolved = resolveAssetPath(htmlPath, href);
    _dbg('CSS (local): ' + href + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.content && !result.is_binary && !result.is_image) {
        _dbg('CSS OK: ' + result.content.length + ' bytes');
        return { original: tag, replacement: '<style>' + result.content + '</style>' };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'CSS failed: ' + href + ' (' + err.message + ')' });
      return null;
    }));
  }

  // Find local script[src] tags (external left as-is).
  const scriptRe = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>\s*<\/script>/gi;
  for (const m of html.matchAll(scriptRe)) {
    const tag = m[0];
    const src = m[1];
    if (isExternal(src)) { _dbg('JS (ext, kept): ' + src); continue; }
    const resolved = resolveAssetPath(htmlPath, src);
    _dbg('JS (local): ' + src + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.content && !result.is_binary && !result.is_image) {
        _dbg('JS OK: ' + result.content.length + ' bytes');
        return { original: tag, replacement: '<script>' + result.content + '<\/script>' };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'JS failed: ' + src + ' (' + err.message + ')' });
      return null;
    }));
  }

  // Find local <img src> tags.
  const imgRe = /(<img\b[^>]*\bsrc\s*=\s*["'])([^"']+)(["'][^>]*>)/gi;
  for (const m of html.matchAll(imgRe)) {
    const tag = m[0];
    const src = m[2];
    if (isExternal(src) || src.startsWith('data:')) continue;
    const resolved = resolveAssetPath(htmlPath, src);
    _dbg('IMG (local): ' + src + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.is_image && result.content) {
        _dbg('IMG OK: ' + resolved);
        return { original: tag, replacement: m[1] + result.content + m[3] };
      } else if (result.content && !result.is_binary) {
        const ext = resolved.split('.').pop().toLowerCase();
        const mime = MIME_TYPES[ext] || 'application/octet-stream';
        return { original: tag, replacement: m[1] + 'data:' + mime + ';base64,' + btoa(result.content) + m[3] };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'IMG failed: ' + src + ' (' + err.message + ')' });
      return null;
    }));
  }

  _dbg('Fetching ' + replacements.length + ' assets...');
  const results = await Promise.all(replacements);
  for (const r of results) {
    if (r) html = html.replace(r.original, r.replacement);
  }
  _dbg('Done. ' + _htmlPreviewErrors.filter(e => e.level === 'error').length + ' errors');

  // Bail if user navigated away.
  if (filesCurrentPath !== htmlPath || filesChannelId !== channelId) return;

  // Inject console capture script right after <head>.
  const headMatch = html.match(/<head[^>]*>/i);
  if (headMatch) {
    const idx = html.indexOf(headMatch[0]) + headMatch[0].length;
    html = html.slice(0, idx) + '<script>' + CONSOLE_CAPTURE_JS + NAV_INTERCEPT_JS + '<\/script>' + html.slice(idx);
  }

  const inlinedHtml = html;

  // Build wrapper with iframe and console overlay.
  fileContentBody.innerHTML = '';
  fileContentBody.style.padding = '0';

  const wrapper = document.createElement('div');
  wrapper.style.cssText = 'position:relative;width:100%;height:100%;display:flex;flex-direction:column';

  const iframe = document.createElement('iframe');
  // No sandbox attr — blob URL already has opaque origin (isolated from parent).
  iframe.style.cssText = 'width:100%;flex:1;border:none;background:#fff;border-radius:4px 4px 0 0;min-height:0';
  wrapper.appendChild(iframe);

  // Console overlay.
  const consoleBar = document.createElement('div');
  consoleBar.className = 'html-console-bar';
  consoleBar.innerHTML = '<span class="html-console-title">Console</span><span class="html-console-badge hc-hidden">0</span><span class="html-console-toggle">&#x25B2;</span>';
  wrapper.appendChild(consoleBar);

  const consolePanel = document.createElement('div');
  consolePanel.className = 'html-console-panel hc-hidden';
  wrapper.appendChild(consolePanel);

  fileContentBody.appendChild(wrapper);

  const badge = consoleBar.querySelector('.html-console-badge');
  const toggleIcon = consoleBar.querySelector('.html-console-toggle');
  let consoleOpen = false;
  let entryCount = 0;

  consoleBar.addEventListener('click', () => {
    consoleOpen = !consoleOpen;
    consolePanel.classList.toggle('hc-hidden', !consoleOpen);
    toggleIcon.innerHTML = consoleOpen ? '&#x25BC;' : '&#x25B2;';
    if (consoleOpen) { badge.classList.add('hc-hidden'); consolePanel.scrollTop = consolePanel.scrollHeight; }
  });

  function addConsoleEntry(level, text) {
    entryCount++;
    if (!consoleOpen) { badge.classList.remove('hc-hidden'); badge.textContent = String(entryCount); }
    const row = document.createElement('div');
    row.className = 'html-console-entry html-console-' + (level || 'log');
    const levelSpan = document.createElement('span');
    levelSpan.className = 'html-console-level';
    levelSpan.textContent = level || 'log';
    row.appendChild(levelSpan);
    const textSpan = document.createElement('span');
    textSpan.textContent = text;
    row.appendChild(textSpan);
    consolePanel.appendChild(row);
    if (consoleOpen) consolePanel.scrollTop = consolePanel.scrollHeight;
  }

  // Surface asset-fetch errors/debug that happened before iframe loaded.
  for (const err of _htmlPreviewErrors) addConsoleEntry(err.level, err.text);
  // Auto-open console if there are entries.
  if (_htmlPreviewErrors.length > 0) {
    consoleOpen = true;
    consolePanel.classList.remove('hc-hidden');
    toggleIcon.innerHTML = '&#x25BC;';
    badge.classList.add('hc-hidden');
  }

  // Listen for console messages and preview-ready signal from iframe.
  function onMsg(evt) {
    if (!evt.data) return;
    if (evt.data.type === '__build_console') {
      addConsoleEntry(evt.data.entry.level, evt.data.entry.text);
    } else if (evt.data.type === '__build_preview_ready') {
      // Preview frame is ready — send the HTML content.
      iframe.contentWindow.postMessage({ type: '__build_preview', html: inlinedHtml }, '*');
    } else if (evt.data.type === '__build_preview_navigate') {
      // Relative link clicked in preview — navigate to that file.
      const targetPath = resolveAssetPath(htmlPath, evt.data.href);
      if (targetPath) {
        // Find the entry in the file tree data and select it in preview mode.
        const data = fileTreeData.get(filesChannelId);
        const dir = targetPath.substring(0, targetPath.lastIndexOf('/') + 1);
        const dirKey = dir ? dir.slice(0, -1) : '';
        const entries = data && data.get(dirKey);
        const entry = entries && entries.entries && entries.entries.find(e => (e.path || e.name) === targetPath);
        selectFile(targetPath, entry || null, 'rendered');
        // Fetch and render the file.
        const conn = getE2EE(filesChannelId);
        if (conn && conn.connected) {
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
          conn.fileRead(filesChannelId, targetPath);
        }
      }
    }
  }
  window.addEventListener('message', onMsg);

  // Clean up listener when content changes.
  const observer = new MutationObserver(() => {
    if (!fileContentBody.contains(wrapper)) {
      window.removeEventListener('message', onMsg);
      observer.disconnect();
    }
  });
  observer.observe(fileContentBody, { childList: true });

  // Load preview frame (has its own permissive CSP).
  iframe.src = '/preview-frame';
}

// ---- Diff Renderer ----

function renderDiffContent(diffText, truncated) {
  fileContentBody.style.padding = '';
  const lines = diffText.split('\n');
  const viewer = document.createElement('div');
  viewer.className = 'diff-viewer';

  // Derive file extension for syntax highlighting.
  const extMatch = filesCurrentPath ? filesCurrentPath.match(/\.([^./]+)$/) : null;
  const ext = extMatch ? extMatch[1].toLowerCase() : '';

  // Pre-parse lines into typed entries.
  const entries = [];
  let oldNum = 0, newNum = 0;
  for (const line of lines) {
    if (line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('---') || line.startsWith('+++')) continue;
    if (line.startsWith('@@')) {
      const m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) { oldNum = parseInt(m[1]); newNum = parseInt(m[2]); }
      entries.push({ type: 'hunk', text: line });
      continue;
    }
    if (line.startsWith('+')) {
      entries.push({ type: 'add', text: line.slice(1), num: String(newNum++) });
    } else if (line.startsWith('-')) {
      entries.push({ type: 'del', text: line.slice(1), num: String(oldNum++) });
    } else {
      entries.push({ type: 'ctx', text: line.startsWith(' ') ? line.slice(1) : line, num: String(newNum) });
      oldNum++; newNum++;
    }
  }

  function addRow(cls, num, html) {
    const row = document.createElement('div');
    row.className = 'diff-line ' + cls;
    const numEl = document.createElement('span');
    numEl.className = 'dl-num';
    numEl.textContent = num;
    const contentEl = document.createElement('span');
    contentEl.className = 'dl-content';
    contentEl.innerHTML = html;
    row.appendChild(numEl);
    row.appendChild(contentEl);
    viewer.appendChild(row);
  }

  for (let ei = 0; ei < entries.length; ei++) {
    const e = entries[ei];
    if (e.type === 'hunk') {
      const hdr = document.createElement('div');
      hdr.className = 'diff-hunk-header';
      hdr.textContent = e.text;
      viewer.appendChild(hdr);
      continue;
    }
    if (e.type === 'ctx') {
      addRow('context', e.num, highlightLine(e.text, ext));
      continue;
    }
    if (e.type === 'del') {
      const dels = [e];
      while (ei + 1 < entries.length && entries[ei + 1].type === 'del') dels.push(entries[++ei]);
      const adds = [];
      while (ei + 1 < entries.length && entries[ei + 1].type === 'add') adds.push(entries[++ei]);
      const pairs = Math.min(dels.length, adds.length);
      for (let pi = 0; pi < dels.length; pi++) {
        const html = pi < pairs ? wordDiffLine(dels[pi].text, adds[pi].text).oldHtml : highlightLine(dels[pi].text, ext);
        addRow('removed', dels[pi].num, html);
      }
      for (let ai = 0; ai < adds.length; ai++) {
        const html = ai < pairs ? wordDiffLine(dels[ai].text, adds[ai].text).newHtml : highlightLine(adds[ai].text, ext);
        addRow('added', adds[ai].num, html);
      }
      continue;
    }
    if (e.type === 'add') {
      addRow('added', e.num, highlightLine(e.text, ext));
    }
  }

  fileContentBody.innerHTML = '';
  fileContentBody.appendChild(viewer);

  if (truncated) {
    const note = document.createElement('div');
    note.style.cssText = 'padding:.5rem .75rem;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border)';
    note.textContent = 'Diff truncated';
    fileContentBody.appendChild(note);
  }
}

// ---- Syntax Highlighting (lightweight) ----

const SYNTAX_RULES = {
  js: [
    [/\b(const|let|var|function|return|if|else|for|while|class|import|export|from|default|async|await|new|this|throw|try|catch|finally|switch|case|break|continue|typeof|instanceof|in|of|yield|void|delete)\b/g, 'keyword'],
    [/(["'`])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  py: [
    [/\b(def|class|return|if|elif|else|for|while|import|from|as|with|try|except|finally|raise|yield|lambda|pass|break|continue|and|or|not|is|in|True|False|None|async|await|self)\b/g, 'keyword'],
    [/("""[\s\S]*?"""|'''[\s\S]*?'''|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/@\w+/g, 'func'],
  ],
  html: [
    [/<!--[\s\S]*?-->/g, 'comment'],
    [/(<\/?)([\w-]+)/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(\w+)=/g, 'attr'],
  ],
  css: [
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(\d+\.?\d*)(px|em|rem|%|vh|vw|s|ms)?\b/g, 'number'],
    [/([.#][\w-]+)/g, 'func'],
    [/\b(color|background|display|flex|grid|margin|padding|border|font|width|height|position|top|left|right|bottom|z-index|overflow|opacity|transition|transform)\b/g, 'keyword'],
  ],
  rs: [
    [/\b(fn|let|mut|const|pub|struct|enum|impl|trait|use|mod|crate|self|super|match|if|else|for|while|loop|return|break|continue|where|async|await|move|type|as|in|ref|unsafe|extern|dyn|static)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  go: [
    [/\b(func|var|const|type|struct|interface|map|chan|go|select|case|default|if|else|for|range|return|break|continue|switch|package|import|defer|nil|true|false|make|new|len|cap|append|copy|close|delete|panic|recover)\b/g, 'keyword'],
    [/(["'`])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  yaml: [
    [/#.*$/gm, 'comment'],
    [/^(\s*)([\w][\w.\-\/]*)(\s*:)/gm, 'attr'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(true|false|yes|no|null|~)\b/gi, 'keyword'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/(\$\w+|\$\{[^}]+\})/g, 'func'],
    [/^(\s*-)\s/gm, 'operator'],
  ],
  toml: [
    [/#.*$/gm, 'comment'],
    [/^\s*\[+[\w.\-"]+\]+/gm, 'type'],
    [/^(\s*)([\w][\w.\-]*)(\s*=)/gm, 'attr'],
    [/("""[\s\S]*?"""|'''[\s\S]*?'''|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g, 'string'],
    [/\b(true|false)\b/g, 'keyword'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}:\d{2})?/g, 'number'],
  ],
  sh: [
    [/#.*$/gm, 'comment'],
    [/\b(if|then|else|elif|fi|for|while|do|done|case|esac|in|function|return|exit|local|export|source|set|unset|readonly|declare|typeset|shift|eval|exec|trap)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/(\$\w+|\$\{[^}]+\}|\$\([^)]+\))/g, 'func'],
    [/\b(\d+)\b/g, 'number'],
    [/[|&;><]{1,2}/g, 'operator'],
  ],
  docker: [
    [/#.*$/gm, 'comment'],
    [/^(FROM|RUN|CMD|LABEL|EXPOSE|ENV|ADD|COPY|ENTRYPOINT|VOLUME|USER|WORKDIR|ARG|ONBUILD|STOPSIGNAL|HEALTHCHECK|SHELL|MAINTAINER|AS)\b/gmi, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/(\$\w+|\$\{[^}]+\})/g, 'func'],
    [/\b(\d+)\b/g, 'number'],
  ],
  c: [
    [/\b(auto|break|case|char|const|continue|default|do|double|else|enum|extern|float|for|goto|if|inline|int|long|register|restrict|return|short|signed|sizeof|static|struct|switch|typedef|union|unsigned|void|volatile|while|_Bool|_Complex|_Imaginary|bool|true|false|NULL|nullptr)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/#\s*(include|define|ifdef|ifndef|endif|if|else|elif|undef|pragma|error|warning)\b/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFlLuU]*|0x[\da-f]+[lLuU]*|0b[01]+[lLuU]*)\b/gi, 'number'],
    [/\b([A-Z][\w]*_[\w]*|[A-Z]{2,})\b/g, 'type'],
  ],
  cpp: [
    [/\b(alignas|alignof|auto|bool|break|case|catch|char|char8_t|char16_t|char32_t|class|concept|const|consteval|constexpr|constinit|continue|co_await|co_return|co_yield|decltype|default|delete|do|double|dynamic_cast|else|enum|explicit|export|extern|false|float|for|friend|goto|if|inline|int|long|mutable|namespace|new|noexcept|nullptr|operator|override|private|protected|public|register|requires|return|short|signed|sizeof|static|static_assert|static_cast|struct|switch|template|this|thread_local|throw|true|try|typedef|typeid|typename|union|unsigned|using|virtual|void|volatile|wchar_t|while)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/#\s*(include|define|ifdef|ifndef|endif|if|else|elif|undef|pragma|error|warning)\b/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFlLuU]*|0x[\da-f]+[lLuU]*|0b[01]+[lLuU]*)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  cs: [
    [/\b(abstract|as|base|bool|break|byte|case|catch|char|checked|class|const|continue|decimal|default|delegate|do|double|else|enum|event|explicit|extern|false|finally|fixed|float|for|foreach|goto|if|implicit|in|int|interface|internal|is|lock|long|namespace|new|null|object|operator|out|override|params|private|protected|public|readonly|ref|return|sbyte|sealed|short|sizeof|stackalloc|static|string|struct|switch|this|throw|true|try|typeof|uint|ulong|unchecked|unsafe|ushort|using|var|virtual|void|volatile|while|async|await|yield|record|init|required|global)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\$"[^"]*"/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/#\s*(if|else|elif|endif|region|endregion|define|undef|pragma|nullable)\b/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFdDmM]?|0x[\da-f]+[lLuU]*)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/\[\w+\]/g, 'attr'],
  ],
  bat: [
    [/\bREM\b.*$/gmi, 'comment'],
    [/^\s*::\s.*$/gm, 'comment'],
    [/\b(echo|set|if|else|goto|call|exit|for|in|do|not|exist|defined|errorlevel|pause|cls|rem|setlocal|endlocal|enabledelayedexpansion|pushd|popd|shift|start|choice|timeout)\b/gi, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/(%\w+%|%~?\d|!\w+!)/g, 'func'],
    [/\b(\d+)\b/g, 'number'],
    [/^\s*:\w+/gm, 'type'],
  ],
  java: [
    [/\b(abstract|assert|boolean|break|byte|case|catch|char|class|const|continue|default|do|double|else|enum|extends|final|finally|float|for|goto|if|implements|import|instanceof|int|interface|long|native|new|null|package|private|protected|public|return|short|static|strictfp|super|switch|synchronized|this|throw|throws|transient|try|void|volatile|while|true|false|var|record|sealed|permits|yield)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFdDlL]?|0x[\da-f]+[lL]?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  rb: [
    [/\b(def|class|module|if|elsif|else|unless|case|when|while|until|for|do|end|begin|rescue|ensure|raise|return|yield|block_given\?|require|require_relative|include|extend|attr_accessor|attr_reader|attr_writer|self|super|nil|true|false|and|or|not|in|then|puts|print|lambda|proc)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/(:\w+)/g, 'attr'],
    [/(@\w+)/g, 'func'],
  ],
  php: [
    [/\b(abstract|and|array|as|break|callable|case|catch|class|clone|const|continue|declare|default|do|echo|else|elseif|empty|enddeclare|endfor|endforeach|endif|endswitch|endwhile|enum|eval|exit|extends|final|finally|fn|for|foreach|function|global|goto|if|implements|include|include_once|instanceof|insteadof|interface|isset|list|match|namespace|new|null|or|print|private|protected|public|readonly|require|require_once|return|static|switch|this|throw|trait|try|unset|use|var|while|xor|yield|true|false|self)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/#.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\$\w+/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  swift: [
    [/\b(actor|associatedtype|async|await|break|case|catch|class|continue|default|defer|deinit|do|else|enum|extension|fallthrough|fileprivate|for|func|guard|if|import|in|init|inout|internal|is|let|nil|open|operator|private|protocol|public|repeat|rethrows|return|self|Self|static|struct|subscript|super|switch|throw|throws|try|typealias|var|weak|where|while|true|false|some|any)\b/g, 'keyword'],
    [/("""|"(?:[^"\\]|\\.)*")/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'attr'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  dart: [
    [/\b(abstract|as|assert|async|await|base|break|case|catch|class|const|continue|covariant|default|deferred|do|dynamic|else|enum|export|extends|extension|external|factory|false|final|finally|for|Function|get|hide|if|implements|import|in|interface|is|late|library|mixin|new|null|on|operator|part|required|rethrow|return|sealed|set|show|static|super|switch|sync|this|throw|true|try|typedef|var|void|when|while|with|yield)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'attr'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  scala: [
    [/\b(abstract|case|catch|class|def|do|else|enum|export|extends|extension|false|final|finally|for|forSome|given|if|implicit|import|lazy|match|new|null|object|override|package|private|protected|return|sealed|super|this|then|throw|trait|true|try|type|using|val|var|while|with|yield)\b/g, 'keyword'],
    [/("""|"(?:[^"\\]|\\.)*")/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'attr'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFdDlL]?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  lua: [
    [/\b(and|break|do|else|elseif|end|false|for|function|goto|if|in|local|nil|not|or|repeat|return|then|true|until|while)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/--\[\[[\s\S]*?\]\]/g, 'comment'],
    [/--.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
  ],
  sql: [
    [/\b(SELECT|FROM|WHERE|INSERT|INTO|UPDATE|SET|DELETE|CREATE|ALTER|DROP|TABLE|INDEX|VIEW|JOIN|INNER|LEFT|RIGHT|OUTER|FULL|CROSS|ON|AND|OR|NOT|IN|IS|NULL|AS|ORDER|BY|GROUP|HAVING|LIMIT|OFFSET|UNION|ALL|DISTINCT|EXISTS|BETWEEN|LIKE|CASE|WHEN|THEN|ELSE|END|BEGIN|COMMIT|ROLLBACK|TRANSACTION|PRIMARY|KEY|FOREIGN|REFERENCES|DEFAULT|CHECK|UNIQUE|CONSTRAINT|VALUES|COUNT|SUM|AVG|MIN|MAX|CASCADE|IF|FUNCTION|PROCEDURE|TRIGGER|GRANT|REVOKE|WITH|RECURSIVE|OVER|PARTITION|RANK|ROW_NUMBER|COALESCE|CAST|CONVERT)\b/gi, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/--.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
  ],
  r: [
    [/\b(if|else|repeat|while|function|for|in|next|break|TRUE|FALSE|NULL|Inf|NaN|NA|NA_integer_|NA_real_|NA_complex_|NA_character_|return|invisible|library|require|source|stop|warning|message|cat|print|paste|paste0|sprintf)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[iL]?)\b/gi, 'number'],
    [/<-|->|<<-|->>|%%|%in%|%\*%/g, 'operator'],
  ],
  perl: [
    [/\b(my|our|local|sub|if|elsif|else|unless|while|until|for|foreach|do|last|next|redo|return|use|require|package|BEGIN|END|die|warn|print|say|chomp|chop|push|pop|shift|unshift|sort|reverse|map|grep|join|split|open|close|read|write|defined|undef|exists|delete|ref|bless|tie|untie|eval|qw)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/(\$[\w:]+|@[\w:]+|%[\w:]+)/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/=~|!~|&&|\|\||\/\//g, 'operator'],
  ],
  elixir: [
    [/\b(def|defp|defmodule|defmacro|defmacrop|defstruct|defprotocol|defimpl|defguard|defdelegate|do|end|if|else|unless|case|cond|when|with|for|in|fn|raise|rescue|catch|after|try|receive|send|spawn|import|use|alias|require|true|false|nil|and|or|not|is_atom|is_binary|is_boolean|is_float|is_function|is_integer|is_list|is_map|is_nil|is_number|is_pid|is_tuple)\b/g, 'keyword'],
    [/("""|"(?:[^"\\]|\\.)*")/g, 'string'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/(:\w+)/g, 'attr'],
    [/@\w+/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/\|>/g, 'operator'],
  ],
  erlang: [
    [/\b(after|and|andalso|band|begin|bnot|bor|bsl|bsr|bxor|case|catch|div|end|fun|if|let|not|of|or|orelse|receive|rem|try|when|xor|true|false|undefined)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/%.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/\b(\w+):/g, 'func'],
  ],
  zig: [
    [/\b(align|allowzero|and|anyframe|anytype|asm|async|await|break|callconv|catch|comptime|const|continue|defer|else|enum|errdefer|error|export|extern|fn|for|if|inline|linksection|noalias|nosuspend|null|opaque|or|orelse|packed|pub|resume|return|struct|suspend|switch|test|threadlocal|true|false|try|undefined|union|unreachable|var|volatile|while)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  haskell: [
    [/\b(as|case|class|data|default|deriving|do|else|family|forall|foreign|hiding|if|import|in|infix|infixl|infixr|instance|let|module|newtype|of|qualified|then|type|where|True|False|Nothing|Just|Left|Right|IO|Maybe|Either|String|Int|Integer|Float|Double|Bool|Char)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/--.*$/gm, 'comment'],
    [/\{-[\s\S]*?-\}/g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/::|=>|->|<-|\.\./g, 'operator'],
  ],
  make: [
    [/#.*$/gm, 'comment'],
    [/^[\w.\-\/]+\s*[:+?]?=/gm, 'attr'],
    [/^[\w.\-\/\%]+\s*:/gm, 'type'],
    [/\$[\(\{][\w@<^+*?%]+[\)\}]/g, 'func'],
    [/\$[@<^+*?%]/g, 'func'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(ifeq|ifneq|ifdef|ifndef|else|endif|include|override|export|unexport|define|endef|vpath)\b/g, 'keyword'],
  ],
};

// Map file extensions to language.
const EXT_MAP = {
  js: 'js', jsx: 'js', ts: 'js', tsx: 'js', mjs: 'js', cjs: 'js',
  py: 'py', pyi: 'py',
  html: 'html', htm: 'html', xml: 'html', svg: 'html',
  css: 'css', scss: 'css', less: 'css',
  rs: 'rs',
  go: 'go',
  json: 'js', jsonc: 'js',
  sh: 'sh', bash: 'sh', zsh: 'sh',
  yml: 'yaml', yaml: 'yaml',
  toml: 'toml', ini: 'toml',
  c: 'c', h: 'c',
  cpp: 'cpp', cxx: 'cpp', cc: 'cpp', hpp: 'cpp', hxx: 'cpp', hh: 'cpp',
  cs: 'cs', csx: 'cs',
  java: 'java', kt: 'java', kts: 'java',
  rb: 'rb', rake: 'rb', gemspec: 'rb',
  bat: 'bat', cmd: 'bat',
  dockerfile: 'docker',
  php: 'php', phtml: 'php',
  swift: 'swift',
  dart: 'dart',
  scala: 'scala', sc: 'scala',
  lua: 'lua',
  sql: 'sql',
  r: 'r',
  pl: 'perl', pm: 'perl', perl: 'perl',
  ex: 'elixir', exs: 'elixir',
  erl: 'erlang', hrl: 'erlang',
  zig: 'zig',
  hs: 'haskell', lhs: 'haskell',
  makefile: 'make', mk: 'make',
};

function highlightLine(text, ext) {
  const lang = EXT_MAP[ext];
  const rules = lang ? SYNTAX_RULES[lang] : null;
  if (!rules || !text) return escHtml(text || '');

  // Tokenize: find all matches, sort by position, apply non-overlapping.
  const tokens = [];
  for (const [re, cls] of rules) {
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(text)) !== null) {
      tokens.push({ start: m.index, end: m.index + m[0].length, cls, text: m[0] });
    }
  }
  tokens.sort((a, b) => a.start - b.start || b.end - a.end);

  let result = '';
  let pos = 0;
  for (const tok of tokens) {
    if (tok.start < pos) continue; // overlapping, skip
    if (tok.start > pos) result += escHtml(text.slice(pos, tok.start));
    result += `<span class="tok-${tok.cls}">${escHtml(tok.text)}</span>`;
    pos = tok.end;
  }
  if (pos < text.length) result += escHtml(text.slice(pos));
  return result || '&nbsp;';
}

// ---- Load files on tab switch ----

function onFilesTabActivated() {
  if (!filesChannelId || filesChannelId !== chatCurrentChannel) {
    // Reset tree for new channel.
    fileTreeData.delete(filesChannelId);
    filesChangesData.delete(filesChannelId);
    filesCurrentPath = null;
    filesCurrentView = 'source';
    filesLastContent = null;
    filesCurrentHasDiff = false;
    filesCurrentIsMarkdown = false;
    filesCurrentIsSvg = false;
    filesCurrentIsHtml = false;
    filesPathText.textContent = 'No file selected';
    filesPathText.classList.add('empty');
    fileReloadBtn.classList.add('hc-hidden');
    fileFloatToggle.style.display = 'none';
    fileContentBody.innerHTML = '<div class="empty-state"><p>Select a file to view contents</p></div>';

    // Check for saved state to restore after tree loads.
    const saved = loadChannelState(chatCurrentChannel);
    if (saved.filesPath) {
      filesPendingRestore = saved.filesPath;
      filesPendingView = saved.filesView || 'source';
    } else {
      filesPendingRestore = null;
    }

    filesLoadRoot();
    const _ftaConn = getActiveE2EE();
    if (filesTreeTab === 'changes' && _ftaConn && _ftaConn.connected) {
      _ftaConn.filesChanges(chatCurrentChannel);
    }
  }
}

// Hook into tab switching — save active tab per channel.
const _origSwitchTab = switchTab;
switchTab = function(tab) {
  _origSwitchTab(tab);
  if (tab === 'files') onFilesTabActivated();
  if (chatCurrentChannel) saveChannelState(chatCurrentChannel, { activeTab: tab });
};

// ===== File Path Link Navigation =====
function navigateToFile(filePath) {
  // If the files tab is already showing this channel, just select the file.
  // Otherwise, set pending restore and switch tabs.
  filesPendingRestore = filePath;
  filesPendingView = 'source';
  switchTab('files');
  location.hash = 'files';
  // Also select immediately — works when tree is already loaded.
  if (filesChannelId) {
    selectFile(filePath, null, 'source');
  }
}

// Delegated click handler for file-path-link elements in messages.
document.addEventListener('click', function(e) {
  var link = e.target.closest('.file-path-link');
  if (!link) return;
  e.preventDefault();
  var path = link.dataset.filePath;
  if (path) navigateToFile(path);
});


// ---- Expose helpers needed by vendor/markdown.js ----
window.highlightLine = highlightLine;
