// Verifies the session-aware transport coordinator.
//
// On transition OUT of `offline_sse`, the coordinator performs a
// soft refresh: re-fetch devices, ask each connected E2EE instance
// to re-list channels + harnesses, and kick connectReady for any
// new devices. It does NOT tear down existing E2EE instances.

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { e2eePool } from '../../src/dashboard-v2/transport/e2ee-pool.js';
import { channelsStore } from '../../src/dashboard-v2/domain/channels-store.js';
import { devicesStore } from '../../src/dashboard-v2/domain/devices-store.js';
import { uiStore } from '../../src/dashboard-v2/domain/ui-store.js';
import { initSessionStore, _resetForTests as resetSession }
  from '../../src/dashboard-v2/core/session-store.js';
import { bindCoordinator, _resetForTests as resetCoordinator }
  from '../../src/dashboard-v2/transport/coordinator.js';

let listInstances = [];
let connectReadyCount = 0;
let fetchCallCount = 0;

// Stub the global fetch used by rest.fetchDevices.
global.fetch = async (url) => {
  if (String(url).includes('/api/devices')) {
    fetchCallCount += 1;
    return { ok: true, json: async () => [] };
  }
  return { ok: false };
};

e2eePool.list = () => listInstances;
e2eePool.disconnectAll = () => { throw new Error('coordinator must not disconnectAll'); };
e2eePool.connectReady = async () => { connectReadyCount += 1; return []; };

function resetAll() {
  listInstances = [];
  connectReadyCount = 0;
  fetchCallCount = 0;
  resetCoordinator();
  resetSession();
}

beforeEach(resetAll);

test('sse.disconnected does not tear anything down', async () => {
  initSessionStore();
  bindCoordinator();
  bus.emit('sse.connected', {});
  bus.emit('sse.disconnected', {});
  await new Promise(r => setTimeout(r, 10));
  assert.equal(connectReadyCount, 0);
  assert.equal(fetchCallCount, 0);
});

test('sse recovery triggers one soft refresh', async () => {
  initSessionStore();
  bindCoordinator();
  let listChannelsCalls = 0;
  let listHarnessesCalls = 0;
  listInstances = [
    { connected: true,  listChannels: () => { listChannelsCalls++; }, listHarnesses: () => { listHarnessesCalls++; } },
    { connected: false, listChannels: () => { listChannelsCalls++; }, listHarnesses: () => { listHarnessesCalls++; } },
  ];

  // Initial connect — not transitioning out of offline_sse, so no refresh.
  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 10));
  assert.equal(fetchCallCount, 0, 'initial sse.connected is not a refresh');

  // Drop SSE then recover — THIS is a soft refresh.
  bus.emit('sse.disconnected', {});
  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 30));

  assert.equal(fetchCallCount, 1, 'fetchDevices called once on recovery');
  assert.equal(connectReadyCount, 1, 'connectReady called once on recovery');
  assert.equal(listChannelsCalls, 1, 'only the connected instance gets listChannels');
  assert.equal(listHarnessesCalls, 1, 'only the connected instance gets listHarnesses');
});

test('back-to-back sse.connected during a refresh does not double-fire', async () => {
  initSessionStore();
  bindCoordinator();
  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 5));
  bus.emit('sse.disconnected', {});
  bus.emit('sse.connected', {});
  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 30));
  assert.equal(fetchCallCount, 1);
  assert.equal(connectReadyCount, 1);
});

test('SSE recovery reloads active channel via channel-loader', async () => {
  initSessionStore({ devicesStore, channelsStore, uiStore });
  bindCoordinator();

  // Prime an active channel wired to a connected device.
  channelsStore.upsert({ deviceId: 'dev-active', channel: { id: 'chActive', name: 'c' } });
  uiStore.setActiveChannel('chActive');
  bus.emit('e2ee.connected', { deviceId: 'dev-active' });

  let getActivityCalls = 0;
  let getMessagesCalls = 0;
  const off1 = bus.on('intent.get_activity', (p) => {
    if (p.channelId === 'chActive') getActivityCalls++;
  });
  const off2 = bus.on('intent.get_messages', (p) => {
    if (p.channelId === 'chActive') getMessagesCalls++;
  });

  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 5));
  bus.emit('sse.disconnected', {});
  bus.emit('sse.connected', {});
  // Give the async refresh time to run its chain.
  await new Promise(r => setTimeout(r, 20));

  off1(); off2();
  uiStore.setActiveChannel(null);

  assert.equal(getActivityCalls, 1, 'active channel gets a refresh');
  assert.equal(getMessagesCalls, 1, 'messages forced on reconnect even if cached');
});

test('SSE recovery with no active channel skips the channel reload', async () => {
  initSessionStore({ devicesStore, channelsStore, uiStore });
  bindCoordinator();
  uiStore.setActiveChannel(null);

  let getActivityCalls = 0;
  const off = bus.on('intent.get_activity', () => { getActivityCalls++; });

  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 5));
  bus.emit('sse.disconnected', {});
  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 20));

  off();
  assert.equal(getActivityCalls, 0);
});
