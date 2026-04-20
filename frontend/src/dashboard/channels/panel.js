import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { addBrowserTab, createBrowserTabItem } from '../browser/tabs.js';

// External deps via window during transition:
//   window.selectChannel, window.anyE2EEConnected, window.showEditChannelDialog,
//   window.connectToDevice, window.updateChatRailUnreadBadge

export function renderChannelPanel() {
  const list = document.getElementById('channel-panel-list');
  if (!list) return;
  list.innerHTML = '';

  const sortedDevices = [...state.devices.values()].sort((a, b) => a.name.localeCompare(b.name));

  for (const device of sortedDevices) {
    const group = document.createElement('div');
    group.className = 'device-group';

    const hasConnection = state.e2eeConnections.has(device.id);
    const conn = state.e2eeConnections.get(device.id);
    const isE2eeConnected = hasConnection && conn && conn.connected;
    const isOnline = device.status === 'online';

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
        const channels = group.querySelector('.device-group-channels');
        if (channels) channels.classList.toggle('collapsed');
        header.querySelector('.device-group-chevron').classList.toggle('collapsed');
      } else if (isOnline && device.has_transport_key) {
        window.connectToDevice?.(device);
      }
    });
    group.appendChild(header);

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

    const hasCachedChannels = state.deviceChannels.has(device.id) && state.deviceChannels.get(device.id).size > 0;
    if (hasConnection || hasCachedChannels) {
      const channelsDiv = document.createElement('div');
      channelsDiv.className = 'device-group-channels';
      const devChans = state.deviceChannels.get(device.id) || new Map();
      const sorted = [...devChans.values()].sort((a, b) => {
        const aTs = state.channelSortTs.get(a.id) || 0;
        const bTs = state.channelSortTs.get(b.id) || 0;
        if (aTs !== bTs) return bTs - aTs;
        return (b.created_at || 0) - (a.created_at || 0);
      });
      for (const ch of sorted) {
        const item = createChannelItem(ch);
        channelsDiv.appendChild(item);
      }
      const tabs = state.browserTabs.get(device.id) || [];
      for (const tab of tabs) {
        const item = createBrowserTabItem(tab, device.id);
        channelsDiv.appendChild(item);
      }
      group.appendChild(channelsDiv);
    }

    list.appendChild(group);
  }

  updateMobileChannelLabel();
}

export function createChannelItem(ch) {
  const item = document.createElement('div');
  const isActive = ch.id === state.chatCurrentChannel;
  const uc = (typeof state.unreadCounts !== 'undefined') ? state.unreadCounts.get(ch.id) : null;
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
    window.selectChannel?.(ch.id);
  });
  const editBtn = item.querySelector('.ch-edit');
  if (editBtn) {
    editBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      window.showEditChannelDialog?.(ch);
    });
  }
  return item;
}

export function updateMobileChannelLabel() {
  const label = document.getElementById('mobile-channel-label');
  if (!label) return;
  const trigger = document.getElementById('mobile-channel-trigger');
  const hashEl = trigger?.querySelector('.dd-hash');
  if (state.chatCurrentChannel) {
    const ch = state.chatChannels.get(state.chatCurrentChannel);
    label.textContent = ch ? (ch.name || ch.id.slice(0, 8)) : 'Select channel';
    const isE2eeConnected = window.anyE2EEConnected?.();
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

  if (trigger && typeof state.unreadCounts !== 'undefined') {
    let totalUnread = 0;
    let anyInteraction = false;
    for (const [chId, uc] of state.unreadCounts) {
      if (chId === state.chatCurrentChannel) continue;
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

  window.updateChatRailUnreadBadge?.();
}
