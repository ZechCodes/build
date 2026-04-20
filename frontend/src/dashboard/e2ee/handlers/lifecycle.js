import { state } from '../../state.js';
import { anyE2EEConnected } from '../bridge.js';
import { syncE2EEStatus } from '../connect.js';
import { renderChannelPanel } from '../../channels/panel.js';
import { _bindUploadProgress } from '../../chat/uploads.js';
import { onFilesTabActivated } from '../../files/mode.js';

export function bindLifecycleHandlers(instance, deviceId) {
  instance.addEventListener('connected', () => {
    console.log('[E2EE] Session established for device', deviceId);
    state.e2eeHasConnected = true;
    const _enableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
    for (const id of _enableBtns) {
      const el = document.getElementById(id);
      if (el) el.disabled = false;
    }
    document.getElementById('e2ee-waiting-overlay').classList.add('hidden');
    // Hide reconnect pill + cancel timer.
    clearTimeout(state._reconnectPillTimer);
    state._reconnectPillTimer = null;
    document.getElementById('reconnect-pill').classList.remove('visible');
    // Clear device-down banner on successful reconnect.
    state.deviceDown = null;
    const _ddb = document.getElementById('device-down-banner');
    if (_ddb) _ddb.classList.remove('visible');
    if (typeof _bindUploadProgress === 'function') _bindUploadProgress(instance);
    syncE2EEStatus();
    renderChannelPanel();
    instance.listChannels();
    instance.listHarnesses();
    // Re-fetch history for the current channel if it belongs to this device.
    if (state.chatCurrentChannel && state.channelDeviceMap.get(state.chatCurrentChannel) === deviceId) {
      state.chatLoadingMessages.add(state.chatCurrentChannel);
      state.chatLoadingActivity.add(state.chatCurrentChannel);
      instance.getMessages(state.chatCurrentChannel);
      instance.getActivity(state.chatCurrentChannel);
      // If the files tab is active, retry the tree/changes fetch — the initial
      // selectChannel() may have run before this `connected` flag flipped.
      if (state.currentTab === 'files') {
        state.filesChannelId = null;
        if (typeof onFilesTabActivated === 'function') onFilesTabActivated();
      }
    }
  });

  instance.addEventListener('disconnected', () => {
    console.log('[E2EE] Disconnected device', deviceId);
    state.e2eeConnections.delete(deviceId);
    // Keep state.deviceChannels and state.channelDeviceMap cached — only clear on server-reported removal.
    state.deviceHarnesses.delete(deviceId);
    state.deviceAgentCwd.delete(deviceId);

    if (!anyE2EEConnected()) {
      const _disableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
      for (const id of _disableBtns) {
        const el = document.getElementById(id);
        if (el) el.disabled = true;
      }
      if (state.deviceDown) {
        // Device is known to be offline — keep banner visible, skip skeleton/pill.
      } else if (!state.e2eeHasConnected) {
        // First load — show skeleton.
        document.getElementById('e2ee-waiting-overlay').classList.remove('hidden');
      } else {
        // Reconnection — show pill after 2s delay.
        clearTimeout(state._reconnectPillTimer);
        state._reconnectPillTimer = setTimeout(() => {
          if (!anyE2EEConnected()) {
            document.getElementById('reconnect-pill').classList.add('visible');
          }
        }, 2000);
      }
    }
    syncE2EEStatus();
    renderChannelPanel();
  });
}
