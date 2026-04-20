import { CHANNEL_STATE_KEY } from '../state.js';

// Per-channel localStorage persistence — draft / activeTab / planMode etc.
// Data shape: { [channelId]: { draft, activeTab, planMode, filesPath, filesView, ... } }

export function loadChannelState(channelId) {
  try {
    const data = JSON.parse(localStorage.getItem(CHANNEL_STATE_KEY) || '{}');
    return data[channelId] || {};
  } catch { return {}; }
}

export function saveChannelState(channelId, patch) {
  try {
    const data = JSON.parse(localStorage.getItem(CHANNEL_STATE_KEY) || '{}');
    data[channelId] = { ...(data[channelId] || {}), ...patch };
    localStorage.setItem(CHANNEL_STATE_KEY, JSON.stringify(data));
  } catch {}
}

export function clearChannelDraft(channelId) {
  try {
    const data = JSON.parse(localStorage.getItem(CHANNEL_STATE_KEY) || '{}');
    if (data[channelId]) {
      delete data[channelId].draft;
      localStorage.setItem(CHANNEL_STATE_KEY, JSON.stringify(data));
    }
  } catch {}
}
