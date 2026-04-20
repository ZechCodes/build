import { state } from '../state.js';
import { BuildE2EE } from '../vendor/e2ee.js';
import { anyE2EEConnected } from './bridge.js';
import { bindE2EEEvents } from './handlers.js';

// External deps via window during the transition:
//   window.fetchDevices — defined in legacy.js (will move to app/init.js in Wave D).

export function syncE2EEStatus() {
  const dot = document.getElementById('e2ee-dot');
  const label = document.getElementById('e2ee-label');
  if (!dot || !label) return;
  if (anyE2EEConnected()) {
    const count = [...state.e2eeConnections.values()].filter(c => c.connected).length;
    dot.className = 'e2ee-dot connected';
    label.textContent = count > 1 ? `E2EE active (${count} devices)` : 'E2EE active';
    document.getElementById('e2ee-waiting-overlay').classList.add('hidden');
  } else if (state.e2eeConnections.size > 0) {
    dot.className = 'e2ee-dot connecting';
    label.textContent = 'Connecting...';
  } else {
    dot.className = 'e2ee-dot connecting';
    label.textContent = 'Waiting for device...';
  }
}

export async function connectDeviceE2EE(deviceId) {
  if (state.e2eeConnections.has(deviceId)) return;
  console.log('[E2EE] Connecting to device', deviceId);

  const instance = new BuildE2EE();
  try {
    await instance.ready();
  } catch (err) {
    console.error('[E2EE] libsodium not available:', err);
    return;
  }

  bindE2EEEvents(instance, deviceId);
  state.e2eeConnections.set(deviceId, instance);
  syncE2EEStatus();

  try {
    await instance.connect(deviceId);
    console.log('[E2EE] Connected to', deviceId);
  } catch (err) {
    console.error('[E2EE] Failed to connect to', deviceId, err);
    instance.disconnect();
    state.e2eeConnections.delete(deviceId);
    syncE2EEStatus();
  }
}

export async function initE2EE(targetDeviceId = null) {
  console.log('[E2EE] Initializing...');

  if (typeof BuildE2EE === 'undefined') {
    const label = document.getElementById('e2ee-label');
    const dot = document.getElementById('e2ee-dot');
    if (label) label.textContent = 'E2EE script not loaded';
    if (dot) dot.className = 'e2ee-dot error';
    console.error('[E2EE] BuildE2EE class not found — e2ee.js failed to load');
    return;
  }

  if (targetDeviceId) {
    await connectDeviceE2EE(targetDeviceId);
    return;
  }

  const candidates = [...state.devices.values()].filter(d => d.status === 'online' && d.has_transport_key);
  if (candidates.length === 0) {
    console.log('[E2EE] No device ready, waiting for e2ee-ready notification');
    syncE2EEStatus();
    const skelText = document.querySelector('.skel-status-text');
    if (skelText) skelText.textContent = 'Waiting for device…';
    // Retry: device may not have sent e2ee-ready yet after SSE reconnect.
    clearTimeout(initE2EE._retryTimer);
    initE2EE._retryTimer = setTimeout(async () => {
      if (state.e2eeConnections.size > 0) return;
      console.log('[E2EE] Retrying — refreshing devices...');
      await window.fetchDevices?.();
      if (state.e2eeConnections.size === 0) initE2EE();
    }, 3000);
    return;
  }
  clearTimeout(initE2EE._retryTimer);

  await Promise.allSettled(candidates.map(d => connectDeviceE2EE(d.id)));
}
