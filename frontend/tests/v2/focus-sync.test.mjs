// Tab visibility re-sync. When the tab comes back from hidden, the
// module asks each connected E2EE instance to re-list channels and
// reloads the active channel — so presenceStore gets reconciled
// against the bridge's authoritative `is_running` snapshot.

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { e2eePool } from '../../src/dashboard-v2/transport/e2ee-pool.js';
import { channelsStore } from '../../src/dashboard-v2/domain/channels-store.js';
import { devicesStore } from '../../src/dashboard-v2/domain/devices-store.js';
import { uiStore } from '../../src/dashboard-v2/domain/ui-store.js';
import {
  initSessionStore, _resetForTests as resetSession,
} from '../../src/dashboard-v2/core/session-store.js';
import {
  bindFocusSync, unbindFocusSync, _resetForTests as resetFocus,
} from '../../src/dashboard-v2/transport/focus-sync.js';

// Minimal `document` shim with mutable visibilityState + a
// dispatchEvent that matches the one production code calls.
const listeners = new Set();
let visibility = 'visible';
globalThis.document = {
  get visibilityState() { return visibility; },
  addEventListener(name, fn) { if (name === 'visibilitychange') listeners.add(fn); },
  removeEventListener(name, fn) { if (name === 'visibilitychange') listeners.delete(fn); },
};
function setVisibility(v) {
  visibility = v;
  for (const fn of listeners) fn();
}

let listChannelsCount;
let instances;

beforeEach(() => {
  unbindFocusSync();
  resetFocus();
  resetSession();
  initSessionStore({ devicesStore, channelsStore, uiStore });
  listChannelsCount = 0;
  instances = [
    { connected: true,  listChannels: () => { listChannelsCount++; }, listHarnesses: () => {} },
    { connected: false, listChannels: () => { listChannelsCount++; }, listHarnesses: () => {} },
  ];
  e2eePool.list = () => instances;
  uiStore.setActiveChannel(null);
});

test('hidden → visible triggers listChannels on connected instances only', async () => {
  bindFocusSync();
  setVisibility('hidden');
  // Need >= 1s hidden before refresh fires.
  await new Promise(r => setTimeout(r, 1020));
  setVisibility('visible');

  assert.equal(listChannelsCount, 1, 'only the connected instance is pinged');
});

test('brief hide (<1s) does not fire', async () => {
  bindFocusSync();
  setVisibility('hidden');
  await new Promise(r => setTimeout(r, 50));
  setVisibility('visible');

  assert.equal(listChannelsCount, 0);
});

test('initial visible (no prior hidden) does not fire', () => {
  bindFocusSync();
  setVisibility('visible');  // never was hidden
  assert.equal(listChannelsCount, 0);
});

test('emits focus.resync with hiddenFor when refreshing', async () => {
  bindFocusSync();
  const seen = [];
  const off = bus.on('focus.resync', (e) => seen.push(e));
  setVisibility('hidden');
  await new Promise(r => setTimeout(r, 1020));
  setVisibility('visible');
  off();

  assert.equal(seen.length, 1);
  assert.ok(seen[0].hiddenFor >= 1000, `hiddenFor ${seen[0].hiddenFor}`);
});

test('active channel triggers loadChannel intents on refresh', async () => {
  // Arm session so loadChannel can resolve.
  channelsStore.upsert({ deviceId: 'devF', channel: { id: 'chF', name: 'f' } });
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'devF' });
  uiStore.setActiveChannel('chF');

  bindFocusSync();
  const seen = [];
  const off = bus.on('intent.get_activity', (p) => {
    if (p.channelId === 'chF') seen.push(p);
  });

  setVisibility('hidden');
  await new Promise(r => setTimeout(r, 1020));
  setVisibility('visible');
  // Let loadChannel fire intents after awaitChannelReady resolves.
  await new Promise(r => setTimeout(r, 20));

  off();
  uiStore.setActiveChannel(null);

  assert.equal(seen.length, 1);
});
