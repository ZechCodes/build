// Verify SSE-reconnect coordinator runs teardown exactly once even when
// sse.connected fires rapidly.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { e2eePool } from '../../src/dashboard-v2/transport/e2ee-pool.js';
import { bindCoordinator } from '../../src/dashboard-v2/transport/coordinator.js';

// Stubs: track call counts. Make connectReady await a tick so the second
// sse.connected emit races against it.
let disconnectAllCount = 0;
let connectReadyCount = 0;

e2eePool.status = () => ({ count: 1, connected: 1, anyConnected: true });
e2eePool.disconnectAll = () => { disconnectAllCount += 1; };
e2eePool.connectReady = async () => {
  connectReadyCount += 1;
  await new Promise(r => setImmediate(r));
  return [];
};

bindCoordinator();

test('back-to-back sse.connected triggers one teardown cycle', async () => {
  disconnectAllCount = 0;
  connectReadyCount = 0;

  bus.emit('sse.connected', {});
  bus.emit('sse.connected', {});

  // Let the async handler resolve.
  await new Promise(r => setTimeout(r, 20));

  assert.equal(disconnectAllCount, 1, 'disconnectAll should run exactly once');
  assert.equal(connectReadyCount, 1, 'connectReady should run exactly once');
});

test('sse.connected with empty pool is a no-op', async () => {
  e2eePool.status = () => ({ count: 0, connected: 0, anyConnected: false });

  disconnectAllCount = 0;
  connectReadyCount = 0;
  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 20));

  assert.equal(disconnectAllCount, 0);
  assert.equal(connectReadyCount, 0);

  e2eePool.status = () => ({ count: 1, connected: 1, anyConnected: true });
});

test('after a cycle completes, another sse.connected can run again', async () => {
  disconnectAllCount = 0;
  connectReadyCount = 0;

  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 20));
  bus.emit('sse.connected', {});
  await new Promise(r => setTimeout(r, 20));

  assert.equal(disconnectAllCount, 2);
  assert.equal(connectReadyCount, 2);
});
