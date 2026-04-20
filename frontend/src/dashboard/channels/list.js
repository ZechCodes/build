import { state, SORT_STABILITY_MS } from '../state.js';
import { renderChannelPanel } from './panel.js';

// Rebuild the merged channel map from each device's per-device channels.
export function rebuildChatChannels() {
  state.chatChannels.clear();
  for (const [, channels] of state.deviceChannels) {
    for (const [chId, ch] of channels) state.chatChannels.set(chId, ch);
  }
}

// Bump a channel's sort timestamp so it floats to the top of the rail.
// Debounced: only promotes if the previous promote was more than
// SORT_STABILITY_MS ago — prevents noise from reshuffling the order.
export function promoteChannel(channelId) {
  const now = Date.now();
  const prev = state.channelSortTs.get(channelId) || 0;
  if (now - prev > SORT_STABILITY_MS) state.channelSortTs.set(channelId, now);
}

// Ordered list of channel IDs for keyboard navigation (Alt+Up/Down in Electron).
export function getOrderedChannelIds() {
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

// Append to channel history (Alt+[ / Alt+] navigation).
export function pushChannelHistory(channelId) {
  if (state._navigatingHistory) return;
  if (state.channelHistory[state.channelHistoryIndex] === channelId) return;
  state.channelHistory.splice(state.channelHistoryIndex + 1);
  state.channelHistory.push(channelId);
  if (state.channelHistory.length > 50) state.channelHistory.shift();
  state.channelHistoryIndex = state.channelHistory.length - 1;
}

// Back-compat shims — the panel is now the single source of truth.
export function renderChannelList() { renderChannelPanel(); }
export function renderChannelSidebar() { /* no-op, merged into renderChannelPanel */ }
export function updateAggregateBadge() { /* no-op, removed with top bar */ }

export function incrementUnread(channelId, isInteraction = false) {
  const uc = state.unreadCounts.get(channelId) || { messages: 0, hasInteraction: false };
  uc.messages++;
  if (isInteraction) uc.hasInteraction = true;
  state.unreadCounts.set(channelId, uc);
  renderChannelPanel();
}
