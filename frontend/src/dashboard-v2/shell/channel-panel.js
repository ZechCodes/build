// Sidebar listing of devices and their channels. Pure view — reads
// from stores, dispatches intents, never imports transport.
// See planning/dashboard-v2/05-views.md § ChannelPanelView.

import { bus } from '../core/bus.js';
import { devicesStore } from '../domain/devices-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { unreadStore } from '../domain/unread-store.js';
import { presenceStore } from '../domain/presence-store.js';
import { uiStore } from '../domain/ui-store.js';
import { router } from './router.js';
import { escapeHtml } from '../util/html.js';

function statusIcon(status) {
  if (status === 'connected') return '<svg class="v2-status-lock" viewBox="0 0 16 16" fill="none"><path d="M4 7V5a4 4 0 118 0v2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><rect x="3" y="7" width="10" height="7" rx="1.5" fill="currentColor"/></svg>';
  if (status === 'connecting') return '<span class="v2-status-dot connecting" aria-label="Connecting"></span>';
  if (status === 'offline')    return '<span class="v2-status-dot offline" aria-label="Offline"></span>';
  return '<span class="v2-status-dot" aria-label="Online"></span>';
}

export class ChannelPanelView {
  constructor() {
    this.root = null;
    this.unsubs = [];
    // Per-device transient state: which device group is showing its
    // inline "new session" form right now.
    this._newSessionForDevice = null;
    // Re-render tick — keeps the Attention section's "recent" grace
    // window accurate and lets relative timestamps (e.g., "3m")
    // advance without a user interaction.
    this._attentionTick = null;
  }

  activate() {
    this.root = document.getElementById('v2-channel-panel-list');
    if (!this.root) return;
    this.render();
    const resub = () => this.render();
    this.unsubs.push(devicesStore.subscribe(resub));
    this.unsubs.push(channelsStore.subscribe(resub));
    this.unsubs.push(unreadStore.subscribe(resub));
    this.unsubs.push(presenceStore.subscribe(e => {
      // Re-render on anything that changes Attention membership —
      // agent_active flips the "Running" row; last_active seeds
      // the "recent" grace window from history.
      if (e.kind === 'agent_active' || e.kind === 'last_active') this.render();
    }));
    this.unsubs.push(uiStore.subscribe(e => { if (e.kind === 'active_channel') this.render(); }));
    this.root.addEventListener('click', this._onClick);
    this.root.addEventListener('keydown', this._onKeydown);
    this.root.addEventListener('submit', this._onSubmit);
    // 60s is plenty — the grace window is an hour and the relative
    // timestamps in the recent-attention rows advance in minutes.
    this._attentionTick = setInterval(() => this.render(), 60 * 1000);
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) {
      this.root.removeEventListener('click', this._onClick);
      this.root.removeEventListener('keydown', this._onKeydown);
      this.root.removeEventListener('submit', this._onSubmit);
    }
    if (this._attentionTick) { clearInterval(this._attentionTick); this._attentionTick = null; }
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

