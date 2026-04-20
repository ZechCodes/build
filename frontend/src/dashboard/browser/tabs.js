import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { selectChannel } from '../channels/select.js';

// External deps kept on window during transition:
//   window.renderChannelPanel, window.showBrowserView

export function addBrowserTab(deviceId) {
  const id = 'browser-' + (++state._browserTabCounter);
  const tab = { id, url: 'http://localhost:', deviceId };
  if (!state.browserTabs.has(deviceId)) state.browserTabs.set(deviceId, []);
  state.browserTabs.get(deviceId).push(tab);
  state.activeBrowserTab = id;
  state.chatCurrentChannel = null;
  window.renderChannelPanel?.();
  window.showBrowserView?.(tab);
}

export function removeBrowserTab(deviceId, tabId) {
  const tabs = state.browserTabs.get(deviceId) || [];
  const idx = tabs.findIndex(t => t.id === tabId);
  if (idx !== -1) tabs.splice(idx, 1);
  if (state.activeBrowserTab === tabId) {
    state.activeBrowserTab = null;
    const firstCh = state.chatChannels.keys().next().value;
    if (firstCh) selectChannel(firstCh);
    else { document.getElementById('viewer-content')?.replaceChildren(); }
  }
  window.renderChannelPanel?.();
}

export function selectBrowserTab(tab) {
  state.activeBrowserTab = tab.id;
  state.chatCurrentChannel = null;
  window.renderChannelPanel?.();
  window.showBrowserView?.(tab);
}

export function createBrowserTabItem(tab, deviceId) {
  const item = document.createElement('div');
  item.className = 'browser-tab-item' + (state.activeBrowserTab === tab.id ? ' active' : '');
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
