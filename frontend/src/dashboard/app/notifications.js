// Skrift `sk:notification` listener for build:device:* events from the relay.
// Distinct from E2EE-decrypted notifications (those flow through bindE2EEEvents).
// This module reacts to device lifecycle (online/offline, e2ee-ready, renamed,
// revoked, etc.) and triggers the appropriate UI / E2EE state updates.

import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { renderChannelPanel } from '../channels/panel.js';
import { connectDeviceE2EE, syncE2EEStatus, initE2EE } from '../e2ee/connect.js';
import { sseSynced } from '../net/notifications.js';
import { fetchDevices, isSseReconnecting } from './init.js';

document.addEventListener('sk:notification', (e) => {
  const data = e.detail;
  const type = data.type || '';

  if (!type.startsWith('build:device:')) return;

  // Prevent default toast rendering — we handle it ourselves.
  e.preventDefault();

  const eventType = type.replace('build:device:', '');
  const deviceId = data.device_id;
  const deviceName = data.device_name || '';

  switch (eventType) {
    case 'authorized': {
      // New device authorized — fetch fresh data to get full record.
      fetchDevices();
      break;
    }

    case 'online': {
      const device = state.devices.get(deviceId);
      if (device) {
        device.status = 'online';
        device.last_heartbeat_at = new Date().toISOString();
      } else {
        fetchDevices();
      }
      renderChannelPanel();
      // If this was the down device, clear banner (e2ee-ready handles reconnect).
      if (state.deviceDown && state.deviceDown.id === deviceId) {
        state.deviceDown = null;
        const banner = document.getElementById('device-down-banner');
        if (banner) banner.classList.remove('visible');
      }
      break;
    }

    case 'offline': {
      const device = state.devices.get(deviceId);
      if (device) device.status = 'offline';
      renderChannelPanel();
      const offlineConn = state.e2eeConnections.get(deviceId);
      if (offlineConn) {
        state.deviceDown = { id: deviceId, name: deviceName };
        const banner = document.getElementById('device-down-banner');
        const bannerText = document.getElementById('device-down-text');
        if (banner) banner.classList.add('visible');
        if (bannerText) bannerText.textContent = `${deviceName} disconnected — waiting for reconnection...`;
        offlineConn.disconnect(); // triggers disconnected handler which cleans up
        syncE2EEStatus();
      }
      break;
    }

    case 'heartbeat-missed': {
      // Refresh to get updated missed windows.
      fetchDevices();
      break;
    }

    case 'e2ee-ready': {
      // Device uploaded transport key — update device record and trigger E2EE connect.
      const dev = state.devices.get(deviceId);
      if (dev) {
        dev.has_transport_key = true;
        dev.status = 'online';
      }
      renderChannelPanel();
      if (state.deviceDown && state.deviceDown.id === deviceId) {
        state.deviceDown = null;
        const banner = document.getElementById('device-down-banner');
        if (banner) banner.classList.remove('visible');
      }
      // Skip replayed e2ee-ready events during SSE flush — the SSE reconnect
      // handler will call initE2EE() after sync anyway.
      if (!sseSynced()) {
        console.log('[E2EE] Suppressing replayed e2ee-ready (SSE not yet synced)');
        break;
      }
      // Skip if the SSE reconnect handler is already tearing down and reiniting.
      if (isSseReconnecting()) break;
      // Device re-announced — relay likely restarted, old session for this device is dead.
      const existingConn = state.e2eeConnections.get(deviceId);
      if (existingConn) {
        console.log('[E2EE] Device e2ee-ready while connected — relay restarted, reconnecting...');
        // Preserve current channel so it's restored after reconnect.
        if (state.chatCurrentChannel) state._pendingChannelId = state.chatCurrentChannel;
        existingConn.disconnect();
        state.e2eeConnections.delete(deviceId);
      }
      console.log('[E2EE] Device e2ee-ready notification — connecting...');
      connectDeviceE2EE(deviceId);
      break;
    }

    case 'renamed': {
      const device = state.devices.get(deviceId);
      if (device) device.name = deviceName;
      renderChannelPanel();
      break;
    }

    case 'revoked': {
      state.devices.delete(deviceId);
      const revokedConn = state.e2eeConnections.get(deviceId);
      if (revokedConn) revokedConn.disconnect();
      renderChannelPanel();
      break;
    }

    case 'status':
      break;

    default:
      console.log('Unhandled device event:', eventType, data);
  }
});
