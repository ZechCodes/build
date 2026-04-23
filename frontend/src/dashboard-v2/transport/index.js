// Transport bootstrap. See planning/dashboard-v2/04-transport.md.

import { log } from '../core/log.js';
import { bindSse } from './sse.js';
import { bindCoordinator } from './coordinator.js';
import { bindIntentDispatcher } from './intent-dispatcher.js';
import { bindDeviceNotifications } from './device-notifications.js';
import { bindAttentionHydrator } from './attention-hydrator.js';
import { bindSelfHeal } from './self-heal.js';
import { bindSessionMarkers } from './session-markers.js';
import { bindFocusSync } from './focus-sync.js';
import { bindCurrentTool } from '../domain/current-tool-store.js';
import { fetchDevices } from './rest.js';
import { e2eePool } from './e2ee-pool.js';

const plog = log('transport');

export { e2eePool };

export async function initTransport() {
  plog.info('init');
  bindSse();
  bindDeviceNotifications();
  bindAttentionHydrator();
  bindSelfHeal();
  bindSessionMarkers();
  bindCurrentTool();
  bindIntentDispatcher();
  bindCoordinator();
  bindFocusSync();
  await fetchDevices();
  await e2eePool.connectReady();
}

/**
 * Upload a file to the device over E2EE. Returns a promise resolving to
 * {file_id, filename, size, mime_type, path}. Progress updates are also
 * emitted on the bus as `upload.progress` events.
 *
 * When `destDir` is supplied (relative to the channel's working
 * directory), the file is written INSIDE the workspace at
 * `<working_directory>/<destDir>/<filename>`. Without it, the file
 * lands in the bridge's scratch uploads registry — the behaviour
 * chat attachments use.
 *
 * This is the only non-intent surface in transport that views call
 * directly — async file transfer needs a promise, which doesn't map
 * cleanly to fire-and-forget intents. Keeps the view from importing
 * e2eePool or BuildE2EE directly.
 */
export async function uploadFile(channelId, file, destDir) {
  const conn = e2eePool.forChannel(channelId);
  if (!conn || !conn.connected) throw new Error('not connected');
  return conn.uploadFile(channelId, file, destDir);
}
