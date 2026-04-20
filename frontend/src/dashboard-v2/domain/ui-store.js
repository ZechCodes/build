// Session-global UI state. Tab, rail, open dropdown, active channel, theme.
// Per-channel UI state lives on Channel.viewState (see 03-channel-lifecycle.md).

import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable('ui');

const state = {
  tab: 'files',           // 'files' | 'chat' | 'browser' | ...
  railState: 'collapsed', // 'collapsed' | 'open' | 'expanded'
  openDropdown: null,
  activeChannel: null,
  theme: 'dark',
};

export const uiStore = {
  getTab() { return state.tab; },
  setTab(tab) {
    if (state.tab === tab) return;
    state.tab = tab;
    notify({ kind: 'tab', tab });
  },

  getRailState() { return state.railState; },
  setRailState(railState) {
    if (state.railState === railState) return;
    state.railState = railState;
    notify({ kind: 'rail', railState });
  },

  getOpenDropdown() { return state.openDropdown; },
  setOpenDropdown(id) {
    if (state.openDropdown === id) return;
    state.openDropdown = id;
    notify({ kind: 'dropdown', id });
  },

  getActiveChannel() { return state.activeChannel; },
  setActiveChannel(id) {
    if (state.activeChannel === id) return;
    state.activeChannel = id;
    notify({ kind: 'active_channel', id });
  },

  getTheme() { return state.theme; },
  setTheme(theme) {
    if (state.theme === theme) return;
    state.theme = theme;
    notify({ kind: 'theme', theme });
  },

  subscribe,
};
