// Minimal viewer header. Shows active channel name + account link slot.
// Model/effort pills come in a later wave once presenceStore + harness
// info have a home in the shell.

import { uiStore } from '../domain/ui-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { escapeHtml } from '../util/html.js';

export class TopBarView {
  constructor() {
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-top-bar');
    if (!this.root) return;
    this.render();
    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'active_channel') this.render();
    }));
    this.unsubs.push(channelsStore.subscribe(() => this.render()));
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.root = null;
  }

  render() {
    const id = uiStore.getActiveChannel();
    const ch = id ? channelsStore.get(id) : null;
    const name = ch?.name || (id ? id.slice(0, 8) : 'No channel');
    this.root.innerHTML = `
      <span class="v2-top-hash">#</span>
      <span class="v2-top-name">${escapeHtml(name)}</span>
    `;
  }
}
