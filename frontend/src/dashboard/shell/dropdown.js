import { state } from '../state.js';

export function closeAllDropdowns() {
  document.querySelectorAll('.top-dropdown-menu.open').forEach(m => m.classList.remove('open'));
  document.querySelectorAll('.top-dropdown-trigger.open').forEach(t => t.classList.remove('open'));
  state._openDropdown = null;
}

export function positionDropdownMenu(trigger, menu) {
  const tr = trigger.getBoundingClientRect();
  menu.style.top = (tr.bottom + 4) + 'px';
  menu.style.left = tr.left + 'px';
  requestAnimationFrame(() => {
    const mr = menu.getBoundingClientRect();
    if (mr.right > window.innerWidth - 8) {
      menu.style.left = Math.max(8, window.innerWidth - mr.width - 8) + 'px';
    }
  });
}

export function toggleDropdown(dropdownId) {
  const trigger = document.getElementById(dropdownId + '-trigger');
  const menu = document.getElementById(dropdownId + '-menu');
  if (!trigger || !menu) return;
  const isOpen = menu.classList.contains('open');
  closeAllDropdowns();
  if (!isOpen) {
    trigger.classList.add('open');
    menu.classList.add('open');
    positionDropdownMenu(trigger, menu);
    state._openDropdown = dropdownId;
  }
}

// Close dropdowns on any click outside
document.addEventListener('click', (e) => {
  if (!state._openDropdown) return;
  const dropdown = document.getElementById(state._openDropdown);
  if (dropdown && !dropdown.contains(e.target)) {
    closeAllDropdowns();
  }
}, true);
