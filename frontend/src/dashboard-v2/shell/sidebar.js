// Sidebar wiring: section collapse/expand with localStorage persistence,
// plus the mobile drawer open/close (hamburger, close X, backdrop,
// auto-close on channel select).

const KEY = (section) => `v2.sidebar.${section}.collapsed`;

function isMobile() {
  return window.matchMedia('(max-width: 768px)').matches;
}

function app() { return document.querySelector('.v2-app'); }

function setDrawerOpen(open) {
  const el = app();
  if (!el) return;
  el.classList.toggle('sidebar-open', !!open);
}

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
    // Mobile drawer open/close.
    if (e.target.closest('#v2-path-bar-menu')) { setDrawerOpen(true); return; }
    if (e.target.closest('#v2-sidebar-close') || e.target.closest('#v2-sidebar-backdrop')) {
      setDrawerOpen(false); return;
    }
    // Mobile file-tree drawer toggle.
    if (e.target.closest('#v2-path-bar-tree')) {
      document.getElementById('v2-viewer-main')?.classList.toggle('mobile-tree-open');
      return;
    }
    // Tapping the tree panel's backdrop area on mobile closes it — rely
    // on the tap happening on .v2-files-content-panel while the tree is
    // shown above it.
    if (isMobile()
        && document.getElementById('v2-viewer-main')?.classList.contains('mobile-tree-open')
        && e.target.closest('.v2-files-content-panel')) {
      document.getElementById('v2-viewer-main')?.classList.remove('mobile-tree-open');
      return;
    }
    // Auto-collapse the tree drawer on file select.
    if (isMobile() && e.target.closest('[data-file-path]')) {
      document.getElementById('v2-viewer-main')?.classList.remove('mobile-tree-open');
    }
    // Channel selection while drawer is open on mobile → auto-close.
    // (Don't close on section-header clicks.)
    if (isMobile()
        && app()?.classList.contains('sidebar-open')
        && e.target.closest('.v2-channel-sidebar-item')) {
      setDrawerOpen(false);
      // Fall through — the channel-panel view handles activation itself.
    }

    // Section collapse/expand.
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
