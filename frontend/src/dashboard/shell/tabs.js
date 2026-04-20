import { state } from '../state.js';
import { onFilesTabActivated } from '../files/mode.js';

export function switchTab(tab) {
  state.currentTab = tab;
  // Only show panels that belong to the main viewer (not the detached chat).
  document.querySelectorAll('.tab-panel').forEach(p => {
    if (p.dataset.detached === 'true') return;
    p.classList.toggle('active', p.id === 'tab-' + tab);
  });
  if (tab === 'files') onFilesTabActivated();
  window.onTabSwitched?.(tab);
}

// Chat-input auto-resize — shell-level ergonomics, lives with tab navigation.
document.getElementById('chat-input')?.addEventListener('input', function () {
  this.style.height = 'auto';
  this.style.height = Math.min(this.scrollHeight, 120) + 'px';
});
