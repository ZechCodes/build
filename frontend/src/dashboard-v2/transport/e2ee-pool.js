// Owns the set of BuildE2EE instances — one per connected device.
//
// Per planning/dashboard-v2/04-transport.md, the pool is the only place
// views and the intent dispatcher resolve an E2EE instance. Connection
// lifecycle events are emitted onto the bus for observability.

import { BuildE2EE } from './vendor/e2ee.js';
import { bindE2EEDispatcher } from './e2ee-dispatcher.js';
import { bus } from '../core/bus.js';
import { log } from '../core/log.js';
import { devicesStore } from '../domain/devices-store.js';
import { channelsStore } from '../domain/channels-store.js';

const plog = log('e2ee-pool');
const instances = new Map();      // deviceId → BuildE2EE

async function connect(deviceId) {
  if (instances.has(deviceId)) return instances.get(deviceId);
  plog.info('connecting', deviceId);
  bus.emit('e2ee.connecting', { deviceId });

  const instance = new BuildE2EE();
  try {
    await instance.ready();
  } catch (err) {
    plog.error('libsodium not available', err);
    bus.emit('e2ee.error', { deviceId, error: String(err) });
    return null;
  }

  bindE2EEDispatcher(instance, deviceId);
  instances.set(deviceId, instance);

  try {
    await instance.connect(deviceId);
    plog.info('connected', deviceId);
  } catch (err) {
    plog.error('failed to connect', deviceId, err);
    try { instance.disconnect(); } catch (_) { /* ignore */ }
    instances.delete(deviceId);
    bus.emit('e2ee.error', { deviceId, error: String(err) });
    return null;
  }

  return instance;
}

function disconnect(deviceId) {
  const instance = instances.get(deviceId);
  if (!instance) return;
  try { instance.disconnect(); } catch (_) { /* ignore */ }
  instances.delete(deviceId);
}

function disconnectAll() {
  for (const id of [...instances.keys()]) disconnect(id);
}

function forDevice(deviceId) { return instances.get(deviceId); }

function forChannel(channelId) {
  const deviceId = channelsStore.deviceFor(channelId);
  return deviceId ? instances.get(deviceId) : undefined;
}

async function connectReady() {
  const candidates = devicesStore.list().filter(d => d.status === 'online' && d.has_transport_key);
  if (candidates.length === 0) {
    plog.info('no devices ready');
    return [];
  }
  return Promise.allSettled(candidates.map(d => connect(d.id)));
}

function status() {
  let connected = 0;
  for (const i of instances.values()) if (i.connected) connected += 1;
  return { count: instances.size, connected, anyConnected: connected > 0 };
}

export const e2eePool = {
  connect, disconnect, disconnectAll,
  forDevice, forChannel,
  connectReady, status,
  // Test hook only. Do not use from production code paths.
  _instances: instances,
};
