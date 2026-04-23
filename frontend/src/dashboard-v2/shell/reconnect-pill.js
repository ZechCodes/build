// Per-device connection pill. Purely reactive to the sessionStore.
//
// States (all derived by sessionStore; this view only renders):
//   - ready        → hidden.
//   - offline_sse  → hidden. Skrift's native SSE pill has the user's
//                     attention.
//   - booting      → hidden. No visible noise until the session has
//                     actually observed something.
//   - connecting   → "Connecting to {device}…"
//   - degraded     → "Reconnecting to {device}…"
//   - + failed flag on the active device → "Disconnected from {device}"
//     with a Retry button that kicks self-heal.

import { sessionStore } from '../core/session-store.js';
import { devicesStore } from '../domain/devices-store.js';
import { retryNow } from '../transport/self-heal.js';

export class ReconnectPillView {
  constructor() {
    this.root = null;
    this.unsub = null;
    this._bound = false;
  }

  activate() {
    if (this._bound) return;
    this._bound = true;
    this._buildRoot();
    this.unsub = sessionStore.subscribe(() => this.render());
    this.render();
  }

  deactivate() {
    if (!this._bound) return;
    this._bound = false;
    this.unsub?.(); this.unsub = null;
    this.root?.remove();
    this.root = null;
  }

  _buildRoot() {
    const host = document.createElement('div');
    host.className = 'v2-reconnect-pill';
    host.setAttribute('aria-hidden', 'true');
    host.setAttribute('role', 'status');
    host.innerHTML = `
      <span class="v2-reconnect-pill-dot" aria-hidden="true"></span>
      <span class="v2-reconnect-pill-label"></span>
      <button class="v2-reconnect-pill-retry" type="button" hidden>Retry</button>
    `;
    document.body.appendChild(host);
    this.root = host;
    this.root.addEventListener('click', (e) => {
      if (!e.target.closest('.v2-reconnect-pill-retry')) return;
      const deviceId = sessionStore.getSnapshot().activeDeviceId;
      if (deviceId) retryNow(deviceId);
    });
  }

  render() {
    if (!this.root) return;
    const snap = sessionStore.getSnapshot();
    const { phase, activeDeviceId, failed } = snap;

    if (phase === 'ready' || phase === 'offline_sse' || phase === 'booting') {
      return this._hide();
    }
    if (!activeDeviceId) return this._hide();

    const device = devicesStore.get(activeDeviceId);
    const deviceName = device?.name || activeDeviceId.slice(0, 8);

    if (failed) return this._show('failed', deviceName);
    if (phase === 'degraded') return this._show('reconnecting', deviceName);
    return this._show('connecting', deviceName);
  }

  _show(kind, deviceName) {
    const label = this.root.querySelector('.v2-reconnect-pill-label');
    const retry = this.root.querySelector('.v2-reconnect-pill-retry');
    if (kind === 'failed') {
      label.textContent = `Disconnected from ${deviceName}`;
      retry.hidden = false;
    } else if (kind === 'reconnecting') {
      label.textContent = `Reconnecting to ${deviceName}…`;
      retry.hidden = true;
    } else {
      label.textContent = `Connecting to ${deviceName}…`;
      retry.hidden = true;
    }
    this.root.classList.add('open');
    this.root.classList.toggle('failed', kind === 'failed');
    this.root.setAttribute('aria-hidden', 'false');
  }

  _hide() {
    this.root.classList.remove('open');
    this.root.classList.remove('failed');
    this.root.setAttribute('aria-hidden', 'true');
  }
}
