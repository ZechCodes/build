import { state } from '../state.js';
import { effortLabel } from '../util/format.js';
import { anyE2EEConnected } from '../e2ee/bridge.js';

document.getElementById('btn-new-channel')?.addEventListener('click', () => {
  if (!anyE2EEConnected()) {
    alert('E2EE not connected. Waiting for device...');
    return;
  }

  // Determine which device to create the channel on.
  const _targetDeviceId = state.channelDeviceMap.get(state.chatCurrentChannel) || state.e2eeConnections.keys().next().value;
  const _createConn = state.e2eeConnections.get(_targetDeviceId);

  const dialogParent = document.body;
  const overlay = document.createElement('div');
  overlay.className = 'new-channel-overlay';

  const cachedHarnesses = state.deviceHarnesses.get(_targetDeviceId) || [];
  let harnessOptions = '<option value="">No agent</option>';
  if (cachedHarnesses.length) {
    harnessOptions = cachedHarnesses.map(h =>
      `<option value="${h.id}">${h.name}</option>`
    ).join('');
  }

  overlay.innerHTML = `
    <div class="new-channel-dialog">
      <h3>New Channel</h3>
      <input type="text" id="new-channel-name" placeholder="Channel name..." autofocus>
      <label for="new-channel-harness">Harness</label>
      <select id="new-channel-harness">${harnessOptions}</select>
      <label for="new-channel-model">Model</label>
      <select id="new-channel-model"><option value="">Select a harness first</option></select>
      <label for="new-channel-effort">Effort</label>
      <select id="new-channel-effort"><option value="">Default</option></select>
      <button class="new-channel-advanced-toggle" type="button" id="new-channel-advanced-toggle">▶ Advanced</button>
      <div class="new-channel-advanced" id="new-channel-advanced">
        <label for="new-channel-workdir">Working Directory</label>
        <input type="text" id="new-channel-workdir" placeholder="/path/to/project">
        <label for="new-channel-prompt">System Prompt</label>
        <textarea id="new-channel-prompt" placeholder="Optional agent instructions..." rows="2"></textarea>
        <label class="new-channel-checkbox">
          <input type="checkbox" id="new-channel-auto-approve">
          Auto-approve all tool uses
        </label>
      </div>
      <div class="dialog-btns">
        <button class="btn btn-cancel" id="new-channel-cancel">Cancel</button>
        <button class="btn btn-create" id="new-channel-create">Create</button>
      </div>
    </div>
  `;
  dialogParent.appendChild(overlay);

  const nameInput = document.getElementById('new-channel-name');
  const harnessSelect = document.getElementById('new-channel-harness');
  const modelSelect = document.getElementById('new-channel-model');
  const advancedToggle = document.getElementById('new-channel-advanced-toggle');
  const advancedSection = document.getElementById('new-channel-advanced');
  nameInput.focus();

  const effortSelect = document.getElementById('new-channel-effort');

  function updateHarnessFields() {
    const harnessId = harnessSelect.value;
    modelSelect.innerHTML = '';
    effortSelect.innerHTML = '<option value="">Default</option>';
    if (!harnessId || !cachedHarnesses) {
      modelSelect.innerHTML = '<option value="">Select a harness first</option>';
      return;
    }
    const harness = cachedHarnesses.find(h => h.id === harnessId);
    if (!harness) return;
    for (const m of harness.models) {
      const opt = document.createElement('option');
      opt.value = m.id;
      opt.textContent = m.name;
      if (m.id === harness.default_model) opt.selected = true;
      modelSelect.appendChild(opt);
    }
    for (const lvl of (harness.effort_levels || [])) {
      const opt = document.createElement('option');
      opt.value = lvl;
      opt.textContent = effortLabel(lvl);
      if (lvl === harness.default_effort) opt.selected = true;
      effortSelect.appendChild(opt);
    }
  }
  harnessSelect.addEventListener('change', updateHarnessFields);
  updateHarnessFields();

  advancedToggle.addEventListener('click', () => {
    advancedSection.classList.toggle('is-open');
    advancedToggle.textContent = advancedSection.classList.contains('is-open') ? '▼ Advanced' : '▶ Advanced';
  });

  const close = () => overlay.remove();
  document.getElementById('new-channel-cancel').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });

  const create = () => {
    const name = nameInput.value.trim();
    if (name) {
      const opts = {};
      if (harnessSelect.value) opts.harness = harnessSelect.value;
      if (modelSelect.value) opts.model = modelSelect.value;
      const effortVal = document.getElementById('new-channel-effort').value;
      if (effortVal) opts.effort = effortVal;
      const wd = document.getElementById('new-channel-workdir').value.trim();
      if (wd) opts.working_directory = wd;
      const sp = document.getElementById('new-channel-prompt').value.trim();
      if (sp) opts.system_prompt = sp;
      opts.auto_approve_tools = document.getElementById('new-channel-auto-approve').checked;
      _createConn?.createChannel(name, opts);
      close();
    }
  };
  document.getElementById('new-channel-create').addEventListener('click', create);
  nameInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') create();
    if (e.key === 'Escape') close();
  });
});
