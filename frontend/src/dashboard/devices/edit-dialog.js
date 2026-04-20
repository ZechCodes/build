import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';

// External deps kept on window during transition:
//   window.renderChannelPanel, window.fetchDevices

export function showEditDeviceDialog(device) {
  const dialogParent = document.body;
  if (!dialogParent) return;

  const overlay = document.createElement('div');
  overlay.className = 'new-channel-overlay';
  overlay.innerHTML = `
    <div class="new-channel-dialog">
      <h3>Edit Device</h3>
      <label for="edit-device-name">Name</label>
      <input type="text" id="edit-device-name" value="${escapeHtml(device.name)}" autofocus>
      <div class="edit-channel-actions">
        <button class="btn" id="edit-device-restart">Restart Device</button>
      </div>
      <div class="edit-channel-danger">
        <div class="edit-channel-danger-label">Danger Zone</div>
        <button class="btn-danger" id="edit-device-revoke">Revoke Device</button>
      </div>
      <div class="dialog-btns">
        <button class="btn btn-cancel" id="edit-device-cancel">Cancel</button>
        <button class="btn btn-create" id="edit-device-save">Save</button>
      </div>
    </div>
  `;
  dialogParent.appendChild(overlay);

  const nameInput = document.getElementById('edit-device-name');
  nameInput.focus();
  nameInput.select();

  const close = () => overlay.remove();

  document.getElementById('edit-device-cancel').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });

  const save = async () => {
    const newName = nameInput.value.trim();
    if (newName && newName !== device.name) {
      try {
        await fetch(`/api/devices/${device.id}`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: newName }),
        });
        device.name = newName;
        window.renderChannelPanel?.();
      } catch (err) {
        console.error('Failed to rename device:', err);
      }
    }
    close();
  };
  document.getElementById('edit-device-save').addEventListener('click', save);
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(); });

  document.getElementById('edit-device-restart').addEventListener('click', async () => {
    try {
      await fetch(`/api/devices/${device.id}/restart`, { method: 'POST' });
    } catch (err) {
      console.error('Failed to restart device:', err);
    }
    close();
  });

  const revokeBtn = document.getElementById('edit-device-revoke');
  let revokeConfirm = false;
  revokeBtn.addEventListener('click', async () => {
    if (!revokeConfirm) {
      revokeConfirm = true;
      revokeBtn.textContent = 'Click again to confirm revocation';
      revokeBtn.classList.add('btn-danger-confirm');
      setTimeout(() => {
        revokeConfirm = false;
        revokeBtn.textContent = 'Revoke Device';
        revokeBtn.classList.remove('btn-danger-confirm');
      }, 3000);
    } else {
      try {
        await fetch(`/api/devices/${device.id}`, { method: 'DELETE' });
        state.devices.delete(device.id);
        const _revokeConn = state.e2eeConnections.get(device.id);
        if (_revokeConn) _revokeConn.disconnect();
        window.renderChannelPanel?.();
        window.fetchDevices?.();
      } catch (err) {
        console.error('Failed to revoke device:', err);
      }
      close();
    }
  });
}
