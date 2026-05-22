// Agent presence (active, plan mode) per channel + harness list per device.
// See planning/dashboard/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('presence');
const byChannel = new Map();       // channelId → {agentActive, planMode}
const harnessesByDevice = new Map(); // deviceId → harnesses[]

// A run of activity is considered a "session"; anything quieter than this
// between adjacent messages opens a new session. Used by the sidebar's
// Recent group to anchor each channel's sort position to the start of
// its current session instead of its most recent message — that anchor
// only moves when a fresh gap opens, so a channel's position stays put
// as new messages stream in.
export const SESSION_GAP_MS = 4 * 60 * 60 * 1000;

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { agentActive: false, planMode: false, lastActiveAt: 0, sessionStartAt: 0 };
    byChannel.set(channelId, s);
  }
  return s;
}

export const presenceStore = {
  get(channelId) {
    return byChannel.get(channelId)
      ?? { agentActive: false, planMode: false, lastActiveAt: 0, sessionStartAt: 0 };
  },

  setAgentActive(channelId, active) {
    const s = getSlot(channelId);
    if (s.agentActive === active) return;
    s.agentActive = active;
    // Stamp every transition so `lastActiveAt` reflects the most
    // recent moment the channel was "doing something" — used by the
    // sidebar Attention section to keep recently-active channels
    // around for a grace window after they go idle.
    s.lastActiveAt = Date.now();
    notify({ kind: 'agent_active', channelId, active });
  },

  setPlanMode(channelId, planMode) {
    const s = getSlot(channelId);
    if (s.planMode === planMode) return;
    s.planMode = planMode;
    notify({ kind: 'plan_mode', channelId, planMode });
  },

  /**
   * Stamp `lastActiveAt` from a historical source (e.g., the
   * latest agent message's `created_at`) without touching
   * `agentActive`. Used by the attention-hydrator to seed the
   * "recent" grace window on load.
   *
   * Keeps the larger of the existing stamp and the new one — we
   * don't want to accidentally rewind time if a live
   * `setAgentActive` already ran.
   */
  hydrateLastActive(channelId, ms) {
    if (!ms) return;
    const s = getSlot(channelId);
    if ((s.lastActiveAt || 0) >= ms) return;
    s.lastActiveAt = ms;
    notify({ kind: 'last_active', channelId, at: ms });
  },

  /**
   * Set the session-start anchor for a channel — computed from full
   * message history by the attention-hydrator, so this replaces rather
   * than accumulates. Ignored when `ms` is falsy.
   */
  hydrateSessionStart(channelId, ms) {
    if (!ms) return;
    const s = getSlot(channelId);
    if (s.sessionStartAt === ms) return;
    s.sessionStartAt = ms;
    notify({ kind: 'session_start', channelId, at: ms });
  },

  setHarnesses(deviceId, harnesses) {
    harnessesByDevice.set(deviceId, harnesses);
    notify({ kind: 'harnesses', deviceId });
  },

  getHarnesses(deviceId) { return harnessesByDevice.get(deviceId) ?? []; },

  __resetForTests__() {
    byChannel.clear();
    harnessesByDevice.clear();
  },

  subscribe,
};

// ----- Bus bindings -----
bus.on('agent.started', ({ channelId }) => {
  if (channelId) presenceStore.setAgentActive(channelId, true);
});
bus.on('agent.stopped', ({ channelId }) => {
  if (channelId) presenceStore.setAgentActive(channelId, false);
});
bus.on('agent.restarted', ({ channelId }) => {
  if (channelId) presenceStore.setAgentActive(channelId, true);
});
bus.on('agent.active', ({ channelId, active }) => {
  if (channelId) presenceStore.setAgentActive(channelId, !!active);
});
bus.on('agent.plan_mode', ({ channelId, planMode }) => {
  if (channelId) presenceStore.setPlanMode(channelId, !!planMode);
});
bus.on('harness.list', ({ deviceId, harnesses }) => {
  presenceStore.setHarnesses(deviceId, harnesses || []);
});
bus.on('message.received', ({ channelId, msg }) => {
  // Any live message — user or agent — bumps the channel into the
  // Recent sidebar's primary (4hr) window. We prefer the message's
  // own `created_at` over `Date.now()` so the timestamp matches
  // hydration on a later page load, but fall back to wall-clock
  // when the wire payload is malformed.
  if (!channelId) return;
  const stamp = timeOf(msg?.created_at) || Date.now();
  // Detect a session boundary BEFORE we overwrite lastActiveAt: a
  // quiet stretch longer than SESSION_GAP_MS (or a channel with no
  // session anchor yet) means this message starts a fresh session
  // and becomes the new sort anchor.
  const slot = byChannel.get(channelId);
  const prevLast = slot?.lastActiveAt || 0;
  const prevSessionStart = slot?.sessionStartAt || 0;
  if (!prevSessionStart || (prevLast && stamp - prevLast >= SESSION_GAP_MS)) {
    presenceStore.hydrateSessionStart(channelId, stamp);
  }
  presenceStore.hydrateLastActive(channelId, stamp);
});

function timeOf(raw) {
  if (!raw) return 0;
  if (raw instanceof Date) return raw.getTime();
  if (typeof raw === 'number') return raw > 1e12 ? raw : raw * 1000;
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}
