// Transport bootstrap. See planning/dashboard-v2/04-transport.md.

import { log } from '../core/log.js';
import { bindSse } from './sse.js';
import { bindCoordinator } from './coordinator.js';
import { bindIntentDispatcher } from './intent-dispatcher.js';
import { fetchDevices } from './rest.js';
import { e2eePool } from './e2ee-pool.js';

const plog = log('transport');

export { e2eePool };

export async function initTransport() {
  plog.info('init');
  bindSse();
  bindIntentDispatcher();
  bindCoordinator();
  await fetchDevices();
  await e2eePool.connectReady();
}
