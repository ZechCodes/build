// Agent presence (active, plan mode) per channel + harness list per device.
// See planning/dashboard/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('presence');
const byChannel = new Map();       // channelId → {agentActive, planMode}
const harnessesByDevice = new Map(); // deviceId → harnesses[]

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { agentActive: false, planMode: false, lastActiveAt: 0 };
    byChannel.set(channelId, s);
  }
  return s;
}

export const presenceStore = {
  get(channelId) {
    return byChannel.get(channelId) ?? { agentActive: false, planMode: false, lastActiveAt: 0 };
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

  setHarnesses(deviceId, harnesses) {
    harnessesByDevice.set(deviceId, harnesses);
    notify({ kind: 'harnesses', deviceId });
  },

  getHarnesses(deviceId) { return harnessesByDevice.get(deviceId) ?? []; },

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
