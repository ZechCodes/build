// Session state machine — the single source of truth for "is the app
// ready to talk to a device."
//
// Phases:
//   booting     — initTransport hasn't started (or is in flight) and no
//                 device has ever been e2ee-connected yet.
//   offline_sse — SSE is disconnected. Nothing else matters until SSE is
//                 back — Skrift's native pill handles the user-visible
//                 copy here; we stay out of the way.
//   connecting  — SSE is up, but either the active channel's device has
//                 not been e2ee-connected yet, or we have no active
//                 channel and no device has an e2ee connection.
//   ready       — SSE up AND the active channel's device is e2ee-
//                 connected. With no active channel, `ready` means at
//                 least one device has an e2ee connection.
//   degraded    — SSE up, the active channel's device WAS previously
//                 connected, and its e2ee dropped / is retrying. The
//                 distinction from `connecting` is what lets the
//                 reconnect pill say "Reconnecting" instead of
//                 "Connecting" on the second round.
//
// Inputs:
//   - `sse.connected` / `sse.disconnected` bus events
//   - `devicesStore` subscribe → per-device online/offline
//   - `e2ee.connecting` / `e2ee.connected` / `e2ee.disconnected` bus
//   - `reconnect.state` bus (self-heal) → adds `retrying` + `failed`
//   - `uiStore.subscribe` for active channel
//   - `channelsStore.subscribe` to resolve active channel → active device
//
// Outputs:
//   - `subscribe(fn)` — fn is called with `{ phase, prevPhase, snapshot }`
//     on every transition. Also called once on subscribe with the current
//     state so listeners don't have to prime themselves.
//   - `bus.emit('session.phase', { phase, prevPhase })` for loggers.

import { bus } from './bus.js';
import { log } from './log.js';

const plog = log('session');

const state = {
  initialized: false,
  phase: 'booting',
  sse: 'unknown',                // 'unknown' | 'connected' | 'disconnected'
  activeChannelId: null,
  // per-device: { online: bool, e2ee: 'idle'|'connecting'|'connected'|'disconnected',
  //               everConnected: bool, retrying: bool, failed: bool }
  devices: new Map(),
  subscribers: new Set(),
};

function deviceEntry(deviceId) {
  let d = state.devices.get(deviceId);
  if (!d) {
    d = {
      online: false,
      e2ee: 'idle',
      everConnected: false,
      retrying: false,
      failed: false,
    };
    state.devices.set(deviceId, d);
  }
  return d;
}

function activeDeviceId() {
  // Resolved lazily so sessionStore doesn't need to carry a channelsStore
  // dependency in its module graph (avoids a boot-time cycle).
  const chId = state.activeChannelId;
  if (!chId) return null;
  return _channelsStore?.deviceFor(chId) || null;
}

function snapshot() {
  const activeId = activeDeviceId();
  const active = activeId ? state.devices.get(activeId) : null;
  return {
    phase: state.phase,
    sse: state.sse,
    activeChannelId: state.activeChannelId,
    activeDeviceId: activeId,
    activeDevice: active ? { ...active } : null,
    failed: !!active?.failed,
  };
}

function deriveAndEmit() {
  if (!state.initialized) return;
  const next = derive();
  if (next === state.phase) {
    // Still notify subscribers — the active device / failed flag may
    // have shifted even if the phase is unchanged. Subscribers are
    // cheap (one render each) and this keeps the pill reactive.
    for (const fn of state.subscribers) {
      try { fn({ phase: next, prevPhase: next, snapshot: snapshot() }); }
      catch (err) { plog.error('subscriber threw', err); }
    }
    return;
  }
  const prevPhase = state.phase;
  state.phase = next;
  plog.debug('phase', prevPhase, '→', next);
  for (const fn of state.subscribers) {
    try { fn({ phase: next, prevPhase, snapshot: snapshot() }); }
    catch (err) { plog.error('subscriber threw', err); }
  }
  bus.emit('session.phase', { phase: next, prevPhase });
}

function derive() {
  if (state.sse === 'disconnected') return 'offline_sse';

  const activeId = activeDeviceId();

  if (activeId) {
    const active = state.devices.get(activeId);
    if (active?.e2ee === 'connected') return 'ready';
    if (active?.everConnected) return 'degraded';
    return 'connecting';
  }

  // No active device resolved yet. If there IS an active channel, we're
  // waiting for its device to appear — treat as connecting, not booting.
  if (state.activeChannelId) {
    return state.sse === 'connected' ? 'connecting' : 'booting';
  }

  // No active channel at all: ready iff any device is e2ee-connected.
  for (const d of state.devices.values()) {
    if (d.e2ee === 'connected') return 'ready';
  }
  // booting stays booting until we've observed at least one non-idle
  // e2ee transition or at least one device in the store; after that
  // any "nothing connected" state is `connecting` not `booting`.
  const anyMovement = [...state.devices.values()].some(d => d.e2ee !== 'idle');
  return state.sse === 'connected' && anyMovement ? 'connecting' : 'booting';
}

// ─── Public API ──────────────────────────────────────────────────────

