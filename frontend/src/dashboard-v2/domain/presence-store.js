// Agent presence (active, plan mode) per channel + harness list per device.
// See planning/dashboard-v2/02-stores.md.

import { makeSubscribable } from '../core/store.js';
import { bus } from '../core/bus.js';

const { subscribe, notify } = makeSubscribable('presence');
const byChannel = new Map();       // channelId → {agentActive, planMode}
const harnessesByDevice = new Map(); // deviceId → harnesses[]

function getSlot(channelId) {
  let s = byChannel.get(channelId);
  if (!s) {
    s = { agentActive: false, planMode: false };
    byChannel.set(channelId, s);
  }
  return s;
}

export const presenceStore = {
  get(channelId) { return byChannel.get(channelId) ?? { agentActive: false, planMode: false }; },

  setAgentActive(channelId, active) {
    const s = getSlot(channelId);
    if (s.agentActive === active) return;
    s.agentActive = active;
    notify({ kind: 'agent_active', channelId, active });
  },

  setPlanMode(channelId, planMode) {
    const s = getSlot(channelId);
    if (s.planMode === planMode) return;
    s.planMode = planMode;
    notify({ kind: 'plan_mode', channelId, planMode });
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
