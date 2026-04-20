import { state } from '../state.js';
import { getE2EE, getActiveE2EE } from '../e2ee/bridge.js';
import { loadChannelState, saveChannelState } from '../channels/state-store.js';
import {
  filesPathText,
  filesPathBar,
  filesPathChevron,
  fileReloadBtn,
  fileTreePanel,
  fileContentBody,
  fileFloatToggle,
  fileWrapToggle,
} from './refs.js';
import { filesLoadRoot, renderFileTree, selectFile } from './tree.js';
import { renderFileContent, renderPreview } from './content.js';

export function updateFilesModifiedCount() {
  const el = document.getElementById('files-mode-modified-count');
  if (!el) return;
  const repos = state.filesChangesData.get(state.filesChannelId) || [];
  let total = 0;
  for (const repo of repos) total += (repo.entries || []).length;
  el.textContent = String(total);
  el.classList.toggle('hidden', total === 0);
}

export function setFilesMode(mode) {
  if (mode !== 'files' && mode !== 'changes') return;
  if (mode === state.filesTreeTab) return;
  state.filesTreeTab = mode;
  // Sync both the new mode-switch and the legacy tree-tab buttons.
  document.querySelectorAll('.files-mode-btn').forEach(b => b.classList.toggle('active', b.dataset.filesMode === mode));
  document.querySelectorAll('.tree-tab').forEach(b => b.classList.toggle('active', b.dataset.treeTab === mode));
  const _ftConn = getE2EE(state.filesChannelId);
  if (mode === 'changes' && state.filesChannelId && _ftConn && _ftConn.connected) {
    _ftConn.filesChanges(state.filesChannelId);
  }
  renderFileTree();
}

document.querySelectorAll('.tree-tab').forEach(btn => {
  btn.addEventListener('click', () => setFilesMode(btn.dataset.treeTab));
});
document.querySelectorAll('.files-mode-btn').forEach(btn => {
  btn.addEventListener('click', () => setFilesMode(btn.dataset.filesMode));
});

export function updateFloatingToggle() {
  if (!state.filesCurrentPath) {
    fileFloatToggle.style.display = 'none';
    return;
  }
  const tabs = [];
  if (state.filesCurrentIsMarkdown || state.filesCurrentIsSvg || state.filesCurrentIsHtml) tabs.push({ id: 'rendered', label: 'Preview' });
  if (state.filesCurrentHasDiff) tabs.push({ id: 'diff', label: 'Diff' });
  tabs.push({ id: 'source', label: 'Source' });

  if (tabs.length <= 1) {
    fileFloatToggle.style.display = 'none';
    return;
  }
  fileFloatToggle.style.display = 'flex';
  fileFloatToggle.innerHTML = tabs.map(t =>
    `<button data-view="${t.id}" class="${state.filesCurrentView === t.id ? 'active' : ''}">${t.label}</button>`
  ).join('');
}

// ---- Floating toggle ----
fileFloatToggle.addEventListener('click', (ev) => {
  const btn = ev.target.closest('button[data-view]');
  if (!btn || !state.filesCurrentPath) return;
  const view = btn.dataset.view;
  if (view === state.filesCurrentView) return;

  state.filesCurrentView = view;
  const _fvConn = getE2EE(state.filesChannelId);
  if (view === 'diff') {
    fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
    _fvConn?.fileDiff(state.filesChannelId, state.filesCurrentPath, false);
  } else if (view === 'rendered') {
    if (state.filesLastContent) {
      renderPreview(state.filesLastContent.content, state.filesLastContent.truncated, state.filesLastContent.size);
    } else {
      fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
      _fvConn?.fileRead(state.filesChannelId, state.filesCurrentPath);
    }
  } else {
    if (state.filesLastContent) {
      renderFileContent(state.filesLastContent.content, state.filesLastContent.path, state.filesLastContent.size, state.filesLastContent.truncated);
    } else {
      fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
      _fvConn?.fileRead(state.filesChannelId, state.filesCurrentPath);
    }
  }
  updateFloatingToggle();
  saveChannelState(state.filesChannelId, { filesView: state.filesCurrentView });
});

