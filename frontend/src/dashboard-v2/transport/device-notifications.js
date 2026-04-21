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
      // reconcile the whole list. Fire-and-forget is fine.
      fetchDevices();
      break;
    default:
      dlog.debug('unhandled', type);
  }
}
