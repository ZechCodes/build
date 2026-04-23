// Unit tests for the session state machine.
//
// Covers phase derivation across all the inputs sessionStore listens to,
// the await helper's abort semantics, and the subscribe call-once-on-init
// contract used by the reconnect pill.

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import {
  sessionStore, initSessionStore, _resetForTests,
} from '../../src/dashboard-v2/core/session-store.js';

// Minimal fakes — we don't need the real stores for this test file.
function makeFakeDevicesStore() {
  const devices = new Map();
  const subs = new Set();
  const notify = () => { for (const fn of subs) fn({ kind: 'changed' }); };
  return {
    _devices: devices,
    list() { return [...devices.values()]; },
    setStatus(id, status) {
      const d = devices.get(id) || { id };
      devices.set(id, { ...d, status });
      notify();
    },
    subscribe(fn) { subs.add(fn); return () => subs.delete(fn); },
  };
}

function makeFakeChannelsStore() {
  const byId = new Map();
  const deviceOf = new Map();
  const subs = new Set();
  const notify = () => { for (const fn of subs) fn({ kind: 'changed' }); };
  return {
    deviceFor(id) { return deviceOf.get(id); },
    setChannel(channelId, deviceId) {
      byId.set(channelId, { id: channelId });
      deviceOf.set(channelId, deviceId);
      notify();
    },
    subscribe(fn) { subs.add(fn); return () => subs.delete(fn); },
  };
}

function makeFakeUiStore() {
  let active = null;
  const subs = new Set();
  return {
    getActiveChannel() { return active; },
    setActiveChannel(id) {
      if (active === id) return;
      active = id;
      for (const fn of subs) fn({ kind: 'active_channel', id });
    },
    subscribe(fn) { subs.add(fn); return () => subs.delete(fn); },
  };
}

let devicesStore, channelsStore, uiStore;

beforeEach(() => {
  _resetForTests();
  devicesStore = makeFakeDevicesStore();
  channelsStore = makeFakeChannelsStore();
  uiStore = makeFakeUiStore();
  initSessionStore({ devicesStore, channelsStore, uiStore });
});

test('starts in booting before any event', () => {
  assert.equal(sessionStore.getPhase(), 'booting');
});

test('sse.disconnected → offline_sse regardless of prior state', () => {
  bus.emit('sse.connected', {});
  bus.emit('sse.disconnected', {});
  assert.equal(sessionStore.getPhase(), 'offline_sse');
});

test('sse.connected with no devices stays in booting', () => {
  bus.emit('sse.connected', {});
  assert.equal(sessionStore.getPhase(), 'booting');
});

test('connecting once an e2ee attempt has begun', () => {
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connecting', { deviceId: 'd1' });
  assert.equal(sessionStore.getPhase(), 'connecting');
});

test('ready once any device is e2ee-connected (no active channel)', () => {
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'd1' });
  assert.equal(sessionStore.getPhase(), 'ready');
});

test('connecting → ready when the active channel resolves to a connected device', () => {
  bus.emit('sse.connected', {});
  uiStore.setActiveChannel('ch1');
  // channel list hasn't come in yet — device unknown for ch1.
  assert.equal(sessionStore.getPhase(), 'connecting');
  bus.emit('e2ee.connected', { deviceId: 'd1' });
  // still connecting — d1 isn't the active channel's device yet.
  assert.equal(sessionStore.getPhase(), 'connecting');
  channelsStore.setChannel('ch1', 'd1');
  assert.equal(sessionStore.getPhase(), 'ready');
});

test('ready → degraded when the active device drops', () => {
  bus.emit('sse.connected', {});
  channelsStore.setChannel('ch1', 'd1');
  uiStore.setActiveChannel('ch1');
  bus.emit('e2ee.connected', { deviceId: 'd1' });
  assert.equal(sessionStore.getPhase(), 'ready');
  bus.emit('e2ee.disconnected', { deviceId: 'd1' });
  assert.equal(sessionStore.getPhase(), 'degraded');
});

test('degraded → ready on reconnect', () => {
  bus.emit('sse.connected', {});
  channelsStore.setChannel('ch1', 'd1');
  uiStore.setActiveChannel('ch1');
  bus.emit('e2ee.connected', { deviceId: 'd1' });
  bus.emit('e2ee.disconnected', { deviceId: 'd1' });
  bus.emit('e2ee.connected', { deviceId: 'd1' });
  assert.equal(sessionStore.getPhase(), 'ready');
});

test('ready → offline_sse trumps degraded', () => {
  bus.emit('sse.connected', {});
  channelsStore.setChannel('ch1', 'd1');
  uiStore.setActiveChannel('ch1');
  bus.emit('e2ee.connected', { deviceId: 'd1' });
  bus.emit('e2ee.disconnected', { deviceId: 'd1' });
  bus.emit('sse.disconnected', {});
  assert.equal(sessionStore.getPhase(), 'offline_sse');
});

test('reconnect.state failed flag surfaces on the snapshot', () => {
  bus.emit('sse.connected', {});
  channelsStore.setChannel('ch1', 'd1');
  uiStore.setActiveChannel('ch1');
  bus.emit('e2ee.connected', { deviceId: 'd1' });
  bus.emit('e2ee.disconnected', { deviceId: 'd1' });
  bus.emit('reconnect.state', { deviceId: 'd1', phase: 'failed' });
  const snap = sessionStore.getSnapshot();
  assert.equal(snap.failed, true);
  assert.equal(snap.activeDeviceId, 'd1');
});

test('subscribe fires once with current snapshot on subscription', () => {
  const events = [];
  const off = sessionStore.subscribe((e) => events.push(e));
  assert.equal(events.length, 1);
  assert.equal(events[0].phase, 'booting');
  off();
});

test('awaitDeviceReady resolves when the device transitions to connected', async () => {
  bus.emit('sse.connected', {});
  const p = sessionStore.awaitDeviceReady('d1');
  setImmediate(() => bus.emit('e2ee.connected', { deviceId: 'd1' }));
  await p;
  assert.equal(sessionStore.isDeviceReady('d1'), true);
});

test('awaitDeviceReady rejects when its signal aborts', async () => {
  const ac = new AbortController();
  const p = sessionStore.awaitDeviceReady('d1', { signal: ac.signal });
  ac.abort();
  await assert.rejects(p, (err) => err.name === 'AbortError');
});
