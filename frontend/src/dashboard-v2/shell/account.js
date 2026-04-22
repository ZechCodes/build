// AccountView — sidebar footer. Renders the signed-in user's avatar
// + name (links to /admin/), plus an "Update" button when the server
// reports a newer theme version than the one this page was loaded
// with.
//
// Version check runs once at mount and on every SSE reconnect. The
// first check on a fresh browser (no stored version) seeds
// localStorage with the server's version. Subsequent checks compare
// server to stored; if server is newer, we show the badge and leave
// stored alone — the badge persists across reconnects until the
// user clicks it (which reloads, re-seeding on the way in).

import { bus } from '../core/bus.js';
import { escapeHtml } from '../util/html.js';

const THEME_VERSION_KEY = 'build_theme_version';

function compareVersions(a, b) {
  const pa = String(a).split('.').map(Number);
  const pb = String(b).split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] || 0;
    const nb = pb[i] || 0;
    if (na !== nb) return na - nb;
  }
  return 0;
}

export class AccountView {
  constructor() {
    this.root = null;
    this._unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-account-slot');
    if (!this.root) return;
    this._render();
    this._unsubs.push(bus.on('sse.connected', () => this._checkThemeVersion()));
    // Probe once at mount so the first page load either seeds the
    // stored version (fresh install) or surfaces a stale-tab badge
    // right away. Subsequent reconnects re-check.
    this._checkThemeVersion();
  }

  deactivate() {
    this._unsubs.forEach(fn => fn());
    this._unsubs = [];
    if (this.root) this.root.innerHTML = '';
    this.root = null;
  }

  _render() {
    const name = document.body?.dataset?.userName || 'Account';
    const initial = name.trim().charAt(0).toUpperCase() || '?';
    this.root.innerHTML = `
      <a class="v2-account" href="/admin/" title="${escapeHtml(name)}">
        <span class="v2-account-avatar">${escapeHtml(initial)}</span>
        <span class="v2-account-name">${escapeHtml(name)}</span>
      </a>
      <div class="v2-account-update-slot" data-update-slot></div>
    `;
  }

  async _checkThemeVersion() {
    if (!this.root) return;
    const server = await this._fetchVersion();
    if (!server) return;
    const stored = localStorage.getItem(THEME_VERSION_KEY);
    if (!stored) {
      localStorage.setItem(THEME_VERSION_KEY, server);
      return;
    }
    if (compareVersions(server, stored) > 0) this._showUpdateBadge();
  }

  async _fetchVersion() {
    try {
      const res = await fetch('/api/theme/version', { cache: 'no-store' });
      if (!res.ok) return null;
      const { version } = await res.json();
      return version || null;
    } catch {
      return null;
    }
  }

  _showUpdateBadge() {
    const slot = this.root?.querySelector('[data-update-slot]');
    if (!slot || slot.querySelector('.v2-theme-update')) return;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'v2-theme-update';
    btn.title = 'New version available — click to reload';
    btn.setAttribute('aria-label', 'Update available, click to reload');
    btn.innerHTML = `
      <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
        <path d="M2 8a6 6 0 019.5-4.9M14 8a6 6 0 01-9.5 4.9M11.5 3.1v3h-3M4.5 12.9v-3h3"
              stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      <span>Update</span>
    `;
    btn.addEventListener('click', () => location.reload());
    slot.appendChild(btn);
  }
}
