import { escapeHtml } from '../util/html.js';
import { timeAgo } from '../util/time.js';

export function renderDeviceCard(device) {
  const isOnline = device.status === 'online';
  const statusClass = isOnline ? '' : 'offline';
  const statusText = isOnline ? 'Online' : 'Offline';
  const heartbeatText = device.last_heartbeat_at
    ? timeAgo(device.last_heartbeat_at)
    : 'never';
  const missedCount = (device.missed_heartbeat_windows || []).length;

  return `
    <div class="device-card glass" data-device-id="${device.id}">
      <div class="device-header">
        <div class="device-icon ${statusClass}">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
            <rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>
          </svg>
        </div>
        <span class="device-name">${escapeHtml(device.name)}</span>
        <div class="device-status">
          <span class="dot ${statusClass}"></span>
          <span>${statusText}</span>
        </div>
      </div>
      <div class="device-meta device-meta-row">
        <span title="Last heartbeat">Last beat: ${heartbeatText}</span>
        <span title="Heartbeat interval">Interval: ${device.heartbeat_interval_s || 30}s</span>
        ${missedCount > 0 ? `<span class="text-amber" title="Missed heartbeat windows">${missedCount} missed</span>` : ''}
      </div>
    </div>
  `;
}
