// REST wrappers for non-E2EE endpoints. Currently just /api/devices/.
//
// Results are emitted as bus events; the devicesStore subscribes to those
// events rather than being mutated directly. See
// planning/dashboard/04-transport.md.

import { bus } from '../core/bus.js';
import { log } from '../core/log.js';

const plog = log('rest');

export async function fetchDevices() {
  try {
    const resp = await fetch('/api/devices/');
    if (!resp.ok) {
      plog.warn('devices fetch failed', resp.status);
      return;
    }
    const devices = await resp.json();
    bus.emit('device.bulk', { devices });
  } catch (err) {
    plog.error('devices fetch threw', err);
  }
}
