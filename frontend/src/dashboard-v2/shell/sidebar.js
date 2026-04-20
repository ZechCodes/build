// Sidebar section collapse/expand with localStorage persistence.
// Pure DOM wiring — section state is transient UI, not domain state.

const KEY = (section) => `v2.sidebar.${section}.collapsed`;

export class SidebarSectionsView {
  constructor() {
    this._onClick = this._onClick.bind(this);
    this._bound = false;
  }

  activate() {
    if (this._bound) return;
    this._bound = true;
    document.addEventListener('click', this._onClick);
    // Restore persisted state.
    document.querySelectorAll('.v2-sidebar-section').forEach(sec => {
      const name = sec.getAttribute('data-section');
      if (!name) return;
      const persisted = localStorage.getItem(KEY(name));
      if (persisted === '1') sec.classList.add('collapsed');
      else if (persisted === '0') sec.classList.remove('collapsed');
    });
  }

  deactivate() {
    if (!this._bound) return;
    this._bound = false;
    document.removeEventListener('click', this._onClick);
  }

  _onClick(e) {
    const header = e.target.closest('[data-sidebar-toggle]');
    if (!header) return;
    const section = header.closest('.v2-sidebar-section');
    if (!section) return;
    section.classList.toggle('collapsed');
    const name = section.getAttribute('data-section');
    if (name) {
      localStorage.setItem(KEY(name), section.classList.contains('collapsed') ? '1' : '0');
    }
  }
}