    // Attention section — lists every channel that's either running
    // (agent is processing) or waiting (unread messages, or a pending
    // interaction: plan review / tool approval / question).
    const attention = buildAttentionList();
    if (attention.length) {
      parts.push(`
        <section class="v2-attention-section">
          <header class="v2-attention-header">Attention</header>
          <div class="v2-attention-rows">
            ${attention.map(a => renderAttentionRow(a, active)).join('')}
          </div>
        </section>
      `);
    }
    for (const d of devices) {
      const channels = channelsStore.listByDevice(d.id)
        .slice()
        .sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
      const status = deviceStatus(d);
      const showForm = this._newSessionForDevice === d.id;
      // Collapsed state: user override wins; otherwise default to
      // collapsed if the device isn't online. Chevron + children are
      // hidden so the group reads as one line.
      const isCollapsed = isDeviceCollapsed(d.id, status);
      const groupClasses = ['v2-device-group', isCollapsed ? 'collapsed' : ''].filter(Boolean).join(' ');
      const headerClasses = ['v2-device-header', isCollapsed ? 'collapsed' : ''].filter(Boolean).join(' ');
      parts.push(`
        <div class="${groupClasses}" data-device-id="${escapeHtml(d.id)}">
          <header class="${headerClasses}" data-device-toggle="${escapeHtml(d.id)}" data-device-status="${status}">
            <svg class="v2-device-chevron" viewBox="0 0 12 12" fill="none" aria-hidden="true"><path d="M4 2l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
            <span class="v2-device-group-status">${statusIcon(status)}</span>
            <span class="v2-device-name">${escapeHtml(d.name || d.id)}</span>
          </header>
          ${isCollapsed ? '' : `
            <div class="v2-device-group-actions">
              <button class="v2-sidebar-new-session" type="button" data-new-session="${escapeHtml(d.id)}" title="New session">
                <svg viewBox="0 0 12 12" fill="none"><path d="M6 2v8M2 6h8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
                New Session
              </button>
            </div>
          `}
          ${showForm && !isCollapsed ? `
            <form class="v2-new-session-form" data-new-session-form="${escapeHtml(d.id)}">
              <input type="text" name="name" class="v2-new-session-input" placeholder="Session name…" autofocus autocomplete="off">
              <button type="submit" class="v2-new-session-go">Create</button>
              <button type="button" class="v2-new-session-cancel" data-new-session-cancel="${escapeHtml(d.id)}">Cancel</button>
            </form>
          ` : ''}
          ${!isCollapsed && channels.length ? `<div class="v2-device-channels">${channels.map(ch => renderChannel(ch, active)).join('')}</div>` : ''}
        </div>
      `);
    }
    this.root.innerHTML = parts.join('');
    // Focus the new-session input if one is open.
    if (this._newSessionForDevice) {
      this.root.querySelector('.v2-new-session-input')?.focus();
    }
  }

  _onClick = (e) => {
    const newBtn = e.target.closest('[data-new-session]');
    if (newBtn) {
      const deviceId = newBtn.getAttribute('data-new-session');
      this._newSessionForDevice = this._newSessionForDevice === deviceId ? null : deviceId;
      this.render();
      return;
    }
    const cancel = e.target.closest('[data-new-session-cancel]');
    if (cancel) {
      this._newSessionForDevice = null;
      this.render();
      return;
    }
    const edit = e.target.closest('[data-channel-edit]');
    if (edit) {
      e.stopPropagation();
      // Edit dialog deferred; no-op for now.
      return;
    }
    const deviceToggle = e.target.closest('[data-device-toggle]');
    if (deviceToggle
        && !e.target.closest('.v2-device-group-actions')
        && !e.target.closest('.v2-new-session-form')) {
      const deviceId = deviceToggle.getAttribute('data-device-toggle');
      const status = deviceToggle.getAttribute('data-device-status');
      // Persist the user's override so it survives reloads and wins
      // over the status-based default on next render.
      const nextCollapsed = !isDeviceCollapsed(deviceId, status);
      localStorage.setItem(deviceCollapsedKey(deviceId), nextCollapsed ? '1' : '0');
      this.render();
      return;
    }
    const channelRow = e.target.closest('[data-channel-id]');
    if (channelRow) {
      const channelId = channelRow.getAttribute('data-channel-id');
      router.navigate(uiStore.getTab() || 'files', channelId);
    }
  };

  _onKeydown = (e) => {
    if (e.key === 'Escape' && e.target.closest('.v2-new-session-form')) {
      this._newSessionForDevice = null;
      this.render();
    }
  };

  _onSubmit = (e) => {
    const form = e.target.closest('[data-new-session-form]');
    if (!form) return;
    e.preventDefault();
    const deviceId = form.getAttribute('data-new-session-form');
    const input = form.querySelector('.v2-new-session-input');
    const name = (input?.value || '').trim();
    if (!name) return;
    bus.emit('intent.create_channel', { deviceId, name });
    this._newSessionForDevice = null;
    this.render();
  };
}

function deviceStatus(device) {
  if (device.status !== 'online') return 'offline';
  if (!device.has_transport_key) return 'online';
  return 'online';
}

const DEVICE_COLLAPSED_KEY_PREFIX = 'v2.device.';
function deviceCollapsedKey(deviceId) {
  return `${DEVICE_COLLAPSED_KEY_PREFIX}${deviceId}.collapsed`;
}

/**
 * Resolve a device's current collapsed state. Respects the user's
 * persisted toggle if one exists; otherwise defaults by live status
 * (offline → collapsed, online → expanded).
 */
function isDeviceCollapsed(deviceId, status) {
  const raw = localStorage.getItem(deviceCollapsedKey(deviceId));
  if (raw === '1') return true;
  if (raw === '0') return false;
  return status !== 'online';
}

const ATTENTION_RECENT_WINDOW_MS = 60 * 60 * 1000;  // 1 hour

