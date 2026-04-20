// Tab bar. Buttons drive the router; uiStore.tab drives which panel is
// visible via body[data-tab]. See planning/dashboard-v2/06-shell.md.

import { uiStore } from '../domain/ui-store.js';
import { router } from './router.js';

const TABS = [
  { id: 'files', label: 'Files' },
  { id: 'chat',  label: 'Chat'  },
  { id: 'terminal', label: 'Terminal' },
  { id: 'browser', label: 'Browser' },
];

export class TabBarView {
  constructor() {
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-tab-bar');
    if (!this.root) return;
    this.render();
    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'tab') this.render();
    }));
    this.root.addEventListener('click', this._onClick);
    this._syncBody();
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.root) this.root.removeEventListener('click', this._onClick);
    this.root = null;
  }

  render() {
    const active = uiStore.getTab();
    this.root.innerHTML = TABS.map(t => `
      <button class="v2-tab-btn ${t.id === active ? 'active' : ''}" data-tab="${t.id}" type="button">${t.label}</button>
    `).join('');
    this._syncBody();
  }

  _syncBody() {
    const t = uiStore.getTab() || 'files';
    document.body.dataset.tab = t;
  }

  _onClick = (e) => {
    const btn = e.target.closest('[data-tab]');
    if (!btn) return;
    const tab = btn.getAttribute('data-tab');
    router.navigate(tab, uiStore.getActiveChannel());
  };
}
