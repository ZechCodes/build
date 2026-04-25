// Per-channel actions. Fired from the "…" trigger on each sidebar
// channel row. Menu offers quick actions (Restart / Stop) + an
// "Edit channel…" item that opens a modal with Name, Working
// directory, and a Delete button.
//
// All popovers / the modal live at document.body so they can
// outline any clipping ancestors.

import { bus } from '../core/bus.js';
import { uiStore } from '../domain/ui-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { presenceStore } from '../domain/presence-store.js';
import { showToast } from '../util/toast.js';
import { escapeHtml } from '../util/html.js';

let activeMenu = null;    // { el, channelId, onDocClick, onClick }
let activeModal = null;   // { el, channelId, onKey }

// ── Popup menu ───────────────────────────────────────────────────────

export function openChannelMenu(channelId, anchorEl) {
  if (!channelId || !anchorEl) return;
  if (activeMenu && activeMenu.channelId === channelId) {
    closeChannelMenu();
    return;
  }
  closeChannelMenu();

  const menu = document.createElement('div');
  menu.className = 'v2-channel-menu';
  menu.setAttribute('data-channel-id', channelId);
  menu.innerHTML = `
    <button type="button" data-ch-action="restart">
      <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
        <path d="M2 8a6 6 0 1110.4 4M13 3v3h-3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      Restart agent
    </button>
    <button type="button" data-ch-action="stop">
      <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
        <rect x="4" y="4" width="8" height="8" rx="1" stroke="currentColor" stroke-width="1.3"/>
      </svg>
      Stop agent
    </button>
    <div class="v2-channel-menu-sep"></div>
    <button type="button" data-ch-action="edit">
      <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
        <path d="M11.5 3.5l1 1M2 14l4-1 7-7-3-3-7 7z" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      Edit channel…
    </button>
  `;
  document.body.appendChild(menu);

  // Position: prefer below-left of the anchor, flip up / clamp to viewport.
  const r = anchorEl.getBoundingClientRect();
  const mr = menu.getBoundingClientRect();
  const margin = 8;
  let top = r.bottom + 4;
  let left = r.left;
  if (left + mr.width > window.innerWidth - margin) {
    left = Math.max(margin, window.innerWidth - mr.width - margin);
  }
  if (top + mr.height > window.innerHeight - margin) {
    top = Math.max(margin, r.top - mr.height - 4);
  }
  menu.style.top  = `${Math.round(top)}px`;
  menu.style.left = `${Math.round(left)}px`;

  const onClick = (e) => {
    const btn = e.target.closest('[data-ch-action]');
    if (!btn) return;
    const action = btn.getAttribute('data-ch-action');
    closeChannelMenu();
    if (action === 'restart') {
      bus.emit('intent.restart_agent', { channelId });
      showToast('Restarting agent…');
    } else if (action === 'stop') {
      bus.emit('intent.stop_agent', { channelId });
      showToast('Stopping agent…');
    } else if (action === 'edit') {
      openChannelEditModal(channelId);
    }
  };
  menu.addEventListener('click', onClick);

  const onDocClick = (e) => {
    if (e.target.closest('.v2-channel-menu')) return;
    closeChannelMenu();
  };
  setTimeout(() => document.addEventListener('click', onDocClick), 0);

  activeMenu = { el: menu, channelId, onClick, onDocClick };
}

export function closeChannelMenu() {
  if (!activeMenu) return;
  const { el, onClick, onDocClick } = activeMenu;
  el.removeEventListener('click', onClick);
  document.removeEventListener('click', onDocClick);
  el.remove();
  activeMenu = null;
}

// ── Edit modal ───────────────────────────────────────────────────────

