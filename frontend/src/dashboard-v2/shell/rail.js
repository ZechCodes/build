// Bottom rail. Terminal + Chat toggles; rail state is driven by uiStore.
// See planning/dashboard-v2/06-shell.md.

import { uiStore } from '../domain/ui-store.js';
import { unreadStore } from '../domain/unread-store.js';

export class RailView {
  constructor() {
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-rail-controls');
    if (!this.root) return;
    this._updateActiveStates();
    this._updateActiveUnread();
    this._applyRailState();

    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'rail' || e.kind === 'rail_panel') this._applyRailState();
      if (e.kind === 'overlay') this._updateActiveStates();
      // Switching channels changes which unread count the rail shows.
      if (e.kind === 'active_channel') this._updateActiveUnread();
    }));
    this.unsubs.push(unreadStore.subscribe(() => this._updateActiveUnread()));

    this.root.addEventListener('click', this._onClick);
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) this.root.removeEventListener('click', this._onClick);
    this.root = null;
  }

  _applyRailState() {
    const s = uiStore.getRailState();
    const p = uiStore.getRailPanel();
    document.querySelector('.v2-app')?.setAttribute('data-rail', s);
    // Toggle active class on terminal button when its panel is showing.
    const tBtn = document.getElementById('v2-rail-terminal-toggle');
    if (tBtn) tBtn.classList.toggle('active', p === 'terminal');
    this._updateActiveStates();
  }

  _updateActiveStates() {
    const overlay = uiStore.getOverlay();
    const cBtn = document.getElementById('v2-rail-chat-toggle');
    if (cBtn) cBtn.classList.toggle('active', overlay.open);
  }

  _updateActiveUnread() {
    // The rail chat icon shows a badge only for the CURRENT channel —
    // other channels are surfaced via the Attention section in the
    // sidebar instead, so the rail stays quiet until there's something
    // to do right here.
    const el = document.getElementById('v2-rail-unread');
    if (!el) return;
    const channelId = uiStore.getActiveChannel();
    const slot = channelId ? unreadStore.get(channelId) : null;
    const count = slot?.count || 0;
    if (count > 0) {
      el.hidden = false;
      el.textContent = String(count);
      el.classList.toggle('has-interaction', !!slot?.hasInteraction);
    } else {
      el.hidden = true;
    }
  }

  _onClick = (e) => {
    const termBtn = e.target.closest('[data-rail-panel]');
    if (termBtn) {
      const panel = termBtn.getAttribute('data-rail-panel');
      uiStore.setRailPanel(uiStore.getRailPanel() === panel ? null : panel);
      return;
    }
    if (e.target.closest('#v2-rail-chat-toggle')) {
      uiStore.toggleOverlay();
    }
  };
}
