// ComplicationsView — shell view rendering complication chips above the
// viewer. Reads complicationsStore + uiStore.activeChannel.

import { bus } from '../core/bus.js';
import { complicationsStore } from '../domain/complications-store.js';
import { uiStore } from '../domain/ui-store.js';
import { escapeHtml } from '../util/html.js';

export class ComplicationsView {
  constructor() {
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-complications');
    if (!this.root) return;
    this.render();
    this.unsubs.push(uiStore.subscribe(e => { if (e.kind === 'active_channel') this.render(); }));
    this.unsubs.push(complicationsStore.subscribe(e => {
      if (e.channelId === uiStore.getActiveChannel()) this.render();
    }));
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
    const channelId = uiStore.getActiveChannel();
    const comps = channelId ? complicationsStore.forChannel(channelId) : [];
    if (!comps.length) {
      this.root.classList.add('empty');
      this.root.innerHTML = '';
      return;
    }
    this.root.classList.remove('empty');
    const sorted = comps.slice().sort((a, b) => (b.timestamp || 0) - (a.timestamp || 0));
    this.root.innerHTML = sorted.map(c => this._renderChip(c)).join('');
  }

  _renderChip(comp) {
    if (comp.kind === 'git-status') return this._renderGitChip(comp);
    return `<div class="v2-comp v2-comp-generic">${escapeHtml(comp.kind || 'unknown')}</div>`;
  }

  _renderGitChip(comp) {
    const d = comp.data || {};
    const branch = escapeHtml(d.branch || '?');
    const remote = d.remote_name ? escapeHtml(d.remote_name) : '';
    const ins = d.insertions || 0;
    const del = d.deletions || 0;
    const stats = [];
    if (ins) stats.push(`<span class="v2-comp-stat v2-comp-add">+${ins}</span>`);
    if (del) stats.push(`<span class="v2-comp-stat v2-comp-del">-${del}</span>`);
    if (d.ahead)     stats.push(`<span class="v2-comp-stat">↑${d.ahead}</span>`);
    if (d.behind)    stats.push(`<span class="v2-comp-stat">↓${d.behind}</span>`);
    if (d.conflicts) stats.push(`<span class="v2-comp-stat v2-comp-del">⚠${d.conflicts}</span>`);
    const label = remote ? `${remote}/${branch}` : branch;
    const options = Array.isArray(comp.options) ? comp.options : [];
    const actions = options.map(o => `
      <button class="v2-comp-action" type="button"
              data-comp-id="${escapeHtml(comp.id)}"
              data-action="${escapeHtml(o.id)}"
              ${o.enabled === false ? 'disabled' : ''}>${escapeHtml(o.label)}</button>
    `).join('');
    return `
      <div class="v2-comp v2-comp-git" data-comp-id="${escapeHtml(comp.id)}">
        <svg class="v2-comp-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
          <path d="M6 3v10M10 3v4"/><circle cx="6" cy="14" r="1.5"/><circle cx="10" cy="8" r="1.5"/>
        </svg>
        <span class="v2-comp-label">${escapeHtml(label)}</span>
        <span class="v2-comp-stats">${stats.join('')}</span>
        ${actions ? `<span class="v2-comp-actions">${actions}</span>` : ''}
      </div>
    `;
  }

  _onClick = (e) => {
    const btn = e.target.closest('[data-action]');
    if (!btn || btn.disabled) return;
    const channelId = uiStore.getActiveChannel();
    const complicationId = btn.getAttribute('data-comp-id');
    const action = btn.getAttribute('data-action');
    btn.disabled = true;
    btn.textContent = `${btn.textContent}…`;
    bus.emit('intent.resolve_complication', { channelId, complicationId, action });
  };
}
