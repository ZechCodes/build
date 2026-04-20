// Sidebar listing of devices and their channels. Pure view — reads
// from stores, dispatches intents, never imports transport.
// See planning/dashboard-v2/05-views.md § ChannelPanelView.

import { bus } from '../core/bus.js';
import { devicesStore } from '../domain/devices-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { unreadStore } from '../domain/unread-store.js';
import { uiStore } from '../domain/ui-store.js';
import { router } from './router.js';
import { escapeHtml } from '../util/html.js';

const STATUS_DOT = {
  connected: '<span class="v2-dot v2-dot-lock" aria-label="E2EE connected">🔒</span>',
  connecting: '<span class="v2-dot v2-dot-connecting" aria-label="Connecting"></span>',
  online: '<span class="v2-dot v2-dot-online" aria-label="Online"></span>',
  offline: '<span class="v2-dot v2-dot-offline" aria-label="Offline"></span>',
};

export class ChannelPanelView {
  constructor() {
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-channel-panel-list');
    if (!this.root) return;
    this.render();
    const resub = () => this.render();
    this.unsubs.push(devicesStore.subscribe(resub));
    this.unsubs.push(channelsStore.subscribe(resub));
    this.unsubs.push(unreadStore.subscribe(resub));
    this.unsubs.push(uiStore.subscribe(e => { if (e.kind === 'active_channel') this.render(); }));
    // Delegated clicks for all interactive rows.
    this.root.addEventListener('click', this._onClick);
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) this.root.removeEventListener('click', this._onClick);
    this.root = null;
  }

  render() {
    if (!this.root) return;
    const devices = devicesStore.list().slice().sort((a, b) => (a.name || '').localeCompare(b.name || ''));
    const active = uiStore.getActiveChannel();
    if (devices.length === 0) {
      this.root.innerHTML = '<div class="v2-empty">No devices.</div>';
      return;
    }
    const parts = [];
    for (const d of devices) {
      const channels = channelsStore.listByDevice(d.id);
      const status = deviceStatus(d);
      parts.push(`
        <div class="v2-device-group" data-device-id="${escapeHtml(d.id)}">
          <header class="v2-device-header" data-device-toggle="${escapeHtml(d.id)}" data-device-status="${status}">
            ${STATUS_DOT[status]}
            <span class="v2-device-name">${escapeHtml(d.name || d.id)}</span>
          </header>
          ${channels.length ? `<div class="v2-device-channels">${channels.map(ch => renderChannel(ch, active)).join('')}</div>` : ''}
        </div>
      `);
    }
    this.root.innerHTML = parts.join('');
  }

  _onClick = (e) => {
    const deviceToggle = e.target.closest('[data-device-toggle]');
    if (deviceToggle) {
      const deviceId = deviceToggle.getAttribute('data-device-toggle');
      const status = deviceToggle.getAttribute('data-device-status');
      if (status === 'online') {
        bus.emit('intent.connect_device', { deviceId });
      }
      return;
    }
    const channelRow = e.target.closest('[data-channel-id]');
    if (channelRow) {
      const channelId = channelRow.getAttribute('data-channel-id');
      router.navigate(uiStore.getTab() || 'chat', channelId);
    }
  };
}

function deviceStatus(device) {
  // 'connected' means E2EE connected; distinguishing connecting vs online
  // requires reading e2eePool. We keep the dot as 'online' when the device
  // is reachable but not yet E2EE-connected. E2EE status updates arrive
  // via the bus and channels-by-device list grows once connected.
  if (device.status !== 'online') return 'offline';
  if (!device.has_transport_key) return 'online';
  // Heuristic: if channels exist for the device, we assume E2EE connected.
  // This gets replaced by presence-tracked e2ee status in a later polish.
  return 'online';
}

function renderChannel(ch, activeId) {
  const unread = unreadStore.get(ch.id);
  const count = unread?.count || 0;
  const hasInteraction = !!unread?.hasInteraction;
  const name = ch.name || (ch.id || '').slice(0, 8);
  const isActive = ch.id === activeId;
  const classes = [
    'v2-channel-row',
    isActive ? 'active' : '',
    hasInteraction ? 'has-interaction' : '',
  ].filter(Boolean).join(' ');
  const badge = count > 0 ? `<span class="v2-unread-badge">${count}</span>` : '';
  return `
    <button class="${classes}" data-channel-id="${escapeHtml(ch.id)}" type="button">
      <span class="v2-ch-hash">#</span>
      <span class="v2-ch-name">${escapeHtml(name)}</span>
      ${badge}
    </button>
  `;
}
