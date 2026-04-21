// Per-device reconnect pill. Shows at the bottom of the viewport
// when the ACTIVE channel's device is in a non-healthy E2EE state
// but SSE itself is alive (we defer to the Skrift SSE pill when
// SSE is the one that dropped — no point nagging about E2EE when
// the underlying stream is gone).
//
// States:
//   - idle      → hidden.
//   - retrying  → "Reconnecting to {device name}…"
//   - failed    → "Disconnected from {device name}" + Retry button.
//
// Visibility tracks uiStore.activeChannel + channelsStore.deviceFor
// + devicesStore + e2eePool.forDevice, and the
// `reconnect.state` / `sse.*` bus events.

import { bus } from '../core/bus.js';
import { uiStore } from '../domain/ui-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { devicesStore } from '../domain/devices-store.js';
import { e2eePool } from '../transport/e2ee-pool.js';
import { retryNow } from '../transport/self-heal.js';

export class ReconnectPillView {
  constructor() {
    this.root = null;
    this.unsubs = [];
    this._sseConnected = true;
    // deviceId → latest phase we heard on the bus.
    this._phase = new Map();
    this._bound = false;
  }

  activate() {
    if (this._bound) return;
    this._bound = true;
    this._buildRoot();
    // Re-render on any state change that could flip the pill.
    this.unsubs.push(uiStore.subscribe(e => {
      if (e.kind === 'active_channel') this.render();
    }));
    this.unsubs.push(devicesStore.subscribe(() => this.render()));
    this.unsubs.push(channelsStore.subscribe(() => this.render()));
    this.unsubs.push(bus.on('e2ee.connected',    () => this.render()));
    this.unsubs.push(bus.on('e2ee.disconnected', () => this.render()));
    this.unsubs.push(bus.on('reconnect.state',   (e) => {
      if (e?.deviceId) this._phase.set(e.deviceId, e.phase || 'idle');
      this.render();
    }));
    this.unsubs.push(bus.on('sse.connected',    () => {
      this._sseConnected = true; this.render();
    }));
    this.unsubs.push(bus.on('sse.disconnected', () => {
      this._sseConnected = false; this.render();
    }));
    this.render();
  }

  deactivate() {
    if (!this._bound) return;
    this._bound = false;
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
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
      if (e.target.closest('.v2-reconnect-pill-retry')) {
        const deviceId = this._currentDeviceId();
        if (deviceId) retryNow(deviceId);
      }
    });
  }

  _currentDeviceId() {
    const chId = uiStore.getActiveChannel();
    if (!chId) return null;
    return channelsStore.deviceFor(chId) || null;
  }

  render() {
    if (!this.root) return;
    // SSE is down — the SSE pill has the user's attention; hide
    // the E2EE pill entirely.
    if (!this._sseConnected) {
      return this._hide();
    }
    const chId = uiStore.getActiveChannel();
    if (!chId) return this._hide();
    const deviceId = channelsStore.deviceFor(chId);
    if (!deviceId) return this._hide();

    const device = devicesStore.get(deviceId);
    const conn = e2eePool.forDevice(deviceId);
    const connected = !!conn?.connected;
    const deviceOnline = device?.status === 'online';

    // Healthy → hidden.
    if (deviceOnline && connected) return this._hide();

    const deviceName = device?.name || deviceId.slice(0, 8);
    // Use the latest phase from the bus. Default to 'retrying' if
    // nothing's come in yet — self-heal will schedule a retry on
    // the next disconnected event.
    const current = this._phase.get(deviceId);
    const phase = current === 'failed' ? 'failed' : 'retrying';
    this._show(phase, deviceName);
  }

  _show(phase, deviceName) {
    const label = this.root.querySelector('.v2-reconnect-pill-label');
    const retry = this.root.querySelector('.v2-reconnect-pill-retry');
    label.textContent = phase === 'failed'
      ? `Disconnected from ${deviceName}`
      : `Reconnecting to ${deviceName}…`;
    retry.hidden = phase !== 'failed';
    this.root.classList.add('open');
    this.root.classList.toggle('failed', phase === 'failed');
    this.root.setAttribute('aria-hidden', 'false');
  }

  _hide() {
    this.root.classList.remove('open');
    this.root.classList.remove('failed');
    this.root.setAttribute('aria-hidden', 'true');
  }
}
