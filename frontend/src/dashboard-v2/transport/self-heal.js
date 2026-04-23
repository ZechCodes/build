// Per-device auto-reconnect for E2EE transports.
//
// Listens on the v2 bus for `e2ee.disconnected` + `e2ee.connected`
// and runs a capped exponential backoff loop, calling
// `e2eePool.connect(deviceId)` on each attempt. Resets on success.
// Suppresses retries while SSE is unhealthy — the coordinator will
// do a global tear-down once SSE comes back and that covers us.
//
// Also exports `getState(deviceId)` + `retryNow(deviceId)` for the
// reconnect pill view to read + let the user trigger a manual
// retry when we've given up.

import { bus } from '../core/bus.js';
import { log } from '../core/log.js';
import { sessionStore } from '../core/session-store.js';
import { e2eePool } from './e2ee-pool.js';

const plog = log('self-heal');

const ATTEMPT_DELAYS_MS = [1000, 2000, 4000, 8000, 15000];

/** @type {Map<string, { attempt: number, timer: any, nextAt: number, phase: string }>} */
const state = new Map();
let bound = false;

function setPhase(deviceId, phase, extra = {}) {
  const s = state.get(deviceId) || { attempt: 0, timer: null, nextAt: 0, phase: 'idle' };
  s.phase = phase;
  if ('attempt' in extra) s.attempt = extra.attempt;
  if ('nextAt' in extra) s.nextAt = extra.nextAt;
  state.set(deviceId, s);
  bus.emit('reconnect.state', { deviceId, phase, attempt: s.attempt, nextAt: s.nextAt });
}

function clearTimer(deviceId) {
  const s = state.get(deviceId);
  if (s?.timer) { clearTimeout(s.timer); s.timer = null; }
}

export function getState(deviceId) {
  const s = state.get(deviceId);
  if (!s) return { phase: 'idle', attempt: 0, nextAt: 0 };
  return { phase: s.phase, attempt: s.attempt, nextAt: s.nextAt };
}

export async function retryNow(deviceId) {
  clearTimer(deviceId);
  const s = state.get(deviceId) || { attempt: 0 };
  s.attempt = 0;
  state.set(deviceId, s);
  await _attemptReconnect(deviceId);
}

function _scheduleRetry(deviceId) {
  clearTimer(deviceId);
  const s = state.get(deviceId) || { attempt: 0, timer: null, nextAt: 0, phase: 'idle' };
  if (s.attempt >= ATTEMPT_DELAYS_MS.length) {
    setPhase(deviceId, 'failed', { attempt: s.attempt });
    plog.warn('giving up on', deviceId, 'after', s.attempt, 'attempts');
    return;
  }
  const delay = ATTEMPT_DELAYS_MS[s.attempt];
  s.attempt += 1;
  s.nextAt = Date.now() + delay;
  s.timer = setTimeout(() => _attemptReconnect(deviceId), delay);
  state.set(deviceId, s);
  setPhase(deviceId, 'retrying', { attempt: s.attempt, nextAt: s.nextAt });
}

async function _attemptReconnect(deviceId) {
  if (sessionStore.getPhase() === 'offline_sse') {
    // Don't fight the SSE-reconnect coordinator — it'll soft-refresh
    // once SSE comes back.
    return;
  }
  try {
    await e2eePool.connect(deviceId);
    // Successful connect fires `e2ee.connected` via dispatcher,
    // which calls _resetRetries below.
  } catch (err) {
    plog.debug('reconnect attempt failed for', deviceId, err);
    _scheduleRetry(deviceId);
  }
}

function _resetRetries(deviceId) {
  clearTimer(deviceId);
  state.set(deviceId, { attempt: 0, timer: null, nextAt: 0, phase: 'idle' });
  bus.emit('reconnect.state', { deviceId, phase: 'idle', attempt: 0, nextAt: 0 });
}

export function bindSelfHeal() {
  if (bound) return;
  bound = true;

  bus.on('e2ee.disconnected', ({ deviceId }) => {
    if (!deviceId) return;
    const s = state.get(deviceId);
    // Don't rearm if we've already given up — a manual retry flips
    // us back into the loop explicitly.
    if (s?.phase === 'failed') return;
    _scheduleRetry(deviceId);
  });

  bus.on('e2ee.connected', ({ deviceId }) => {
    if (!deviceId) return;
    _resetRetries(deviceId);
  });
}

// Testing hook: lets tests swap out the delay table for tiny
// delays so the backoff runs quickly.
export function _setAttemptDelaysForTests(delaysMs) {
  ATTEMPT_DELAYS_MS.length = 0;
  ATTEMPT_DELAYS_MS.push(...delaysMs);
}
export function _resetStateForTests() {
  for (const [, s] of state) if (s.timer) clearTimeout(s.timer);
  state.clear();
  bound = false;
}
