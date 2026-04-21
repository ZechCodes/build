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
    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'active_channel') this.render();
      // Re-render on dropdown open/close so the popover shows/hides.
      else if (e.kind === 'dropdown') this.render();
    }));
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
    // When an upstream is tracked, always show ahead/behind (even 0)
    // so the user can tell at a glance whether the branch is in sync.
    if (d.upstream) {
      stats.push(`<span class="v2-comp-stat v2-comp-ahead">↑${d.ahead || 0}</span>`);
      stats.push(`<span class="v2-comp-stat v2-comp-behind">↓${d.behind || 0}</span>`);
    }
    if (d.conflicts) stats.push(`<span class="v2-comp-stat v2-comp-del">⚠${d.conflicts}</span>`);
    const label = remote ? `${remote}/${branch}` : branch;
    const dropdownId = `comp:${comp.id}`;
    const isOpen = uiStore.getOpenDropdown() === dropdownId;
    const menu = isOpen ? this._renderGitMenu(comp) : '';
    return `
      <div class="v2-comp v2-comp-git${isOpen ? ' open' : ''}"
           data-comp-id="${escapeHtml(comp.id)}"
           data-dropdown-trigger="${escapeHtml(dropdownId)}">
        <svg class="v2-comp-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
          <path d="M6 3v10M10 3v4"/><circle cx="6" cy="14" r="1.5"/><circle cx="10" cy="8" r="1.5"/>
        </svg>
        <span class="v2-comp-label">${escapeHtml(label)}</span>
        <span class="v2-comp-stats">${stats.join('')}</span>
        ${menu}
      </div>
    `;
  }

  _renderGitMenu(comp) {
    const d = comp.data || {};
    const staged = d.staged || {};
    const unstaged = d.unstaged || {};
    const fileBreak = (s) => `
      <span class="v2-comp-stat v2-comp-add">+${s.added || 0}</span>
      <span class="v2-comp-stat">~${s.modified || 0}</span>
      <span class="v2-comp-stat v2-comp-del">-${s.deleted || 0}</span>
    `;
    const branchRow = `
      <div class="v2-comp-menu-section">
        <div class="v2-comp-menu-label">Branch</div>
        <div class="v2-comp-menu-row">
          <span>${escapeHtml(d.branch || '?')}</span>
          ${d.upstream ? `<span class="muted">→ ${escapeHtml(d.upstream)}</span>` : ''}
        </div>
      </div>
    `;
    const stagedRow = `
      <div class="v2-comp-menu-section">
        <div class="v2-comp-menu-label">Staged</div>
        <div class="v2-comp-menu-row">${fileBreak(staged)}</div>
      </div>
    `;
    const unstagedRow = `
      <div class="v2-comp-menu-section">
        <div class="v2-comp-menu-label">Unstaged</div>
        <div class="v2-comp-menu-row">${fileBreak(unstaged)}</div>
      </div>
    `;
    const untrackedRow = `
      <div class="v2-comp-menu-section">
        <div class="v2-comp-menu-label">Untracked</div>
        <div class="v2-comp-menu-row"><span class="v2-comp-stat">${d.untracked || 0}</span></div>
      </div>
    `;
    const remoteRow = d.upstream ? `
      <div class="v2-comp-menu-section">
        <div class="v2-comp-menu-label">Remote</div>
        <div class="v2-comp-menu-row">
          <span class="v2-comp-stat v2-comp-ahead">↑${d.ahead || 0} ahead</span>
          <span class="v2-comp-stat v2-comp-behind">↓${d.behind || 0} behind</span>
        </div>
      </div>
    ` : '';
    let lastFetchRow = '';
    if (d.last_fetch) {
      const ago = Math.round((Date.now() / 1000 - d.last_fetch));
      const agoStr = ago < 60 ? `${ago}s ago`
                   : ago < 3600 ? `${Math.round(ago / 60)}m ago`
                   : `${Math.round(ago / 3600)}h ago`;
      lastFetchRow = `
        <div class="v2-comp-menu-section">
          <div class="v2-comp-menu-label">Last fetch</div>
          <div class="v2-comp-menu-row muted">${escapeHtml(agoStr)}</div>
        </div>
      `;
    }
    const options = Array.isArray(comp.options) ? comp.options : [];
    const actions = options.length ? `
      <div class="v2-comp-menu-actions">
        ${options.map(o => `
          <button class="v2-comp-action" type="button"
                  data-comp-id="${escapeHtml(comp.id)}"
                  data-action="${escapeHtml(o.id)}"
                  ${o.enabled === false ? 'disabled' : ''}>${escapeHtml(o.label)}</button>
        `).join('')}
      </div>
    ` : '';
    return `
      <div class="v2-comp-menu" data-dropdown="comp:${escapeHtml(comp.id)}">
        ${branchRow}
        ${stagedRow}
        ${unstagedRow}
        ${untrackedRow}
        ${remoteRow}
        ${lastFetchRow}
        ${actions}
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
    // The document-level dropdown listener will close the menu next
    // because the click bubbles up from inside the trigger.
  };
}
