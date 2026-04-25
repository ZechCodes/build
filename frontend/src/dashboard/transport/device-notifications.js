// Translates Skrift `sk:notification` DOM events for `build:device:*`
// into v2 bus events so the sidebar reacts to live device-status
// changes (online / offline / heartbeat-missed) without a page
// reload.
//
// The devicesStore already binds `device.status_changed` and
// `device.bulk` — this module is just the publisher.

import { bus } from '../core/bus.js';
import { fetchDevices } from './rest.js';
import { log } from '../core/log.js';

const dlog = log('device-notif');

const FETCH_RETRY_DELAYS_MS = [2000, 4000, 8000];

/**
 * Fire-and-retry `fetchDevices()`. On failure, retry 3× with 2/4/8s
 * backoff. Logs + gives up silently if all retries fail — the next
 * sk:notification (or a full SSE reconnect) will trigger another
 * attempt anyway.
 */
async function fetchDevicesWithRetry(attempt = 0) {
  try {
    await fetchDevices();
  } catch (err) {
    if (attempt >= FETCH_RETRY_DELAYS_MS.length) {
      dlog.error('fetchDevices gave up after', attempt, 'retries', err);
      return;
    }
    const delay = FETCH_RETRY_DELAYS_MS[attempt];
    dlog.debug(`fetchDevices failed (attempt ${attempt + 1}); retrying in ${delay}ms`);
    setTimeout(() => fetchDevicesWithRetry(attempt + 1), delay);
  }
}

let bound = false;

export function bindDeviceNotifications() {
  if (bound) return;
  bound = true;
  document.addEventListener('sk:notification', _onNotification);
}

export function unbindDeviceNotifications() {
  if (!bound) return;
  bound = false;
  document.removeEventListener('sk:notification', _onNotification);
}

function _onNotification(e) {
  const data = e.detail || {};
  const type = data.type || '';
  if (!type.startsWith('build:device:')) return;
  // Suppress Skrift's default toast rendering for these lifecycle
  // events — the sidebar + Attention section already surface them.
  e.preventDefault?.();

  const eventType = type.slice('build:device:'.length);
  const deviceId = data.device_id;
  dlog.debug(eventType, deviceId);

  switch (eventType) {
    case 'online':
      if (deviceId) bus.emit('device.status_changed', { deviceId, status: 'online' });
      break;
    case 'offline':
    case 'heartbeat-missed':
      if (deviceId) bus.emit('device.status_changed', { deviceId, status: 'offline' });
      break;
    case 'authorized':
    case 'e2ee-ready':
    case 'renamed':
    case 'revoked':
      // Full record changes — re-fetch and let the bulk binding
      // reconcile the whole list. Retries with backoff so a
      // transient network blip on this API call doesn't leave the
      // sidebar stale.
      fetchDevicesWithRetry();
      break;
    default:
      dlog.debug('unhandled', type);
  }
}
