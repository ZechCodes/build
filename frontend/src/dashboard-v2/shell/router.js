// Hash-based router. See planning/dashboard-v2/06-shell.md.
//
// Hash format: `#tab/channelId`  (channelId optional)
// Valid tabs: files, chat, browser.
//
// Router owns the channel history stack so v1's state.channelHistory and
// _navigatingHistory have no equivalent in v2 — they live here.

import { uiStore } from '../domain/ui-store.js';
import { log } from '../core/log.js';

const plog = log('router');
const VALID_TABS = new Set(['files', 'browser']);
// Legacy tabs that collapse to 'files' for backward-compatible URLs.
const LEGACY_MAP = { chat: 'files', terminal: 'files' };

export class Router {
  constructor() {
    this.history = [];
    this.index = -1;
    this._bound = false;
  }

  init() {
    if (this._bound) return;
    this._bound = true;
    window.addEventListener('hashchange', () => this._onHashChange());
    this._onHashChange();
  }

  parse(hash) {
    const raw = (hash || '').replace(/^#/, '');
    if (!raw) return { tab: null, channelId: null };
    const [tabRaw, channelId] = raw.split('/');
    const tab = VALID_TABS.has(tabRaw)
      ? tabRaw
      : (LEGACY_MAP[tabRaw] || null);
    return { tab, channelId: channelId || null };
  }

  navigate(tab, channelId) {
    if (!VALID_TABS.has(tab)) {
      plog.warn('invalid tab', tab);
      return;
    }
    const hash = channelId ? `${tab}/${channelId}` : tab;
    if (window.location.hash !== `#${hash}`) {
      window.location.hash = hash;
      // hashchange will fire _onHashChange which calls this again; guard
      // by checking whether state already matches. Applying eagerly here
      // avoids a tick of visual lag.
    }
    this._apply(tab, channelId, true);
  }

  back() {
    if (this.index <= 0) return;
    this.index -= 1;
    this._applyFromHistory();
  }

  forward() {
    if (this.index >= this.history.length - 1) return;
    this.index += 1;
    this._applyFromHistory();
  }

  _applyFromHistory() {
    const entry = this.history[this.index];
    if (!entry) return;
    this._apply(entry.tab, entry.channelId, false);
    const hash = entry.channelId ? `${entry.tab}/${entry.channelId}` : entry.tab;
    if (window.location.hash !== `#${hash}`) window.location.hash = hash;
  }

  _onHashChange() {
    const { tab, channelId } = this.parse(window.location.hash);
    if (!tab) return;
    this._apply(tab, channelId, true);
  }

  _apply(tab, channelId, push) {
    uiStore.setTab(tab);
    uiStore.setActiveChannel(channelId || null);
    if (push) this._push(tab, channelId);
  }

  _push(tab, channelId) {
    const top = this.history[this.index];
    if (top && top.tab === tab && top.channelId === channelId) return;
    this.history = this.history.slice(0, this.index + 1);
    this.history.push({ tab, channelId });
    this.index = this.history.length - 1;
  }
}

export const router = new Router();
