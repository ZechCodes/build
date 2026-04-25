// Session-aware transport coordinator.
//
// Watches the session-store phase. When SSE comes back (transition out
// of `offline_sse`), performs a SOFT REFRESH instead of the old v1
// teardown-and-rebuild:
//
//   - Re-fetch the device list (REST) so any new/changed devices land
//     in the store.
//   - Ask each currently-connected E2EE instance to re-list channels
//     and harnesses so their in-memory state is fresh.
//   - Kick `e2eePool.connectReady()` for any newly-ready devices.
//
// We do NOT disconnect existing E2EE sessions on SSE recovery. The
// E2EE WebSocket is independent of the SSE stream and usually survives
// short SSE blips. If a particular E2EE socket actually died during
// the outage, self-heal.js is already watching and will schedule a
// retry on its own `e2ee.disconnected` event — nothing to do here.
//
// This replaces the v1 bug where `initE2EE._retryTimer` and the SSE
// reconnect handler could both initiate teardown; soft-refresh is
// idempotent and single-owner.

import { log } from '../core/log.js';
import { sessionStore } from '../core/session-store.js';
import { uiStore } from '../domain/ui-store.js';
import { e2eePool } from './e2ee-pool.js';
import { fetchDevices } from './rest.js';
import { loadChannel } from './channel-loader.js';

const plog = log('coordinator');
let refreshing = false;

export function bindCoordinator() {
  sessionStore.subscribe(({ phase, prevPhase }) => {
    if (prevPhase === 'offline_sse' && phase !== 'offline_sse') {
      _softRefresh();
    }
  });
}

async function _softRefresh() {
  if (refreshing) return;
  refreshing = true;
  plog.info('SSE back — soft-refreshing devices + channels');
  try {
    await fetchDevices();
    for (const inst of e2eePool.list()) {
      if (!inst.connected) continue;
      try { inst.listChannels(); } catch (err) { plog.debug('listChannels', err); }
      try { inst.listHarnesses(); } catch (err) { plog.debug('listHarnesses', err); }
    }
    await e2eePool.connectReady();

    // Refresh the active channel's panel data too — listChannels only
    // covers metadata. Reuses the same code path as initial load and
    // channel-switch so behavior stays consistent across entry points.
    const activeId = uiStore.getActiveChannel?.();
    if (activeId) {
      loadChannel(activeId, { forceFetchMessages: true }).catch(err => {
        if (err?.name !== 'AbortError') {
          plog.error('reload active channel failed', err);
        }
      });
    }
  } catch (err) {
    plog.error('soft refresh failed', err);
  } finally {
    refreshing = false;
  }
}

export function isRefreshing() { return refreshing; }

// Test hook — kept narrow.
export function _resetForTests() { refreshing = false; }
