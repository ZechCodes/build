// Mirrors the active channel's lifecycle phase onto the DOM as a
// `data-channel-phase` attribute on `.v2-app`. CSS keys off this to
// show/hide panel loading skeletons and suppress "No messages yet"
// / "No activity yet" empty states while a channel is still loading.
//
// The full Channel phase set (unmounted / mounting / mounted / loading
// / ready / unloading) collapses to three UI phases:
//   idle    — no active channel. Panels are free to show their
//              "select a channel" state.
//   loading — a channel is mounting or fetching initial data.
//   ready   — the channel has data (or has timed out trying).

import { bus } from '../core/bus.js';
import { uiStore } from '../domain/ui-store.js';
import { channelRegistry } from '../channel/registry.js';

function toUiPhase(channelPhase, hasActive) {
  if (!hasActive) return 'idle';
  if (channelPhase === 'ready') return 'ready';
  return 'loading';
}

export class ChannelPhaseView {
  constructor() {
    this.root = null;
    this.unsubs = [];
    this._bound = false;
  }

  activate() {
    if (this._bound) return;
    this._bound = true;
    this.root = document.querySelector('.v2-app') || document.body;
    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'active_channel') this._apply();
    }));
    this.unsubs.push(bus.on('channel.phase', ({ channelId, phase }) => {
      // Only react when the event belongs to the currently active channel.
      if (channelId !== uiStore.getActiveChannel()) return;
      this._set(toUiPhase(phase, true));
    }));
    this._apply();
  }

  deactivate() {
    if (!this._bound) return;
    this._bound = false;
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.root?.removeAttribute('data-channel-phase');
    this.root = null;
  }

  _apply() {
    const id = uiStore.getActiveChannel();
    if (!id) return this._set('idle');
    const ch = channelRegistry.pool?.get(id);
    this._set(toUiPhase(ch?.phase, true));
  }

  _set(phase) {
    if (!this.root) return;
    this.root.setAttribute('data-channel-phase', phase);
  }
}
