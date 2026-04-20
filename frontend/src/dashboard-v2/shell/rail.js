// Bottom rail. Minimal for Wave 3 — toggle state + aggregate unread badge.
// See planning/dashboard-v2/06-shell.md § Rail.

import { uiStore } from '../domain/ui-store.js';
import { unreadStore } from '../domain/unread-store.js';
import { escapeHtml } from '../util/html.js';

const CYCLE = { collapsed: 'open', open: 'expanded', expanded: 'collapsed' };

export class RailView {
  constructor() {
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-rail');
    if (!this.root) return;
    this.render();
    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'rail') this.render();
    }));
    this.unsubs.push(unreadStore.subscribe(() => this.render()));
    this.root.addEventListener('click', this._onClick);
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) this.root.removeEventListener('click', this._onClick);
    this.root = null;
  }

  render() {
    const state = uiStore.getRailState();
    const agg = unreadStore.aggregate();
    this.root.setAttribute('data-state', state);
    this.root.innerHTML = `
      <button class="v2-rail-toggle" type="button" title="${escapeHtml(state)}">
        <span class="v2-rail-icon" data-state="${state}"></span>
        <span class="v2-rail-label">${label(state)}</span>
      </button>
      ${agg.total > 0 ? `<span class="v2-rail-unread ${agg.anyInteraction ? 'has-interaction' : ''}">${agg.total}</span>` : ''}
    `;
    document.querySelector('.v2-app')?.setAttribute('data-rail', state);
  }

  _onClick = (e) => {
    if (!e.target.closest('.v2-rail-toggle')) return;
    const cur = uiStore.getRailState();
    uiStore.setRailState(CYCLE[cur] || 'collapsed');
  };
}

function label(state) {
  return state === 'collapsed' ? 'Activity' : state === 'open' ? 'Minimize' : 'Shrink';
}