export function openChannelEditModal(channelId) {
  if (!channelId) return;
  closeChannelEditModal();

  const ch = channelsStore.get(channelId);
  const deviceId = channelsStore.deviceFor(channelId);
  const name    = ch?.name || '';
  const cwd     = ch?.working_directory || '';
  const curHarness = ch?.harness || '';
  const curModel   = ch?.model || '';
  const curEffort  = ch?.effort || '';
  const curAutoApprove = !!ch?.auto_approve_tools;

  let harnesses = deviceId ? (presenceStore.getHarnesses(deviceId) || []) : [];

  const root = document.createElement('div');
  root.className = 'v2-modal-backdrop';
  root.setAttribute('role', 'dialog');
  root.setAttribute('aria-modal', 'true');
  root.innerHTML = `
    <div class="v2-modal" data-channel-id="${escapeHtml(channelId)}">
      <header class="v2-modal-header">
        <h2 class="v2-modal-title">Edit channel</h2>
        <button class="v2-modal-close" type="button" data-modal-action="cancel"
                aria-label="Close">
          <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
        </button>
      </header>
      <div class="v2-modal-body">
        <label class="v2-modal-field">
          <span class="v2-modal-label">Name</span>
          <input class="v2-modal-input" data-edit-field="name"
                 type="text" autocomplete="off" spellcheck="false">
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Working directory</span>
          <input class="v2-modal-input" data-edit-field="cwd"
                 type="text" autocomplete="off" spellcheck="false"
                 placeholder="~/Projects/repo">
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Harness</span>
          <select class="v2-modal-select" data-edit-field="harness"></select>
          <span class="v2-modal-hint">Changing the harness restarts the agent.</span>
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Model</span>
          <select class="v2-modal-select" data-edit-field="model"></select>
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Effort</span>
          <select class="v2-modal-select" data-edit-field="effort"></select>
        </label>
        <label class="v2-modal-checkbox">
          <input type="checkbox" data-edit-field="auto_approve_tools">
          <span>Auto-approve all tool uses</span>
        </label>
      </div>
      <footer class="v2-modal-footer">
        <button type="button" class="v2-modal-delete" data-modal-action="delete-start">
          Delete channel
        </button>
        <div class="v2-modal-delete-confirm" hidden>
          <span>Delete this channel and stop its agent?</span>
          <button type="button" class="v2-modal-btn secondary" data-modal-action="delete-cancel">Cancel</button>
          <button type="button" class="v2-modal-btn danger"    data-modal-action="delete-confirm">Delete</button>
        </div>
        <div class="v2-modal-primary-actions">
          <button type="button" class="v2-modal-btn secondary" data-modal-action="cancel">Cancel</button>
          <button type="button" class="v2-modal-btn primary"   data-modal-action="save">Save</button>
        </div>
      </footer>
    </div>
  `;
  document.body.appendChild(root);

  const modal = root.querySelector('.v2-modal');
  modal.querySelector('[data-edit-field="name"]').value = name;
  modal.querySelector('[data-edit-field="cwd"]').value  = cwd;
  modal.querySelector('[data-edit-field="auto_approve_tools"]').checked = curAutoApprove;

  const harnessSel = modal.querySelector('[data-edit-field="harness"]');
  const modelSel   = modal.querySelector('[data-edit-field="model"]');
  const effortSel  = modal.querySelector('[data-edit-field="effort"]');

  const refreshHarnessOptions = () => {
    const selected = harnessSel.value || curHarness;
    if (!harnesses.length) {
      harnessSel.innerHTML = `<option value="${escapeHtml(curHarness)}" selected>${escapeHtml(curHarness || '(no harnesses available)')}</option>`;
      return;
    }
    harnessSel.innerHTML = harnesses.map(h => {
      const isSel = h.id === selected ? ' selected' : '';
      return `<option value="${escapeHtml(h.id)}"${isSel}>${escapeHtml(h.name || h.id)}</option>`;
    }).join('');
  };

  const refreshModelEffort = () => {
    const h = harnesses.find(x => x.id === harnessSel.value) || null;
    const models  = h?.models || [];
    const efforts = h?.effort_levels || ['low', 'medium', 'high'];
    const harnessUnchanged = harnessSel.value === curHarness;
    const preferredModel  = harnessUnchanged ? (modelSel.value || curModel) : (h?.default_model || '');
    const preferredEffort = harnessUnchanged ? (effortSel.value || curEffort) : (h?.default_effort || '');
    modelSel.innerHTML = models.length
      ? models.map(m => {
          const isSel = m.id === preferredModel ? ' selected' : '';
          return `<option value="${escapeHtml(m.id)}"${isSel}>${escapeHtml(m.name || m.id)}</option>`;
        }).join('')
      : `<option value="${escapeHtml(curModel)}" selected>${escapeHtml(curModel || '(no models available)')}</option>`;
    effortSel.innerHTML = efforts.map(e => {
      const isSel = e === preferredEffort ? ' selected' : '';
      return `<option value="${escapeHtml(e)}"${isSel}>${escapeHtml(e)}</option>`;
    }).join('');
  };

  const rerender = () => {
    refreshHarnessOptions();
    refreshModelEffort();
  };

  harnessSel.addEventListener('change', refreshModelEffort);
  rerender();

  // Harness list may arrive after the modal opens — keep the dropdowns fresh.
  const offHarnessList = bus.on('harness.list', (evt) => {
    if (!evt || !deviceId || evt.deviceId !== deviceId) return;
    harnesses = presenceStore.getHarnesses(deviceId) || [];
    rerender();
  });

  // Focus the name input so Enter / typing starts working.
  setTimeout(() => modal.querySelector('[data-edit-field="name"]').focus(), 0);

  const commit = () => {
    const nextName    = modal.querySelector('[data-edit-field="name"]').value.trim();
    const nextCwd     = modal.querySelector('[data-edit-field="cwd"]').value.trim();
    const nextHarness = harnessSel.value;
    const nextModel   = modelSel.value;
    const nextEffort  = effortSel.value;
    const nextAuto    = modal.querySelector('[data-edit-field="auto_approve_tools"]').checked;
    let any = false;
    if (nextName && nextName !== name) {
      bus.emit('intent.rename_channel', { channelId, name: nextName });
      any = true;
    }
    const patch = {};
    if (nextCwd !== cwd) patch.working_directory = nextCwd;
    if (nextHarness && nextHarness !== curHarness) patch.harness = nextHarness;
    if (nextModel && nextModel !== curModel) patch.model = nextModel;
    if (nextEffort && nextEffort !== curEffort) patch.effort = nextEffort;
    if (nextAuto !== curAutoApprove) patch.auto_approve_tools = nextAuto;
    if (Object.keys(patch).length) {
      bus.emit('intent.update_channel', { channelId, patch });
      any = true;
    }
    if (any) showToast('Channel updated');
    closeChannelEditModal();
  };

  const remove = () => closeChannelEditModal();

  const onClick = (e) => {
    const action = e.target.closest('[data-modal-action]')?.getAttribute('data-modal-action');
    if (!action) {
      // Click on backdrop (outside the panel) closes. Inside the
      // panel does nothing.
      if (e.target === root) closeChannelEditModal();
      return;
    }
    if (action === 'cancel') return remove();
    if (action === 'save')   return commit();
    if (action === 'delete-start') {
      modal.querySelector('.v2-modal-primary-actions').hidden = true;
      modal.querySelector('.v2-modal-delete').hidden = true;
      modal.querySelector('.v2-modal-delete-confirm').hidden = false;
      return;
    }
    if (action === 'delete-cancel') {
      modal.querySelector('.v2-modal-delete-confirm').hidden = true;
      modal.querySelector('.v2-modal-delete').hidden = false;
      modal.querySelector('.v2-modal-primary-actions').hidden = false;
      return;
    }
    if (action === 'delete-confirm') {
      bus.emit('intent.delete_channel', { channelId });
      showToast('Channel deleted');
      closeChannelEditModal();
    }
  };
  root.addEventListener('click', onClick);

  const onKey = (e) => {
    if (e.key === 'Escape') { e.preventDefault(); remove(); }
    else if (e.key === 'Enter' && e.target.matches('[data-edit-field]')) {
      e.preventDefault();
      commit();
    }
  };
  root.addEventListener('keydown', onKey);

  activeModal = { el: root, channelId, onClick, onKey, cleanup: offHarnessList };
}

