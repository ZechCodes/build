// Shared channel-actions popover — Restart / Stop / Rename. Opened
// from the sidebar channel row (primary surface: the channel is
// exactly where users look for "edit this channel"). Body-level
// portal, same positioning pattern as the model picker / complication
// menu.

import { bus } from '../core/bus.js';
import { uiStore } from '../domain/ui-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { showToast } from '../util/toast.js';

let activeMenu = null;    // { el, channelId, onDocClick }

export function openChannelMenu(channelId, anchorEl) {
  if (!channelId || !anchorEl) return;
  // Toggle off if the same anchor is clicked while open.
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
    <button type="button" data-ch-action="rename">
      <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
        <path d="M11.5 3.5l1 1M2 14l4-1 7-7-3-3-7 7z" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      Rename channel
    </button>
  `;
  document.body.appendChild(menu);

  // Position: prefer bottom-left of the anchor, flip up if no room.
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
    } else if (action === 'rename') {
      _beginRename(channelId);
    }
  };
  menu.addEventListener('click', onClick);

  // Outside click — close. Register on next microtask so the click
  // that opened the menu doesn't immediately close it.
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

// Rename flow: activate the channel, then replace the overlay title
// with an inline input. Works regardless of whether the overlay was
// already open — activating the channel is a sensible side effect.
function _beginRename(channelId) {
  uiStore.setActiveChannel(channelId);
  // Defer one frame so the title element is up-to-date.
  requestAnimationFrame(() => {
    const titleEl = document.querySelector('#v2-co-header .v2-co-title');
    if (!titleEl) {
      // Overlay not mounted — fall back to a prompt().
      const ch = channelsStore.get(channelId);
      const next = window.prompt('Rename channel', ch?.name || '');
      if (next && next.trim()) {
        bus.emit('intent.update_channel', { channelId, patch: { name: next.trim() } });
        showToast('Channel renamed');
      }
      return;
    }
    const ch = channelsStore.get(channelId);
    const current = ch?.name || '';
    const prevTitleHtml = titleEl.innerHTML;

    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'v2-co-rename-input';
    input.value = current;
    input.setAttribute('aria-label', 'Rename channel');
    titleEl.innerHTML = '';
    titleEl.appendChild(input);
    input.focus();
    input.select();

    let done = false;
    const restore = () => {
      if (done) return;
      done = true;
      input.removeEventListener('blur', onBlur);
      titleEl.innerHTML = prevTitleHtml;
      const nameEl = document.getElementById('v2-co-channel-name');
      if (nameEl) {
        const refreshed = channelsStore.get(channelId);
        nameEl.textContent = refreshed?.name || (channelId ? channelId.slice(0, 8) : 'Select a channel');
      }
    };
    const commit = () => {
      if (done) return;
      const next = input.value.trim();
      if (next && next !== current) {
        bus.emit('intent.update_channel', { channelId, patch: { name: next } });
        showToast('Channel renamed');
      }
      restore();
    };
    const onBlur = () => commit();
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); commit(); }
      else if (e.key === 'Escape') { e.preventDefault(); restore(); }
    });
    input.addEventListener('blur', onBlur);
  });
}
