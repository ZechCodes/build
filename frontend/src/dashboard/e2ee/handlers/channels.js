import { state } from '../../state.js';
import { rebuildChatChannels, renderChannelList, renderChannelSidebar, incrementUnread } from '../../channels/list.js';
import { selectChannel } from '../../channels/select.js';
import { renderMessages, updateStopButton } from '../../chat/messages.js';
import { syncChatOverlayHeader, applyChannelHarnessInfo } from '../../chat/overlay.js';

export function bindChannelHandlers(instance, deviceId) {
  instance.addEventListener('channel_list', (evt) => {
    const { channels, agent_cwd } = evt.detail;
    if (agent_cwd) state.deviceAgentCwd.set(deviceId, agent_cwd);
    // Diff against cached channels to detect removals.
    const oldChans = state.deviceChannels.get(deviceId);
    const oldIds = oldChans ? new Set(oldChans.keys()) : new Set();
    // Update per-device channel map.
    const devChans = new Map();
    for (const ch of channels) {
      devChans.set(ch.id, ch);
      state.channelDeviceMap.set(ch.id, deviceId);
      if (ch.plan_mode != null) state.channelPlanMode.set(ch.id, ch.plan_mode);
      if (ch.last_seen_at) state.channelLastSeen.set(ch.id, ch.last_seen_at);
      // Reset agent active state — real-time events will re-set if agent is mid-turn.
      state.channelAgentActive.set(ch.id, false);
    }
    updateStopButton();
    state.deviceChannels.set(deviceId, devChans);
    // Clean up channels removed by the device.
    for (const oldId of oldIds) {
      if (!devChans.has(oldId)) {
        state.channelDeviceMap.delete(oldId);
        state.chatMessages.delete(oldId);
        state.unreadCounts.delete(oldId);
        state.channelSortTs.delete(oldId);
      }
    }
    rebuildChatChannels();
    renderChannelList();
    // If current channel was removed by the device, auto-select another.
    if (state.chatCurrentChannel && oldIds.has(state.chatCurrentChannel) && !devChans.has(state.chatCurrentChannel)) {
      const remaining = [...state.chatChannels.values()];
      if (remaining.length > 0) selectChannel(remaining[0].id);
      else { state.chatCurrentChannel = null; renderMessages(); }
    }
    // Restore pending channel from hash.
    if (state._pendingChannelId && state.chatChannels.has(state._pendingChannelId)) {
      selectChannel(state._pendingChannelId);
      state._pendingChannelId = null;
    } else if (!state.chatCurrentChannel && !state._pendingChannelId && channels.length > 0) {
      // Only auto-select first channel if there's no pending channel waiting for another device.
      selectChannel(channels[0].id);
    }
    // Fetch messages for all non-current channels to compute unread counts.
    for (const ch of channels) {
      if (ch.id !== state.chatCurrentChannel && instance.connected) {
        instance.getMessages(ch.id);
      }
    }
  });

  instance.addEventListener('channel_created', (evt) => {
    const ch = evt.detail;
    state.channelDeviceMap.set(ch.id, deviceId);
    const devChans = state.deviceChannels.get(deviceId) || new Map();
    devChans.set(ch.id, ch);
    state.deviceChannels.set(deviceId, devChans);
    rebuildChatChannels();
    renderChannelList();
    selectChannel(ch.id);
  });

  // ----- Harness & Agent events -----

  instance.addEventListener('channel_renamed', (evt) => {
    const { channel_id, name } = evt.detail;
    const ch = state.chatChannels.get(channel_id);
    if (ch) {
      ch.name = name;
      const devChans = state.deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) devChans.get(channel_id).name = name;
      renderChannelList();
      renderChannelSidebar();
    }
  });

  instance.addEventListener('channel_updated', (evt) => {
    const { channel_id, model, effort, working_directory } = evt.detail;
    const ch = state.chatChannels.get(channel_id);
    if (ch) {
      if (model) ch.model = model;
      if (effort !== undefined) ch.effort = effort;
      if (working_directory !== undefined) ch.working_directory = working_directory;
      const devChans = state.deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) {
        const dc = devChans.get(channel_id);
        if (model) dc.model = model;
        if (effort !== undefined) dc.effort = effort;
        if (working_directory !== undefined) dc.working_directory = working_directory;
      }
    }
    // If this is the active channel, refresh the chat overlay model/effort pills.
    if (state.chatCurrentChannel === channel_id) applyChannelHarnessInfo(channel_id);
  });

  instance.addEventListener('channel_deleted', (evt) => {
    const { channel_id } = evt.detail;
    state.channelDeviceMap.delete(channel_id);
    const devChans = state.deviceChannels.get(deviceId);
    if (devChans) devChans.delete(channel_id);
    rebuildChatChannels();
    state.unreadCounts.delete(channel_id);
    if (state.chatCurrentChannel === channel_id) {
      const remaining = [...state.chatChannels.values()];
      if (remaining.length > 0) {
        selectChannel(remaining[0].id);
      } else {
        state.chatCurrentChannel = null;
        renderMessages();
      }
    }
    renderChannelList();
    renderChannelSidebar();
  });
}
