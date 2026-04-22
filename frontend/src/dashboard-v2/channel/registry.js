// One registry for the whole app. Pools Channel instances, activates one
// at a time based on uiStore.activeChannel, and evicts on bus
// channel.removed. LRU trims pool above MAX_POOL.
//
// See planning/dashboard-v2/03-channel-lifecycle.md.

import { bus } from '../core/bus.js';
import { uiStore } from '../domain/ui-store.js';
import { Channel } from './channel.js';
import { log } from '../core/log.js';

const plog = log('registry');
const MAX_POOL = 16;

export class ChannelRegistry {
  constructor({ ChannelCls = Channel, maxPool = MAX_POOL } = {}) {
    this._ChannelCls = ChannelCls;
    this._maxPool = maxPool;
    this.pool = new Map();     // channelId → Channel
    this.active = null;        // Channel | null
    this._bound = false;
  }

  init() {
    if (this._bound) return;
    this._bound = true;
    this.unsubUi = uiStore.subscribe(e => {
      if (e.kind === 'active_channel') this.activate(e.id);
    });
    this.unsubBus = bus.on('channel.removed', ({ channelId }) => this.evict(channelId));
    // Flush the active channel's viewState on page unload so the
    // user's tree / viewer / scroll state survives a reload. We also
    // persist in deactivate(), which handles the channel-switch case.
    this._onUnload = () => { this.active?.persistNow?.(); };
    if (typeof window !== 'undefined') {
      window.addEventListener('pagehide', this._onUnload);
      window.addEventListener('beforeunload', this._onUnload);
    }
    // Apply current state.
    const cur = uiStore.getActiveChannel();
    if (cur) this.activate(cur);
  }

  teardown() {
    if (!this._bound) return;
    this._bound = false;
    this.unsubUi?.(); this.unsubUi = null;
    this.unsubBus?.(); this.unsubBus = null;
    if (this._onUnload) {
      if (typeof window !== 'undefined') {
        window.removeEventListener('pagehide', this._onUnload);
        window.removeEventListener('beforeunload', this._onUnload);
      }
      this._onUnload = null;
    }
    this.deactivateAll();
  }

  activate(channelId) {
    if (!channelId) {
      if (this.active) { this.active.deactivate(); this.active = null; }
      return;
    }
    if (this.active?.id === channelId) return;

    let ch = this.pool.get(channelId);
    if (!ch) {
      ch = new this._ChannelCls(channelId);
      this.pool.set(channelId, ch);
    }
    if (this.active) this.active.deactivate();
    ch.activate();
    this.active = ch;
    this._enforceCap();
  }

  deactivateAll() {
    if (this.active) this.active.deactivate();
    this.active = null;
  }

  evict(channelId) {
    const ch = this.pool.get(channelId);
    if (!ch) return;
    if (this.active === ch) this.active = null;
    try { ch.destroy(); } catch (err) { plog.error('destroy failed', err); }
    this.pool.delete(channelId);
  }

  _enforceCap() {
    if (this.pool.size <= this._maxPool) return;
    const entries = [...this.pool.values()]
      .filter(c => c !== this.active)
      .sort((a, b) => (a.viewState?.lastActivatedAt || 0) - (b.viewState?.lastActivatedAt || 0));
    while (this.pool.size > this._maxPool && entries.length) {
      const victim = entries.shift();
      this.evict(victim.id);
    }
  }
}

export const channelRegistry = new ChannelRegistry();
