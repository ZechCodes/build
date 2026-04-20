export function setSidebarSectionOpen(name, open, persist = true) {
  const section = document.querySelector(`.sidebar-section[data-section="${name}"]`);
  if (!section) return;
  const body = section.querySelector('.sidebar-section-body');
  const chevron = section.querySelector('.sidebar-section-chevron');
  body?.classList.toggle('collapsed', !open);
  chevron?.classList.toggle('collapsed', !open);
  section.classList.toggle('collapsed', !open);
  if (persist) {
    try { localStorage.setItem(`sidebar.section.${name}.collapsed`, open ? '0' : '1'); } catch (_) {}
  }
}

export function toggleSidebarSection(name) {
  const section = document.querySelector(`.sidebar-section[data-section="${name}"]`);
  if (!section) return;
  const body = section.querySelector('.sidebar-section-body');
  if (!body) return;
  setSidebarSectionOpen(name, body.classList.contains('collapsed'));
}

export function expandSidebarSection(name) {
  // Auto-expansion (triggered by arriving activity) does NOT persist, so a
  // page reload still respects the user's default-collapsed preference.
  setSidebarSectionOpen(name, true, false);
}

// Wire up headers and initial collapsed state.
document.querySelectorAll('.sidebar-section-header[data-sidebar-toggle]').forEach(h => {
  h.addEventListener('click', () => toggleSidebarSection(h.dataset.sidebarToggle));
});
for (const section of document.querySelectorAll('.sidebar-section')) {
  const body = section.querySelector('.sidebar-section-body');
  section.classList.toggle('collapsed', !!body?.classList.contains('collapsed'));
}
try {
  for (const name of ['tasks', 'activity']) {
    const saved = localStorage.getItem(`sidebar.section.${name}.collapsed`);
    if (saved === '1') setSidebarSectionOpen(name, false);
    else if (saved === '0') setSidebarSectionOpen(name, true);
  }
} catch (_) {}
