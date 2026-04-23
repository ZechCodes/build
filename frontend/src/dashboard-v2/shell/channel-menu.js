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
  const name = ch?.name || '';
  const cwd  = ch?.working_directory || '';

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
          <span class="v2-modal-hint">Changing this restarts the agent with the new path.</span>
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

  // Focus the name input so Enter / typing starts working.
  setTimeout(() => modal.querySelector('[data-edit-field="name"]').focus(), 0);

  const commit = () => {
    const nextName = modal.querySelector('[data-edit-field="name"]').value.trim();
    const nextCwd  = modal.querySelector('[data-edit-field="cwd"]').value.trim();
    let any = false;
    if (nextName && nextName !== name) {
      bus.emit('intent.rename_channel', { channelId, name: nextName });
      any = true;
    }
    if (nextCwd !== cwd) {
      bus.emit('intent.update_channel', { channelId, patch: { working_directory: nextCwd } });
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

  activeModal = { el: root, channelId, onClick, onKey };
}

export function closeChannelEditModal() {
  if (!activeModal) return;
  const { el, onClick, onKey } = activeModal;
  el.removeEventListener('click', onClick);
  el.removeEventListener('keydown', onKey);
  el.remove();
  activeModal = null;
}

// ── New-channel modal ────────────────────────────────────────────────

export function openChannelNewModal(deviceId) {
  if (!deviceId) return;
  closeChannelEditModal();

  const harnesses = presenceStore.getHarnesses(deviceId) || [];
  const defaultHarness = harnesses[0] || null;

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
        ${_harnessSelect(harnesses, defaultHarness)}
        <label class="v2-modal-field" data-new-slot="model">
          <span class="v2-modal-label">Model</span>
          <select class="v2-modal-select" data-new-field="model"></select>
        </label>
        <label class="v2-modal-field" data-new-slot="effort">
          <span class="v2-modal-label">Effort</span>
          <select class="v2-modal-select" data-new-field="effort"></select>
        </label>
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

  const refreshForHarness = () => {
    const hId = harnessSel ? harnessSel.value : defaultHarness?.id;
    const h = harnesses.find(x => x.id === hId) || defaultHarness;
    const models = h?.models || [];
    const efforts = h?.effort_levels || ['low', 'medium', 'high'];
    modelSel.innerHTML = models.length
      ? models.map(m => `<option value="${escapeHtml(m.id)}">${escapeHtml(m.name || m.id)}</option>`).join('')
      : '<option value="">(harness has no models)</option>';
    effortSel.innerHTML = efforts.map(e => `<option value="${escapeHtml(e)}">${escapeHtml(e)}</option>`).join('');
    // Hide model / effort if there's only one option — nothing to choose.
    modal.querySelector('[data-new-slot="model"]').hidden  = models.length <= 1;
    modal.querySelector('[data-new-slot="effort"]').hidden = efforts.length <= 1;
  };
  if (harnessSel) harnessSel.addEventListener('change', refreshForHarness);
  refreshForHarness();

  setTimeout(() => modal.querySelector('[data-new-field="name"]').focus(), 0);

  const create = () => {
    const nameV   = modal.querySelector('[data-new-field="name"]').value.trim();
    const cwdV    = modal.querySelector('[data-new-field="cwd"]').value.trim();
    const harV    = harnessSel ? harnessSel.value : defaultHarness?.id || '';
    const modelV  = modelSel.value;
    const effortV = effortSel.value;
    if (!nameV) { showToast('Name is required', { kind: 'error' }); return; }
    bus.emit('intent.create_channel', {
      deviceId,
      name: nameV,
      harness: harV || undefined,
      model:   modelV || undefined,
      effort:  effortV || undefined,
      working_directory: cwdV || undefined,
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
    if (action === 'cancel') return closeChannelEditModal();
    if (action === 'create') return create();
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

  activeModal = { el: root, channelId: null, onClick, onKey };
}

function _harnessSelect(harnesses, defaultHarness) {
  if (harnesses.length <= 1) return '';
  const options = harnesses.map(h =>
    `<option value="${escapeHtml(h.id)}"${h.id === defaultHarness?.id ? ' selected' : ''}>${escapeHtml(h.name || h.id)}</option>`,
  ).join('');
  return `
    <label class="v2-modal-field" data-new-slot="harness">
      <span class="v2-modal-label">Harness</span>
      <select class="v2-modal-select" data-new-field="harness">${options}</select>
    </label>
  `;
}
