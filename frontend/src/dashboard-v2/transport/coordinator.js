// SSE-reconnect coordinator: tears down stale E2EE sessions and reconnects
// on a single code path so retries do not double-fire.
//
// Fixes v1 bug where `initE2EE._retryTimer` and the SSE reconnect handler
// could both initiate teardown (see planning/dashboard-v2/00-diagnosis.md).

import { bus } from '../core/bus.js';
import { log } from '../core/log.js';
import { e2eePool } from './e2ee-pool.js';
import { fetchDevices } from './rest.js';

const plog = log('coordinator');
let reconnecting = false;

export function bindCoordinator() {
  bus.on('sse.connected', async () => {
    if (reconnecting) return;
    if (e2eePool.status().count === 0) return;
    reconnecting = true;
    plog.info('SSE reconnected — refreshing E2EE sessions');
    try {
      e2eePool.disconnectAll();
      await fetchDevices();
      await e2eePool.connectReady();
    } finally {
      reconnecting = false;
    }
  });
}

export function isReconnecting() { return reconnecting; }
