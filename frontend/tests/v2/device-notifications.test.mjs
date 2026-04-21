// `sk:notification` → v2 bus fan-out via device-notifications.js.
//
// node:test runs headless — stub `document` + `CustomEvent` from jsdom-ish
// globals: Node 20+ exposes EventTarget but no `document`. Use a minimal
// shim that satisfies the module's single `addEventListener('sk:notification')`
// call.

import { test } from 'node:test';
import assert from 'node:assert/strict';

// Minimal DOM shim. The module only calls
// `document.addEventListener('sk:notification', handler)`
// and we dispatch events by invoking the handler directly.
const listeners = new Map();
globalThis.document = {
  addEventListener(type, handler) { listeners.set(type, handler); },
  removeEventListener(type, handler) {
    if (listeners.get(type) === handler) listeners.delete(type);
  },
};

// Stub fetch so fetchDevices (triggered on authorized/e2ee-ready/etc.) doesn't
// break anything. It just needs to not throw.
globalThis.fetch = async () => ({ ok: false, status: 200, json: async () => [] });

const { bus } = await import('../../src/dashboard-v2/core/bus.js');
const { bindDeviceNotifications, unbindDeviceNotifications } =
  await import('../../src/dashboard-v2/transport/device-notifications.js');

function fire(type, detail) {
  const handler = listeners.get('sk:notification');
  if (!handler) throw new Error('no sk:notification handler bound');
  handler({ type: 'sk:notification', detail, preventDefault() {} });
}

bindDeviceNotifications();

test('build:device:online emits device.status_changed online', () => {
  const seen = [];
  const off = bus.on('device.status_changed', (p) => seen.push(p));
  fire('sk:notification', { type: 'build:device:online', device_id: 'd-online' });
  off();
  assert.equal(seen.length, 1);
  assert.deepEqual(seen[0], { deviceId: 'd-online', status: 'online' });
});

test('build:device:offline emits device.status_changed offline', () => {
  const seen = [];
  const off = bus.on('device.status_changed', (p) => seen.push(p));
  fire('sk:notification', { type: 'build:device:offline', device_id: 'd-offline' });
  off();
  assert.deepEqual(seen[0], { deviceId: 'd-offline', status: 'offline' });
});

test('build:device:heartbeat-missed emits offline (same as lost socket)', () => {
  const seen = [];
  const off = bus.on('device.status_changed', (p) => seen.push(p));
  fire('sk:notification', {
    type: 'build:device:heartbeat-missed', device_id: 'd-hb',
  });
  off();
  assert.deepEqual(seen[0], { deviceId: 'd-hb', status: 'offline' });
});

test('non-build:device notifications are ignored', () => {
  const seen = [];
  const off = bus.on('device.status_changed', (p) => seen.push(p));
  fire('sk:notification', { type: 'skrift:notice', device_id: 'd-noise' });
  off();
  assert.equal(seen.length, 0);
});

test('missing device_id does not emit', () => {
  const seen = [];
  const off = bus.on('device.status_changed', (p) => seen.push(p));
  fire('sk:notification', { type: 'build:device:online' });
  off();
  assert.equal(seen.length, 0);
});

// Cleanup so this module leaves no global handler registered.
test.after(() => {
  unbindDeviceNotifications();
});
