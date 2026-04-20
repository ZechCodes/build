import { state } from '../state.js';

export function getE2EE(channelId) {
  const deviceId = state.channelDeviceMap.get(channelId);
  return deviceId ? state.e2eeConnections.get(deviceId) : null;
}

export function getActiveE2EE() {
  return getE2EE(state.chatCurrentChannel);
}

export function anyE2EEConnected() {
  for (const conn of state.e2eeConnections.values()) if (conn.connected) return true;
  return false;
}
