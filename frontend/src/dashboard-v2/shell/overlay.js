// OverlayView — chat overlay header wiring (pin/expand/minimize) + state
// application on the DOM. The chat content itself is mounted by ChatView
// into #v2-chat-overlay-body.

import { uiStore } from '../domain/ui-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { escapeHtml } from '../util/html.js';

export class OverlayView {
  constructor() {
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-chat-overlay');
    if (!this.root) return;
    this._apply();
    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'overlay') this._apply();
      if (e.kind === 'active_channel') this._renderTitle();
    }));
    this.unsubs.push(channelsStore.subscribe(() => this._renderTitle()));
    this.root.addEventListener('click', this._onClick);
    this._renderTitle();
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) this.root.removeEventListener('click', this._onClick);
    this.root = null;
  }

  _apply() {
    const ov = uiStore.getOverlay();
    this.root.classList.toggle('open', ov.open);
    this.root.setAttribute('aria-hidden', ov.open ? 'false' : 'true');
    this.root.setAttribute('data-pinned', ov.pinned ? 'true' : 'false');
    this.root.setAttribute('data-mode', ov.mode || 'normal');
    document.querySelector('.v2-app')?.setAttribute('data-overlay', ov.open ? 'open' : 'closed');
    document.getElementById('v2-co-pin')?.classList.toggle('active', ov.pinned);
    document.getElementById('v2-co-expand')?.classList.toggle('active', ov.mode === 'expanded');
  }

  _renderTitle() {
    const el = document.getElementById('v2-co-channel-name');
    if (!el) return;
    const id = uiStore.getActiveChannel();
    const ch = id ? channelsStore.get(id) : null;
    el.textContent = ch?.name || (id ? id.slice(0, 8) : 'Select a channel');
  }

  _onClick = (e) => {
    if (e.target.closest('#v2-co-pin')) {
      uiStore.setOverlayPinned(!uiStore.getOverlay().pinned);
      return;
    }
    if (e.target.closest('#v2-co-expand')) {
      uiStore.setOverlayMode(uiStore.getOverlay().mode === 'expanded' ? 'normal' : 'expanded');
      return;
    }
    if (e.target.closest('#v2-co-minimize')) {
      uiStore.setOverlayOpen(false);
      return;
    }
  };
}