/** @type {{ deviceFor(id: string): string | undefined } | null} */
let _channelsStore = null;
/** @type {{ getActiveChannel(): string | null, subscribe(fn): () => void } | null} */
let _uiStore = null;

/**
 * Initialize the session store. Idempotent. Must be called after the
 * bus + stores are importable but can be called before transport boots.
 *
 * Takes the stores as arguments to avoid a circular import between
 * session-store and the domain stores that rely on core/.
 */
export function initSessionStore({ devicesStore, channelsStore, uiStore } = {}) {
  if (state.initialized) return;
  state.initialized = true;
  _channelsStore = channelsStore || null;
  _uiStore = uiStore || null;

  state.activeChannelId = _uiStore?.getActiveChannel() || null;

  // SSE — treat initial state as 'unknown' rather than connected. The
  // sk:notification-status event will flip us to 'connected' as soon
  // as Skrift's EventSource latches. Until then we stay in `booting`.
  bus.on('sse.connected', () => {
    state.sse = 'connected';
    deriveAndEmit();
  });
  bus.on('sse.disconnected', () => {
    state.sse = 'disconnected';
    deriveAndEmit();
  });

  // E2EE lifecycle.
  bus.on('e2ee.connecting', ({ deviceId }) => {
    if (!deviceId) return;
    deviceEntry(deviceId).e2ee = 'connecting';
    deriveAndEmit();
  });
  bus.on('e2ee.connected', ({ deviceId }) => {
    if (!deviceId) return;
    const d = deviceEntry(deviceId);
    d.e2ee = 'connected';
    d.everConnected = true;
    d.retrying = false;
    d.failed = false;
    deriveAndEmit();
  });
  bus.on('e2ee.disconnected', ({ deviceId }) => {
    if (!deviceId) return;
    deviceEntry(deviceId).e2ee = 'disconnected';
    deriveAndEmit();
  });

  // Self-heal state (retrying / failed flags).
  bus.on('reconnect.state', ({ deviceId, phase }) => {
    if (!deviceId) return;
    const d = deviceEntry(deviceId);
    d.retrying = phase === 'retrying';
    d.failed = phase === 'failed';
    deriveAndEmit();
  });

  // Devices.
  if (devicesStore) {
    // Prime from any devices already loaded.
    for (const dev of devicesStore.list?.() || []) {
      deviceEntry(dev.id).online = dev.status === 'online';
    }
    devicesStore.subscribe(() => {
      for (const dev of devicesStore.list()) {
        deviceEntry(dev.id).online = dev.status === 'online';
      }
      deriveAndEmit();
    });
  }

  // Active channel — the resolved device may change when channelsStore
  // catches up after a channel list, so listen to both ui + channels.
  if (_uiStore) {
    _uiStore.subscribe(e => {
      if (e.kind === 'active_channel') {
        state.activeChannelId = e.id;
        deriveAndEmit();
      }
    });
  }
  if (_channelsStore) {
    _channelsStore.subscribe(() => {
      // channelsStore doesn't emit fine-grained kinds for our purpose;
      // re-derive any time the store changes. It's cheap.
      deriveAndEmit();
    });
  }

  deriveAndEmit();
}

/** Current phase. */
export function getPhase() { return state.phase; }

/** Current snapshot — phase + SSE + active device view. Clone-safe. */
export function getSnapshot() { return snapshot(); }

/**
 * Subscribe to phase changes. Calls fn once immediately with the current
 * snapshot so listeners don't have to prime themselves.
 */
export function subscribe(fn) {
  state.subscribers.add(fn);
  try {
    fn({ phase: state.phase, prevPhase: state.phase, snapshot: snapshot() });
  } catch (err) { plog.error('subscriber threw on subscribe', err); }
  return () => state.subscribers.delete(fn);
}

/** True iff the given device's E2EE session is connected right now. */
export function isDeviceReady(deviceId) {
  return state.devices.get(deviceId)?.e2ee === 'connected';
}

/**
 * Resolve when the given device's E2EE session is connected. Rejects
 * with an AbortError if `signal` is aborted first.
 */
export function awaitDeviceReady(deviceId, { signal } = {}) {
  if (isDeviceReady(deviceId)) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      cleanup();
      const err = new Error('aborted');
      err.name = 'AbortError';
      reject(err);
    };
    const unsub = subscribe(() => {
      if (isDeviceReady(deviceId)) { cleanup(); resolve(); }
    });
    const cleanup = () => {
      unsub();
      signal?.removeEventListener?.('abort', onAbort);
    };
    if (signal?.aborted) return onAbort();
    signal?.addEventListener?.('abort', onAbort);
  });
}

// ─── Test hooks ──────────────────────────────────────────────────────

export function _resetForTests() {
  state.initialized = false;
  state.phase = 'booting';
  state.sse = 'unknown';
  state.activeChannelId = null;
  state.devices.clear();
  state.subscribers.clear();
  _channelsStore = null;
  _uiStore = null;
}

export const sessionStore = {
  getPhase, getSnapshot, subscribe,
  isDeviceReady, awaitDeviceReady,
};
