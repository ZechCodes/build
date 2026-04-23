// OverlayView — chat overlay header wiring (pin/expand/minimize) + state
// application on the DOM. The chat content itself is mounted by ChatView
// into #v2-chat-overlay-body.

import { bus } from '../core/bus.js';
import { uiStore } from '../domain/ui-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { escapeHtml } from '../util/html.js';
import { showToast } from '../util/toast.js';

export class OverlayView {
  constructor() {
    this.root = null;
    this.unsubs = [];
    this._menuEl = null;
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
    // Document-level outside-click handler: closes the overlay when
    // it's open AND unpinned. Pinned stays modal-like. Attached once
    // and guarded inside the handler so we don't have to flip
    // listeners on every state change.
    document.addEventListener('click', this._onDocClick);
    this._renderTitle();
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) this.root.removeEventListener('click', this._onClick);
    document.removeEventListener('click', this._onDocClick);
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
    if (e.target.closest('#v2-co-menu')) {
      this._toggleChannelMenu();
      return;
    }
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

  _onDocClick = (e) => {
    // Channel menu: close on any outside click.
    if (this._menuEl && !e.target.closest('.v2-channel-menu, #v2-co-menu')) {
      this._closeChannelMenu();
    }
    const ov = uiStore.getOverlay();
    if (!ov.open || ov.pinned) return;
    // Any click inside the overlay, on the rail toggle (which would
    // re-open anyway), on the model picker popover, or on other
    // transient popovers (complication menu, image lightbox) must
    // NOT close.
    if (e.target.closest(
      '#v2-chat-overlay, #v2-rail-chat-toggle, .v2-model-picker, ' +
      '.v2-comp-menu, .v2-lightbox, .v2-channel-menu'
    )) return;
    uiStore.setOverlayOpen(false);
  };

  // ── Channel menu popover ─────────────────────────────────────────
  // Body-level popover (same pattern as model picker) with Restart
  // Agent / Stop Agent / Rename actions.

  _toggleChannelMenu() {
    if (this._menuEl) { this._closeChannelMenu(); return; }
    const btn = document.getElementById('v2-co-menu');
    const channelId = uiStore.getActiveChannel();
    if (!btn || !channelId) return;

    const menu = document.createElement('div');
    menu.className = 'v2-channel-menu';
    menu.innerHTML = `
      <button type="button" data-ch-action="restart">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <path d="M2 8a6 6 0 1110.4 4M13 3v3h-3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
        Restart agent
      </button>
      <button type="button" data-ch-action="stop">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <rect x="4" y="4" width="8" height="8" rx="1" stroke="currentColor" stroke-width="1.3"/>
        </svg>
        Stop agent
      </button>
      <div class="v2-channel-menu-sep"></div>
      <button type="button" data-ch-action="rename">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <path d="M11.5 3.5l1 1M2 14l4-1 7-7-3-3-7 7z" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
        Rename channel
      </button>
    `;
    document.body.appendChild(menu);
    this._menuEl = menu;

    const r = btn.getBoundingClientRect();
    // Place below + right-aligned to the button.
    const menuRect = menu.getBoundingClientRect();
    let top = r.bottom + 4;
    let left = r.right - menuRect.width;
    const margin = 8;
    if (left < margin) left = margin;
    if (top + menuRect.height > window.innerHeight - margin) {
      top = r.top - menuRect.height - 4;
    }
    menu.style.top  = `${Math.round(top)}px`;
    menu.style.left = `${Math.round(left)}px`;

    menu.addEventListener('click', this._onMenuClick);
  }

  _closeChannelMenu() {
    if (!this._menuEl) return;
    this._menuEl.removeEventListener('click', this._onMenuClick);
    this._menuEl.remove();
    this._menuEl = null;
  }

  _onMenuClick = (e) => {
    const btn = e.target.closest('[data-ch-action]');
    if (!btn) return;
    const action = btn.getAttribute('data-ch-action');
    const channelId = uiStore.getActiveChannel();
    if (!channelId) return;
    this._closeChannelMenu();
    if (action === 'restart') {
      bus.emit('intent.restart_agent', { channelId });
      showToast('Restarting agent…');
    } else if (action === 'stop') {
      bus.emit('intent.stop_agent', { channelId });
      showToast('Stopping agent…');
    } else if (action === 'rename') {
      this._beginRename(channelId);
    }
  };

  _beginRename(channelId) {
    const titleEl = document.querySelector('#v2-co-header .v2-co-title');
    const nameEl = document.getElementById('v2-co-channel-name');
    if (!titleEl || !nameEl) return;
    const ch = channelsStore.get(channelId);
    const current = ch?.name || '';

    // Replace the title span with an input. Save on Enter / blur,
    // cancel on Esc.
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'v2-co-rename-input';
    input.value = current;
    input.setAttribute('aria-label', 'Rename channel');
    const prevTitleHtml = titleEl.innerHTML;
    titleEl.innerHTML = '';
    titleEl.appendChild(input);
    input.focus();
    input.select();

    let done = false;
    const restore = () => {
      if (done) return;
      done = true;
      input.removeEventListener('blur', onBlur);
      titleEl.innerHTML = prevTitleHtml;
      this._renderTitle();
    };
    const commit = () => {
      if (done) return;
      const next = input.value.trim();
      if (next && next !== current) {
        bus.emit('intent.update_channel', { channelId, patch: { name: next } });
        showToast('Channel renamed');
      }
      restore();
    };
    const onBlur = () => commit();
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); commit(); }
      else if (e.key === 'Escape') { e.preventDefault(); restore(); }
    });
    input.addEventListener('blur', onBlur);
  }
}
