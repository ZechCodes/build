// Single-active-dropdown manager. Future device menu / account menu hang
// off [data-dropdown-trigger] / [data-dropdown-id]. For Wave 3 this just
// installs the close-on-outside-click and tracks open state in uiStore.

import { uiStore } from '../domain/ui-store.js';

export class DropdownView {
  constructor() {
    this._onDocClick = this._onDocClick.bind(this);
    this._bound = false;
  }

  activate() {
    if (this._bound) return;
    this._bound = true;
    document.addEventListener('click', this._onDocClick);
  }

  deactivate() {
    if (!this._bound) return;
    this._bound = false;
    document.removeEventListener('click', this._onDocClick);
  }

  _onDocClick(e) {
    const trigger = e.target.closest('[data-dropdown-trigger]');
    if (trigger) {
      const id = trigger.getAttribute('data-dropdown-trigger');
      const current = uiStore.getOpenDropdown();
      uiStore.setOpenDropdown(current === id ? null : id);
      return;
    }
    // Click outside any dropdown element closes it.
    if (uiStore.getOpenDropdown() && !e.target.closest('[data-dropdown]')) {
      uiStore.setOpenDropdown(null);
    }
  }
}
