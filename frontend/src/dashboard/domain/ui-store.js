// Session-global UI state. Tab, rail, open dropdown, active channel, theme,
// chat overlay state, rail panel. Per-channel UI state lives on
// Channel.viewState (see planning/dashboard/03-channel-lifecycle.md).

import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable('ui');

const OVERLAY_KEY = 'v2.overlay';
const RAIL_KEY = 'v2.rail';

function loadOverlayState() {
  try {
    const raw = localStorage.getItem(OVERLAY_KEY);
    if (!raw) return null;
    const s = JSON.parse(raw);
    if (s && typeof s === 'object') return s;
  } catch (_) { /* ignore */ }
  return null;
}
function loadRailState() {
  try {
    const raw = localStorage.getItem(RAIL_KEY);
    if (!raw) return null;
    const s = JSON.parse(raw);
    if (s && typeof s === 'object') return s;
  } catch (_) { /* ignore */ }
  return null;
}
function persistOverlay(s) {
  try { localStorage.setItem(OVERLAY_KEY, JSON.stringify(s)); } catch (_) {}
}
function persistRail(s) {
  try { localStorage.setItem(RAIL_KEY, JSON.stringify(s)); } catch (_) {}
}

const persistedOverlay = typeof localStorage !== 'undefined' ? loadOverlayState() : null;
const persistedRail = typeof localStorage !== 'undefined' ? loadRailState() : null;

const state = {
  tab: 'files',           // legacy; retained for router compatibility
  railState: (persistedRail?.state) || 'collapsed', // 'collapsed' | 'open' | 'expanded'
  railPanel: persistedRail?.panel || null,          // 'terminal' | null
  openDropdown: null,
  activeChannel: null,
  theme: 'dark',
  chatOverlay: {
    open:   persistedOverlay?.open ?? false,
    pinned: persistedOverlay?.pinned ?? true,
    mode:   persistedOverlay?.mode || 'normal',    // 'normal' | 'expanded'
  },
};

function persistOverlayNow() {
  persistOverlay({ ...state.chatOverlay });
}
function persistRailNow() {
  persistRail({ state: state.railState, panel: state.railPanel });
}

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
    persistRailNow();
    notify({ kind: 'rail', railState });
  },

  getRailPanel() { return state.railPanel; },
  setRailPanel(panel) {
    if (state.railPanel === panel) return;
    state.railPanel = panel;
    // Panel drives rail state: showing a panel opens the rail.
    if (panel && state.railState === 'collapsed') state.railState = 'open';
    if (!panel && state.railState === 'open') state.railState = 'collapsed';
    persistRailNow();
    notify({ kind: 'rail_panel', panel });
    notify({ kind: 'rail', railState: state.railState });
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

  // ----- Chat overlay -----
  getOverlay() { return { ...state.chatOverlay }; },

  setOverlayOpen(open) {
    const v = !!open;
    if (state.chatOverlay.open === v) return;
    state.chatOverlay.open = v;
    persistOverlayNow();
    notify({ kind: 'overlay', overlay: { ...state.chatOverlay } });
  },

  setOverlayPinned(pinned) {
    const v = !!pinned;
    if (state.chatOverlay.pinned === v) return;
    state.chatOverlay.pinned = v;
    persistOverlayNow();
    notify({ kind: 'overlay', overlay: { ...state.chatOverlay } });
  },

  setOverlayMode(mode) {
    if (state.chatOverlay.mode === mode) return;
    state.chatOverlay.mode = mode;
    persistOverlayNow();
    notify({ kind: 'overlay', overlay: { ...state.chatOverlay } });
  },

  toggleOverlay() {
    this.setOverlayOpen(!state.chatOverlay.open);
  },

  subscribe,
};