// ---- Line-wrap toggle ----
let filesLineWrap = localStorage.getItem('filesLineWrap') === '1';
if (filesLineWrap) {
  fileContentBody.classList.add('line-wrap');
  fileWrapToggle.classList.add('active');
}
fileWrapToggle.addEventListener('click', () => {
  filesLineWrap = !filesLineWrap;
  fileContentBody.classList.toggle('line-wrap', filesLineWrap);
  fileWrapToggle.classList.toggle('active', filesLineWrap);
  localStorage.setItem('filesLineWrap', filesLineWrap ? '1' : '0');
});

// ---- Mobile file tree dropdown toggle ----
filesPathBar.addEventListener('click', (ev) => {
  // Only act on mobile (chevron visible).
  if (getComputedStyle(filesPathChevron).display === 'none') return;
  const isOpen = fileTreePanel.classList.toggle('mobile-open');
  filesPathChevron.classList.toggle('open', isOpen);
});

// ---- Reload button ----
fileReloadBtn.addEventListener('click', (ev) => {
  ev.stopPropagation();
  if (!state.filesCurrentPath || !state.filesChannelId) return;
  state.filesLastContent = null;
  const conn = getE2EE(state.filesChannelId);
  if (!conn || !conn.connected) return;
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
  if (state.filesCurrentView === 'diff') {
    conn.fileDiff(state.filesChannelId, state.filesCurrentPath, false);
  } else {
    conn.fileRead(state.filesChannelId, state.filesCurrentPath);
  }
});

// ---- Load files on tab switch ----

export function onFilesTabActivated() {
  if (!state.filesChannelId || state.filesChannelId !== state.chatCurrentChannel) {
    // Reset tree for new channel.
    state.fileTreeData.delete(state.filesChannelId);
    state.filesChangesData.delete(state.filesChannelId);
    state.filesCurrentPath = null;
    state.filesCurrentView = 'source';
    state.filesLastContent = null;
    state.filesCurrentHasDiff = false;
    state.filesCurrentIsMarkdown = false;
    state.filesCurrentIsSvg = false;
    state.filesCurrentIsHtml = false;
    filesPathText.textContent = 'No file selected';
    filesPathText.classList.add('empty');
    fileReloadBtn.classList.add('hc-hidden');
    fileFloatToggle.style.display = 'none';
    fileContentBody.innerHTML = '<div class="empty-state"><p>Select a file to view contents</p></div>';

    // Check for saved state to restore after tree loads.
    const saved = loadChannelState(state.chatCurrentChannel);
    if (saved.filesPath) {
      state.filesPendingRestore = saved.filesPath;
      state.filesPendingView = saved.filesView || 'source';
    } else {
      state.filesPendingRestore = null;
    }

    filesLoadRoot();
    const _ftaConn = getActiveE2EE();
    if (state.filesTreeTab === 'changes' && _ftaConn && _ftaConn.connected) {
      _ftaConn.filesChanges(state.chatCurrentChannel);
    }
  }
}

// Post-switch hook — dashboard's switchTab calls this so Files can react.
window.onTabSwitched = function(tab) {
  if (tab === 'files') onFilesTabActivated();
  if (state.chatCurrentChannel) saveChannelState(state.chatCurrentChannel, { activeTab: tab });
};

// ===== File Path Link Navigation =====
export function navigateToFile(filePath) {
  // If the files tab is already showing this channel, just select the file.
  // Otherwise, set pending restore and switch tabs.
  state.filesPendingRestore = filePath;
  state.filesPendingView = 'source';
  window.switchTab?.('files');
  location.hash = 'files';
  // Also select immediately — works when tree is already loaded.
  if (state.filesChannelId) {
    selectFile(filePath, null, 'source');
  }
}

// Delegated click handler for file-path-link elements in messages.
document.addEventListener('click', function(e) {
  var link = e.target.closest('.file-path-link');
  if (!link) return;
  e.preventDefault();
  var path = link.dataset.filePath;
  if (path) navigateToFile(path);
});
