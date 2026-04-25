// One registry for the whole app. Pools Channel instances, activates
// one at a time based on uiStore.activeChannel, and evicts on bus
// `channel.removed`. LRU trims pool above MAX_POOL.
//
// activate() is asynchronous: it awaits the previous channel's
// unload() before mounting the next. A serialization token
// (_latestRequestedId) makes rapid activate(A) → activate(B) →
// activate(A) converge on the last request.

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
    this.pool = new Map();
    this.active = null;
    this._latestRequestedId = null;
    this._bound = false;
  }

  init() {
    if (this._bound) return;
    this._bound = true;
    this.unsubUi = uiStore.subscribe(e => {
      if (e.kind === 'active_channel') {
        // Fire-and-forget — activate is async now but uiStore subscribers
        // can't await. Re-entrancy is handled by _latestRequestedId.
        this.activate(e.id).catch(err => plog.error('activate failed', err));
      }
    });
    this.unsubBus = bus.on('channel.removed', ({ channelId }) => {
      this.evict(channelId).catch(err => plog.error('evict failed', err));
    });
    // Flush the active channel's viewState on page unload so the
    // user's tree / viewer / scroll state survives a reload. We also
    // persist in unload(), which handles the channel-switch case.
    this._onUnload = () => { this.active?.persistNow?.(); };
    if (typeof window !== 'undefined') {
      window.addEventListener('pagehide', this._onUnload);
      window.addEventListener('beforeunload', this._onUnload);
    }
    const cur = uiStore.getActiveChannel();
    if (cur) {
      this.activate(cur).catch(err => plog.error('initial activate failed', err));
    }
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
    return this.deactivateAll();
  }

  /**
   * Activate `channelId` (or deactivate all with null). Always awaits
   * the previous channel's unload() before mounting the next.
   *
   * Re-entrant: if another activate() is called while we're awaiting,
   * the final `_latestRequestedId` wins — earlier awaits bail out on
   * return and do nothing visible.
   */
  async activate(channelId) {
    if (this.active?.id === channelId && this._latestRequestedId === channelId) return;

    this._latestRequestedId = channelId;

    const prev = this.active;
    this.active = null;

    if (prev) {
      try { await prev.unload(); }
      catch (err) { plog.error('previous unload failed', err); }
    }

    if (this._latestRequestedId !== channelId) return;

    if (!channelId) return;

    let ch = this.pool.get(channelId);
    if (!ch) {
      ch = new this._ChannelCls(channelId);
      this.pool.set(channelId, ch);
    }

    this.active = ch;
    ch.mount();
    // load() is intentionally not awaited by the registry — panels
    // subscribe to channel.phase for loading UI.
    ch.load().catch(err => {
      if (err?.name !== 'AbortError') plog.error('channel load failed', err);
    });
    this._enforceCap();
  }

  async deactivateAll() {
    this._latestRequestedId = null;
    if (this.active) {
      const prev = this.active;
      this.active = null;
      try { await prev.unload(); }
      catch (err) { plog.error('deactivate unload failed', err); }
    }
  }

  async evict(channelId) {
    const ch = this.pool.get(channelId);
    if (!ch) return;
    if (this.active === ch) {
      this.active = null;
      this._latestRequestedId = null;
    }
    try { await ch.destroy(); }
    catch (err) { plog.error('destroy failed', err); }
    this.pool.delete(channelId);
  }

  _enforceCap() {
    if (this.pool.size <= this._maxPool) return;
    const entries = [...this.pool.values()]
      .filter(c => c !== this.active)
      .sort((a, b) => (a.viewState?.lastActivatedAt || 0) - (b.viewState?.lastActivatedAt || 0));
    while (this.pool.size > this._maxPool && entries.length) {
      const victim = entries.shift();
      // Fire-and-forget — eviction doesn't need to block activate().
      this.evict(victim.id).catch(err => plog.error('evict failed', err));
    }
  }
}

export const channelRegistry = new ChannelRegistry();
