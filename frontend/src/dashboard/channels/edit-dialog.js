import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { effortLabel } from '../util/format.js';
import { getE2EE } from '../e2ee/bridge.js';

export function showEditChannelDialog(ch) {
  const _editConn = getE2EE(ch.id);
  if (!_editConn || !_editConn.connected) return;
  const dialogParent = document.body;
  if (!dialogParent) return;

  const deviceId = state.channelDeviceMap.get(ch.id);
  const cachedHarnesses = deviceId ? state.deviceHarnesses.get(deviceId) : [];
  const harness = cachedHarnesses?.find(h => h.id === ch.harness);
  let modelOptions = '';
  if (harness) {
    modelOptions = harness.models.map(m =>
      `<option value="${m.id}"${m.id === ch.model ? ' selected' : ''}>${m.name}</option>`
    ).join('');
  }

  const harnessEffortLevels = harness?.effort_levels || [];
  const effortOptions =
    `<option value=""${(ch.effort || '') === '' ? ' selected' : ''}>Default</option>` +
    harnessEffortLevels.map(lvl =>
      `<option value="${lvl}"${lvl === (ch.effort || '') ? ' selected' : ''}>${effortLabel(lvl)}</option>`
    ).join('');

  const overlay = document.createElement('div');
  overlay.className = 'new-channel-overlay';
  overlay.innerHTML = `
    <div class="new-channel-dialog">
      <h3>Edit Channel</h3>
      <label for="edit-channel-name">Name</label>
      <input type="text" id="edit-channel-name" value="${escapeHtml(ch.name)}" autofocus>
      ${modelOptions ? `<label for="edit-channel-model">Model</label>
      <select id="edit-channel-model">${modelOptions}</select>` : ''}
      <label for="edit-channel-effort">Effort</label>
      <select id="edit-channel-effort">${effortOptions}</select>
      <label for="edit-channel-workdir">Working Directory</label>
      <input type="text" id="edit-channel-workdir" value="${escapeHtml(ch.working_directory || '')}" placeholder="/path/to/project">
      <label class="new-channel-checkbox">
        <input type="checkbox" id="edit-channel-auto-approve"${ch.auto_approve_tools ? ' checked' : ''}>
        Auto-approve all tool uses
      </label>
      <div class="edit-channel-actions">
        <button class="btn" id="edit-channel-restart">Restart Agent</button>
      </div>
      <div class="edit-channel-danger">
        <div class="edit-channel-danger-label">Danger Zone</div>
        <button class="btn-danger" id="edit-channel-delete">Delete Channel</button>
      </div>
      <div class="dialog-btns">
        <button class="btn btn-cancel" id="edit-channel-cancel">Cancel</button>
        <button class="btn btn-create" id="edit-channel-save">Save</button>
      </div>
    </div>
  `;
  dialogParent.appendChild(overlay);

  const nameInput = document.getElementById('edit-channel-name');
  nameInput.focus();
  nameInput.select();

  const close = () => overlay.remove();

  document.getElementById('edit-channel-cancel').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });

  const workdirInput = document.getElementById('edit-channel-workdir');
  const modelSelect = document.getElementById('edit-channel-model');
  const effortSelect = document.getElementById('edit-channel-effort');
  const save = () => {
    const newName = nameInput.value.trim();
    if (newName && newName !== ch.name) {
      _editConn.renameChannel(ch.id, newName);
    }
    const updates = {};
    const newWorkdir = workdirInput.value.trim();
    if (newWorkdir !== (ch.working_directory || '')) updates.working_directory = newWorkdir;
    const newModel = modelSelect?.value;
    if (newModel && newModel !== ch.model) updates.model = newModel;
    const newEffort = effortSelect.value;
    if (newEffort !== (ch.effort || '')) updates.effort = newEffort;
    const newAutoApprove = document.getElementById('edit-channel-auto-approve').checked;
    if (newAutoApprove !== !!ch.auto_approve_tools) updates.auto_approve_tools = newAutoApprove;
    if (Object.keys(updates).length) {
      _editConn.updateChannel(ch.id, updates);
      if (updates.model) ch.model = updates.model;
      if (updates.effort !== undefined) ch.effort = updates.effort;
      if (updates.auto_approve_tools !== undefined) ch.auto_approve_tools = updates.auto_approve_tools;
    }
    close();
  };
  document.getElementById('edit-channel-save').addEventListener('click', save);
  nameInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(); });
  workdirInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(); });

  document.getElementById('edit-channel-restart').addEventListener('click', () => {
    _editConn.restartAgent(ch.id);
    close();
  });

  const deleteBtn = document.getElementById('edit-channel-delete');
  let deleteConfirm = false;
  deleteBtn.addEventListener('click', () => {
    if (!deleteConfirm) {
      deleteConfirm = true;
      deleteBtn.textContent = 'Click again to confirm deletion';
      deleteBtn.classList.add('btn-danger-confirm');
      setTimeout(() => {
        deleteConfirm = false;
        deleteBtn.textContent = 'Delete Channel';
        deleteBtn.classList.remove('btn-danger-confirm');
      }, 3000);
    } else {
      _editConn.deleteChannel(ch.id);
      close();
    }
  });
}
