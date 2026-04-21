// Unit coverage for transport/self-heal.js — per-device reconnect
// with exponential backoff + retryNow().
//
// The module calls `e2eePool.connect(deviceId)`. We intercept that
// by monkey-patching the singleton.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { e2eePool } from '../../src/dashboard-v2/transport/e2ee-pool.js';

const selfHeal = await import('../../src/dashboard-v2/transport/self-heal.js');
const { bindSelfHeal, getState, retryNow, _setAttemptDelaysForTests, _resetStateForTests } = selfHeal;

// Tiny delays so tests run in ms, not seconds.
_setAttemptDelaysForTests([10, 10, 10]);

function tick(ms) { return new Promise(r => setTimeout(r, ms)); }

function captureReconnect() {
  const events = [];
  const off = bus.on('reconnect.state', (p) => events.push(p));
  return { events, off };
}

test('e2ee.disconnected → scheduled retry → e2eePool.connect called', async () => {
  _resetStateForTests();
  bindSelfHeal();
  const calls = [];
  const origConnect = e2eePool.connect;
  e2eePool.connect = async (id) => { calls.push(id); throw new Error('still down'); };
  try {
    const cap = captureReconnect();
    bus.emit('e2ee.disconnected', { deviceId: 'd-retry' });
    await tick(80);  // enough for 3 attempts at 10ms each
    cap.off();
    assert.ok(calls.length >= 1, `expected at least 1 connect call, saw ${calls.length}`);
    assert.deepEqual(calls.filter(id => id === 'd-retry').length > 0, true);
    // After exhausting retries we should end on "failed".
    const final = cap.events.at(-1);
    assert.equal(final?.phase, 'failed');
  } finally {
    e2eePool.connect = origConnect;
  }
});

test('successful reconnect resets the attempt counter', async () => {
  _resetStateForTests();
  bindSelfHeal();
  assert.equal(getState('d-reset').attempt, 0);
  // Simulate a disconnected → retrying → connected cycle.
  bus.emit('e2ee.disconnected', { deviceId: 'd-reset' });
  await tick(5);
  assert.ok(getState('d-reset').attempt >= 1);
  bus.emit('e2ee.connected', { deviceId: 'd-reset' });
  await tick(5);
  assert.equal(getState('d-reset').phase, 'idle');
  assert.equal(getState('d-reset').attempt, 0);
});

test('retryNow triggers an immediate connect attempt', async () => {
  _resetStateForTests();
  bindSelfHeal();
  const calls = [];
  const origConnect = e2eePool.connect;
  e2eePool.connect = async (id) => { calls.push(id); };
  try {
    await retryNow('d-manual');
    assert.deepEqual(calls, ['d-manual']);
    assert.equal(getState('d-manual').attempt, 0);
  } finally {
    e2eePool.connect = origConnect;
  }
});

test('SSE disconnect suppresses retry attempts', async () => {
  _resetStateForTests();
  bindSelfHeal();
  bus.emit('sse.disconnected', {});
  const calls = [];
  const origConnect = e2eePool.connect;
  e2eePool.connect = async (id) => { calls.push(id); throw new Error('nope'); };
  try {
    bus.emit('e2ee.disconnected', { deviceId: 'd-sse-down' });
    await tick(80);
    // connect() should be gated out because SSE is reported down.
    assert.equal(calls.length, 0);
  } finally {
    e2eePool.connect = origConnect;
    bus.emit('sse.connected', {});
  }
});
