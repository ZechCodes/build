// Sidebar listing of devices and their channels. Pure view — reads
// from stores, dispatches intents, never imports transport.
// See planning/dashboard/05-views.md § ChannelPanelView.

import { bus } from '../core/bus.js';
import { devicesStore } from '../domain/devices-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { unreadStore } from '../domain/unread-store.js';
import { presenceStore } from '../domain/presence-store.js';
import { uiStore } from '../domain/ui-store.js';
import { router } from './router.js';
import { escapeHtml } from '../util/html.js';
import { openChannelMenu, openChannelNewModal } from './channel-menu.js';

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
    // Re-render tick — advances the Recent rows' relative
    // timestamps and evicts entries as they cross the 4hr/72hr
    // window boundaries without needing a user interaction.
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
    // 60s is plenty — Recent rows show minute/hour/day resolution
    // and the 4hr/72hr window boundaries are far apart enough that
    // a one-minute drift on row eviction is invisible.
    this._attentionTick = setInterval(() => this.render(), 60 * 1000);
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) {
      this.root.removeEventListener('click', this._onClick);
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

    // Recent section — two stacked groups under one header:
    //   • primary: channels active in the last 4hrs, oldest at top
    //     (newly-bumped channels append to the bottom, matching the
    //     pre-existing "stable while you work" ordering).
    //   • fallback: channels active 4–72hrs ago, newest at top
    //     (the entries you're most likely to want first when coming
    //     back after time off; oldest fall off the bottom).
    // The fallback only fills enough rows to bring the total to
    // RECENT_TARGET_COUNT, so the section caps at 10.
    const { primary, fallback } = buildAttentionList();
    if (primary.length || fallback.length) {
      const primaryRows = primary.map(a => renderAttentionRow(a, active)).join('');
      const fallbackRows = fallback.map(a => renderAttentionRow(a, active)).join('');
      const divider = primary.length && fallback.length
        ? '<div class="v2-attention-divider" aria-hidden="true"></div>'
        : '';
      parts.push(`
        <section class="v2-attention-section">
          <header class="v2-attention-header">Recent</header>
          <div class="v2-attention-rows">
            ${primaryRows}
            ${divider}
            ${fallbackRows}
          </div>
        </section>
      `);
    }
    for (const d of devices) {
      const channels = channelsStore.listByDevice(d.id)
        .slice()
        .sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
      const status = deviceStatus(d);
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
              <button class="v2-sidebar-new-session" type="button" data-new-session="${escapeHtml(d.id)}" title="New channel">
                <svg viewBox="0 0 12 12" fill="none"><path d="M6 2v8M2 6h8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
                New channel
              </button>
            </div>
          `}
          ${!isCollapsed && channels.length ? `<div class="v2-device-channels">${channels.map(ch => renderChannel(ch, active)).join('')}</div>` : ''}
        </div>
      `);
    }
    this.root.innerHTML = parts.join('');
  }

  _onClick = (e) => {
    const newBtn = e.target.closest('[data-new-session]');
    if (newBtn) {
      e.stopPropagation();
      e.preventDefault();
      const deviceId = newBtn.getAttribute('data-new-session');
      openChannelNewModal(deviceId);
      return;
    }
    const edit = e.target.closest('[data-channel-edit]');
    if (edit) {
      e.stopPropagation();
      e.preventDefault();
      const cid = edit.getAttribute('data-channel-edit');
      openChannelMenu(cid, edit);
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

const RECENT_PRIMARY_MS = 4  * 60 * 60 * 1000;   // 4 hours
const RECENT_FALLBACK_MS = 72 * 60 * 60 * 1000;  // 72 hours
const RECENT_TARGET_COUNT = 10;

/**
 * Bucket every known channel into the two Recent groups:
 *   - `primary`: `lastActiveAt` within the last 4hrs. Sorted by
 *     `sessionStartAt` (the timestamp of the first message after the
 *     most recent ≥4hr gap) ascending, so the channel whose current
 *     working session began earliest sits at the top. That anchor
 *     only moves on a fresh gap, so positions hold steady across the
 *     normal flow of messages. Channels seeded without history fall
 *     back to `lastActiveAt` as the sort key.
 *   - `fallback`: activity 4–72hrs old, sorted newest→oldest so the
 *     most likely "where was I" candidates sit on top; truncated to
 *     fill the remaining slots up to RECENT_TARGET_COUNT.
 *
 * Channels with no `lastActiveAt` are excluded from both groups —
 * they appear only in the per-device list below.
 */
export function buildAttentionList(now = Date.now()) {
  const primary = [];
  const fallback = [];
  for (const ch of channelsStore.list()) {
    const pres = presenceStore.get(ch.id);
    const last = pres.lastActiveAt || 0;
    if (!last) continue;
    const age = now - last;
    const unread = unreadStore.get(ch.id);
    const item = {
      ch,
      running: !!pres.agentActive,
      waitingInteraction: !!unread.hasInteraction,
      waitingUnread: (unread.count || 0) > 0,
      count: unread.count || 0,
      lastActiveAt: last,
      sessionStartAt: pres.sessionStartAt || 0,
    };
    if (age < RECENT_PRIMARY_MS) primary.push(item);
    else if (age < RECENT_FALLBACK_MS) fallback.push(item);
  }
  const sortKey = (item) => item.sessionStartAt || item.lastActiveAt;
  primary.sort((a, b) => sortKey(a) - sortKey(b));
  fallback.sort((a, b) => b.lastActiveAt - a.lastActiveAt);
  // Cap the section at RECENT_TARGET_COUNT total. When primary
  // overflows we keep the freshest entries (drop from the *top*,
  // since primary is oldest-first); when it doesn't, fallback fills
  // the remaining slots.
  if (primary.length > RECENT_TARGET_COUNT) {
    primary.splice(0, primary.length - RECENT_TARGET_COUNT);
  }
  const slots = Math.max(0, RECENT_TARGET_COUNT - primary.length);
  return { primary, fallback: fallback.slice(0, slots) };
}

function renderAttentionRow(item, activeId) {
  const { ch, running, waitingInteraction, waitingUnread, count, lastActiveAt } = item;
  const isActive = ch.id === activeId;
  const name = ch.name || (ch.id || '').slice(0, 8);
  const status = running            ? 'running'
               : waitingInteraction ? 'interaction'
               : waitingUnread      ? 'unread'
               : 'recent';
  const statusLabel = running            ? 'Running'
                    : waitingInteraction ? 'Needs you'
                    : waitingUnread      ? 'Unread'
                    : relativeAge(lastActiveAt);
  const badge = count > 0
    ? `<span class="v2-ch-unread-badge">${count}</span>`
    : '';
  const classes = [
    'v2-attention-row',
    'v2-channel-sidebar-item',
    isActive ? 'active' : '',
    !running && waitingInteraction ? 'has-interaction' : '',
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

function relativeAge(stampMs) {
  if (!stampMs) return '';
  const diff = Math.max(0, Date.now() - stampMs);
  const m = Math.floor(diff / 60000);
  if (m < 1) return 'just now';
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  return `${d}d`;
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
      <button class="v2-ch-edit" type="button" data-channel-edit="${escapeHtml(ch.id)}"
              title="Channel actions" aria-label="Channel actions">
        <svg viewBox="0 0 12 12" fill="none" aria-hidden="true"><circle cx="2.5" cy="6" r="1" fill="currentColor"/><circle cx="6" cy="6" r="1" fill="currentColor"/><circle cx="9.5" cy="6" r="1" fill="currentColor"/></svg>
      </button>
    </div>
  `;
}