/**
 * Walk every known channel, and return those that either:
 *   - have their agent actively processing (running), or
 *   - have unread messages / a pending interaction (waiting), or
 *   - finished running within the last hour (recent grace window).
 * Sorted so the loudest signal floats to the top.
 */
function buildAttentionList() {
  const now = Date.now();
  const items = [];
  for (const ch of channelsStore.list()) {
    const pres = presenceStore.get(ch.id);
    const unread = unreadStore.get(ch.id);
    const running = !!pres.agentActive;
    const waitingInteraction = !!unread.hasInteraction;
    const waitingUnread = (unread.count || 0) > 0;
    const recent = !running
                && !waitingInteraction
                && !waitingUnread
                && pres.lastActiveAt > 0
                && (now - pres.lastActiveAt) < ATTENTION_RECENT_WINDOW_MS;
    if (!running && !waitingInteraction && !waitingUnread && !recent) continue;
    items.push({
      ch, running, waitingInteraction, waitingUnread, recent,
      count: unread.count || 0,
      lastActiveAt: pres.lastActiveAt || 0,
    });
  }
  items.sort((a, b) => {
    // Interaction > unread > running > recent. Within the same
    // category, fall back to channel name for stability.
    const score = (x) =>
      (x.waitingInteraction ? 8 : 0) +
      (x.waitingUnread      ? 4 : 0) +
      (x.running            ? 2 : 0) +
      (x.recent             ? 1 : 0);
    const d = score(b) - score(a);
    if (d) return d;
    // Within "recent" specifically, sort most-recent-first.
    if (a.recent && b.recent) return b.lastActiveAt - a.lastActiveAt;
    return (a.ch.name || '').localeCompare(b.ch.name || '');
  });
  return items;
}

function renderAttentionRow(item, activeId) {
  const { ch, running, waitingInteraction, waitingUnread, recent, count, lastActiveAt } = item;
  const isActive = ch.id === activeId;
  const name = ch.name || (ch.id || '').slice(0, 8);
  const status = waitingInteraction ? 'interaction'
               : waitingUnread      ? 'unread'
               : running            ? 'running'
               : 'recent';
  const statusLabel = waitingInteraction ? 'Needs you'
                    : waitingUnread      ? 'Unread'
                    : running            ? 'Running'
                    : recent             ? relativeMinutes(lastActiveAt)
                    : '';
  const badge = count > 0
    ? `<span class="v2-ch-unread-badge">${count}</span>`
    : '';
  const classes = [
    'v2-attention-row',
    'v2-channel-sidebar-item',
    isActive ? 'active' : '',
    waitingInteraction ? 'has-interaction' : '',
    `status-${status}`,
  ].filter(Boolean).join(' ');
  return `
    <div class="${classes}" data-channel-id="${escapeHtml(ch.id)}">
      <span class="v2-attention-indicator" data-indicator="${status}" aria-label="${statusLabel}"></span>
      <span class="v2-ch-hash">#</span>
      <span class="v2-ch-name">${escapeHtml(name)}</span>
      ${badge}
      <span class="v2-attention-status">${statusLabel}</span>
    </div>
  `;
}

function relativeMinutes(stampMs) {
  if (!stampMs) return '';
  const diff = Math.max(0, Date.now() - stampMs);
  const m = Math.floor(diff / 60000);
  if (m < 1) return 'just now';
  if (m === 1) return '1m';
  return `${m}m`;
}

function renderChannel(ch, activeId) {
  const unread = unreadStore.get(ch.id);
  const count = unread?.count || 0;
  const hasInteraction = !!unread?.hasInteraction;
  const name = ch.name || (ch.id || '').slice(0, 8);
  const isActive = ch.id === activeId;
  const classes = [
    'v2-channel-sidebar-item',
    isActive ? 'active' : '',
    hasInteraction ? 'has-interaction' : '',
  ].filter(Boolean).join(' ');
  const badge = count > 0 ? `<span class="v2-ch-unread-badge">${count}</span>` : '';
  return `
    <div class="${classes}" data-channel-id="${escapeHtml(ch.id)}">
      <span class="v2-ch-hash">#</span>
      <span class="v2-ch-name">${escapeHtml(name)}</span>
      ${badge}
      <button class="v2-ch-edit" type="button" data-channel-edit="${escapeHtml(ch.id)}" title="Edit">
        <svg viewBox="0 0 12 12" fill="none" aria-hidden="true"><path d="M8.5 1.5l2 2M1 11l.7-2.8L9 1l2 2-7.2 7.2L1 11z" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"/></svg>
      </button>
    </div>
  `;
}