export function closeChannelEditModal() {
  if (!activeModal) return;
  const { el, onClick, onKey, cleanup } = activeModal;
  el.removeEventListener('click', onClick);
  el.removeEventListener('keydown', onKey);
  if (typeof cleanup === 'function') cleanup();
  el.remove();
  activeModal = null;
}

// ── New-channel modal ────────────────────────────────────────────────

export function openChannelNewModal(deviceId) {
  if (!deviceId) return;
  closeChannelEditModal();

  let harnesses = presenceStore.getHarnesses(deviceId) || [];

  const root = document.createElement('div');
  root.className = 'v2-modal-backdrop';
  root.setAttribute('role', 'dialog');
  root.setAttribute('aria-modal', 'true');
  root.innerHTML = `
    <div class="v2-modal" data-device-id="${escapeHtml(deviceId)}">
      <header class="v2-modal-header">
        <h2 class="v2-modal-title">New channel</h2>
        <button class="v2-modal-close" type="button" data-modal-action="cancel" aria-label="Close">
          <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
        </button>
      </header>
      <div class="v2-modal-body">
        <label class="v2-modal-field">
          <span class="v2-modal-label">Name</span>
          <input class="v2-modal-input" data-new-field="name"
                 type="text" autocomplete="off" spellcheck="false"
                 placeholder="e.g. Refactor auth">
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Working directory</span>
          <input class="v2-modal-input" data-new-field="cwd"
                 type="text" autocomplete="off" spellcheck="false"
                 placeholder="~/Projects/repo">
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Harness</span>
          <select class="v2-modal-select" data-new-field="harness"></select>
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Model</span>
          <select class="v2-modal-select" data-new-field="model"></select>
        </label>
        <label class="v2-modal-field">
          <span class="v2-modal-label">Effort</span>
          <select class="v2-modal-select" data-new-field="effort"></select>
        </label>
        <button type="button" class="v2-modal-advanced-toggle" data-modal-action="advanced">
          <span class="v2-modal-advanced-caret">▶</span> Advanced
        </button>
        <div class="v2-modal-advanced" hidden>
          <label class="v2-modal-field">
            <span class="v2-modal-label">System prompt</span>
            <textarea class="v2-modal-textarea" data-new-field="system_prompt"
                      rows="3" spellcheck="false"
                      placeholder="Optional agent instructions…"></textarea>
          </label>
          <label class="v2-modal-checkbox">
            <input type="checkbox" data-new-field="auto_approve_tools">
            <span>Auto-approve all tool uses</span>
          </label>
        </div>
      </div>
      <footer class="v2-modal-footer">
        <div class="v2-modal-primary-actions">
          <button type="button" class="v2-modal-btn secondary" data-modal-action="cancel">Cancel</button>
          <button type="button" class="v2-modal-btn primary"   data-modal-action="create">Create</button>
        </div>
      </footer>
    </div>
  `;
  document.body.appendChild(root);

  const modal = root.querySelector('.v2-modal');
  const harnessSel = modal.querySelector('[data-new-field="harness"]');
  const modelSel   = modal.querySelector('[data-new-field="model"]');
  const effortSel  = modal.querySelector('[data-new-field="effort"]');
  const advCaret   = modal.querySelector('.v2-modal-advanced-caret');
  const advSection = modal.querySelector('.v2-modal-advanced');

  const refreshHarnessOptions = () => {
    const prev = harnessSel.value;
    if (!harnesses.length) {
      harnessSel.innerHTML = '<option value="">(no harnesses available)</option>';
    } else {
      harnessSel.innerHTML = harnesses.map(h =>
        `<option value="${escapeHtml(h.id)}">${escapeHtml(h.name || h.id)}</option>`,
      ).join('');
      if (prev && harnesses.some(h => h.id === prev)) harnessSel.value = prev;
    }
  };

  const refreshModelEffort = () => {
    const h = harnesses.find(x => x.id === harnessSel.value) || harnesses[0] || null;
    const models  = h?.models || [];
    const efforts = h?.effort_levels || ['low', 'medium', 'high'];
    const prevModel  = modelSel.value;
    const prevEffort = effortSel.value;
    modelSel.innerHTML = models.length
      ? models.map(m => {
          const selected = m.id === (prevModel || h?.default_model) ? ' selected' : '';
          return `<option value="${escapeHtml(m.id)}"${selected}>${escapeHtml(m.name || m.id)}</option>`;
        }).join('')
      : '<option value="">(harness has no models)</option>';
    effortSel.innerHTML = efforts.map(e => {
      const selected = e === (prevEffort || h?.default_effort) ? ' selected' : '';
      return `<option value="${escapeHtml(e)}"${selected}>${escapeHtml(e)}</option>`;
    }).join('');
  };

  const rerender = () => {
    refreshHarnessOptions();
    refreshModelEffort();
  };

  harnessSel.addEventListener('change', refreshModelEffort);
  rerender();

  // Harness list may arrive after the modal opens — keep the dropdowns fresh.
  const offHarnessList = bus.on('harness.list', (evt) => {
    if (!evt || evt.deviceId !== deviceId) return;
    harnesses = presenceStore.getHarnesses(deviceId) || [];
    rerender();
  });

  setTimeout(() => modal.querySelector('[data-new-field="name"]').focus(), 0);

  const toggleAdvanced = () => {
    const open = advSection.hidden;
    advSection.hidden = !open;
    advCaret.textContent = open ? '▼' : '▶';
  };

  const create = () => {
    const nameV   = modal.querySelector('[data-new-field="name"]').value.trim();
    const cwdV    = modal.querySelector('[data-new-field="cwd"]').value.trim();
    const harV    = harnessSel.value;
    const modelV  = modelSel.value;
    const effortV = effortSel.value;
    const spV     = modal.querySelector('[data-new-field="system_prompt"]').value.trim();
    const aatV    = modal.querySelector('[data-new-field="auto_approve_tools"]').checked;
    if (!nameV) { showToast('Name is required', { kind: 'error' }); return; }
    bus.emit('intent.create_channel', {
      deviceId,
      name: nameV,
      harness: harV || undefined,
      model:   modelV || undefined,
      effort:  effortV || undefined,
      working_directory: cwdV || undefined,
      system_prompt: spV || undefined,
      auto_approve_tools: aatV,
    });
    showToast('Creating channel…');
    closeChannelEditModal();
  };

  const onClick = (e) => {
    const action = e.target.closest('[data-modal-action]')?.getAttribute('data-modal-action');
    if (!action) {
      if (e.target === root) closeChannelEditModal();
      return;
    }
    if (action === 'cancel')   return closeChannelEditModal();
    if (action === 'advanced') return toggleAdvanced();
    if (action === 'create')   return create();
  };
  root.addEventListener('click', onClick);

  const onKey = (e) => {
    if (e.key === 'Escape') { e.preventDefault(); closeChannelEditModal(); }
    else if (e.key === 'Enter' && e.target.matches('input[data-new-field]')) {
      e.preventDefault();
      create();
    }
  };
  root.addEventListener('keydown', onKey);

  activeModal = { el: root, channelId: null, onClick, onKey, cleanup: offHarnessList };
}
