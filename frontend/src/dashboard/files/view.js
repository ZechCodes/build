import { state } from '../state.js';
import { escapeHtml, escHtml, formatBytes, formatFileSize } from '../util/html.js';
import { highlightLine } from './syntax.js';

// External deps kept as window globals during transition:
//   window.getActiveE2EE, window.getE2EE, window.switchTab, window.saveChannelState,
//   window.renderMessages (for file-embed rendering), window.loadChannelState

// ===== FILES VIEW =====

const fileTree = document.getElementById('file-tree');
const fileTreeInner = document.getElementById('file-tree-inner');
const fileTreeEmpty = document.getElementById('file-tree-empty');
const filesPathText = document.getElementById('files-path-text');
const filesPathBar = document.getElementById('files-path-bar');
const filesPathChevron = document.getElementById('files-path-chevron');
const fileReloadBtn = document.getElementById('file-reload-btn');
const fileTreePanel = document.getElementById('file-tree-panel');
export const fileContentBody = document.getElementById('file-content-body');
const fileFloatToggle = document.getElementById('file-float-toggle');

// Per-channel file state.

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
  const _ftConn = window.getE2EE?.(state.filesChannelId);
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

export function filesLoadRoot() {
  const _flConn = window.getActiveE2EE?.();
  if (!state.chatCurrentChannel || !_flConn || !_flConn.connected) return;
  state.filesChannelId = state.chatCurrentChannel;
  _flConn.filesList(state.chatCurrentChannel, '');
}

// ---- Tree Rendering ----

export function renderFileTree() {
  fileTreeInner.querySelectorAll('.tree-item, .tree-children, .changes-empty, .changes-repo-group').forEach(n => n.remove());

  // If the tab is open with a channel selected but state.filesChannelId wasn't
  // set (E2EE timing race), try once to kick off the load so the view
  // self-heals on re-render instead of showing the "Select a channel" stub.
  if (!state.filesChannelId && state.chatCurrentChannel && typeof onFilesTabActivated === 'function' && !state._filesSelfHealed) {
    state._filesSelfHealed = true;
    onFilesTabActivated();
  }

  if (state.filesTreeTab === 'changes') {
    // Changes view has its own "No modified files" / "Select a channel" empty
    // states inside renderChangesView; hide the tree-specific placeholder.
    fileTreeEmpty.style.display = 'none';
    renderChangesView(fileTreeInner, state.filesChannelId ? state.filesChangesData.get(state.filesChannelId) : null);
    return;
  }

  // Files tree mode — requires a channel + root listing in state.fileTreeData.
  if (!state.filesChannelId) { fileTreeEmpty.style.display = ''; return; }
  const data = state.fileTreeData.get(state.filesChannelId);
  if (!data || !data.has('')) {
    fileTreeEmpty.style.display = '';
    return;
  }
  fileTreeEmpty.style.display = 'none';
  const rootEntries = data.get('');
  if (rootEntries) renderTreeLevel(fileTreeInner, rootEntries.entries, 0, data);
}

export function renderTreeLevel(container, entries, depth, data) {
  for (const entry of entries) {
    const item = document.createElement('div');
    item.className = 'tree-item';
    if (depth > 0) item.setAttribute('data-depth', Math.min(depth, 5));
    item.dataset.path = entry.path;
    item.dataset.type = entry.type;

    // Git status classes.
    if (entry.is_gitignored) item.classList.add('gitignored');
    if (entry.git_status === '?' || entry.staged_status === '?') item.classList.add('untracked');
    if (entry.path === state.filesCurrentPath) item.classList.add('active');

    // Arrow for directories.
    const arrow = document.createElement('span');
    arrow.className = 'tree-arrow';
    if (entry.type === 'dir') {
      const expanded = data.has(entry.path);
      arrow.innerHTML = '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
      if (expanded) arrow.classList.add('open');
    } else {
      arrow.classList.add('hidden');
    }
    item.appendChild(arrow);

    // Icon.
    const icon = document.createElement('span');
    icon.className = 'tree-icon' + (entry.type === 'dir' ? ' folder' : '');
    icon.innerHTML = entry.type === 'dir'
      ? '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M1.5 3.5v7a1 1 0 001 1h9a1 1 0 001-1v-5a1 1 0 00-1-1H7L5.5 2.5h-3a1 1 0 00-1 1z" stroke="currentColor" stroke-width="1.2"/></svg>'
      : '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3.5 1.5h4l3 3v7a1 1 0 01-1 1h-6a1 1 0 01-1-1v-9a1 1 0 011-1z" stroke="currentColor" stroke-width="1.2"/><path d="M7.5 1.5v3h3" stroke="currentColor" stroke-width="1.2"/></svg>';
    item.appendChild(icon);

    // Label.
    const label = document.createElement('span');
    label.className = 'tree-label';
    label.textContent = entry.name;
    label.title = entry.path || entry.name;
    item.appendChild(label);

    // Git badge + stats — wrapped in sticky container.
    const hasGitBadge = entry.git_status || entry.staged_status;
    const hasStats = entry.insertions > 0 || entry.deletions > 0;
    if (hasGitBadge || hasStats) {
      const gitInfo = document.createElement('span');
      gitInfo.className = 'tree-git-info';
      if (hasGitBadge) {
        const st = entry.staged_status || entry.git_status;
        if (st === '?' || st === 'A') {
          const badge = document.createElement('span');
          badge.className = 'tree-badge added';
          badge.textContent = st === '?' ? 'U' : 'A';
          gitInfo.appendChild(badge);
        } else if (st === 'M') {
          const badge = document.createElement('span');
          badge.className = 'tree-badge modified';
          badge.textContent = 'M';
          gitInfo.appendChild(badge);
        } else if (st === 'D') {
          const badge = document.createElement('span');
          badge.className = 'tree-badge modified';
          badge.textContent = 'D';
          gitInfo.appendChild(badge);
        }
      }
      if (hasStats) {
        const stats = document.createElement('span');
        stats.className = 'tree-stats';
        if (entry.insertions > 0) {
          const add = document.createElement('span');
          add.className = 'stat-add';
          add.textContent = '+' + entry.insertions;
          stats.appendChild(add);
        }
        if (entry.deletions > 0) {
          const del = document.createElement('span');
          del.className = 'stat-del';
          del.textContent = '-' + entry.deletions;
          stats.appendChild(del);
        }
        gitInfo.appendChild(stats);
      }
      item.appendChild(gitInfo);
    }

    container.appendChild(item);

    // Click handler.
    item.addEventListener('click', (ev) => {
      ev.stopPropagation();
      if (entry.type === 'dir') {
        toggleDirectory(entry.path, arrow, item);
      } else {
        selectFile(entry.path, entry);
      }
    });

    // Render children if expanded.
    if (entry.type === 'dir' && data.has(entry.path)) {
      const childContainer = document.createElement('div');
      childContainer.className = 'tree-children open';
      childContainer.dataset.parentPath = entry.path;
      renderTreeLevel(childContainer, data.get(entry.path).entries, depth + 1, data);
      container.appendChild(childContainer);
    }
  }
}

export function renderChangesView(container, repos) {
  // repos is an array of {path, remote, branch, entries[]} from the backend (or undefined if not yet loaded).
  if (!repos) {
    const loadingDiv = document.createElement('div');
    loadingDiv.className = 'empty-state pad-1-5 changes-empty';
    loadingDiv.innerHTML = '<div class="loading-spinner"></div><p class="text-xs">Loading changes…</p>';
    container.appendChild(loadingDiv);
    return;
  }
  if (repos.length === 0) {
    const emptyDiv = document.createElement('div');
    emptyDiv.className = 'empty-state pad-1-5 changes-empty';
    emptyDiv.innerHTML = '<p class="text-xs">No changes</p>';
    container.appendChild(emptyDiv);
    return;
  }

  // Sort repos by most recently modified file (descending).
  const sortedRepos = repos.slice().sort((a, b) => {
    const aMax = Math.max(0, ...a.entries.map(e => e.modified || 0));
    const bMax = Math.max(0, ...b.entries.map(e => e.modified || 0));
    return bMax - aMax;
  });

  for (const repo of sortedRepos) {
    const group = document.createElement('div');
    group.className = 'changes-repo-group';

    // Repo header.
    const header = document.createElement('div');
    header.className = 'changes-repo-header';

    const chevron = document.createElement('span');
    chevron.className = 'changes-repo-chevron';
    chevron.innerHTML = '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
    header.appendChild(chevron);

    const nameSpan = document.createElement('span');
    nameSpan.className = 'changes-repo-name';
    nameSpan.textContent = repo.path === '.' ? '(root)' : repo.path;
    header.appendChild(nameSpan);

    const meta = document.createElement('span');
    meta.className = 'changes-repo-meta';
    if (repo.remote && repo.branch) {
      meta.textContent = repo.remote + ' @ ' + repo.branch;
    } else {
      meta.textContent = repo.remote || repo.branch || '';
    }
    header.appendChild(meta);

    // Aggregate stats.
    let totalIns = 0, totalDels = 0;
    for (const e of repo.entries) { totalIns += e.insertions || 0; totalDels += e.deletions || 0; }
    const statsSpan = document.createElement('span');
    statsSpan.className = 'changes-repo-stats';
    let statsHTML = repo.entries.length + ' file' + (repo.entries.length !== 1 ? 's' : '');
    if (totalIns > 0) statsHTML += '&ensp;<span class="stat-add">+' + totalIns + '</span>';
    if (totalDels > 0) statsHTML += '&ensp;<span class="stat-del">\u2212' + totalDels + '</span>';
    statsSpan.innerHTML = statsHTML;
    header.appendChild(statsSpan);

    group.appendChild(header);

    // File list.
    const fileList = document.createElement('div');
    fileList.className = 'changes-repo-files';

    const sorted = repo.entries.slice().sort((a, b) => a.path.localeCompare(b.path));
    for (const entry of sorted) {
      const item = document.createElement('div');
      item.className = 'tree-item';
      if (entry.git_status === '?' || entry.staged_status === '?') item.classList.add('untracked');
      if (entry.path === state.filesCurrentPath) item.classList.add('active');
      item.dataset.path = entry.path;
      item.dataset.type = entry.type;

      const arrow = document.createElement('span');
      arrow.className = 'tree-arrow hidden';
      item.appendChild(arrow);

      const icon = document.createElement('span');
      icon.className = 'tree-icon';
      icon.innerHTML = '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3.5 1.5h4l3 3v7a1 1 0 01-1 1h-6a1 1 0 01-1-1v-9a1 1 0 011-1z" stroke="currentColor" stroke-width="1.2"/><path d="M7.5 1.5v3h3" stroke="currentColor" stroke-width="1.2"/></svg>';
      item.appendChild(icon);

      // Show path relative to repo (strip repo prefix).
      const label = document.createElement('span');
      label.className = 'tree-label';
      let displayPath = entry.path;
      if (repo.path !== '.' && displayPath.startsWith(repo.path + '/')) {
        displayPath = displayPath.slice(repo.path.length + 1);
      }
      label.textContent = displayPath;
      label.title = entry.path;
      item.appendChild(label);

      const hasGitBadge = entry.git_status || entry.staged_status;
      const hasStats = entry.insertions > 0 || entry.deletions > 0;
      if (hasGitBadge || hasStats) {
        const gitInfo = document.createElement('span');
        gitInfo.className = 'tree-git-info';
        if (hasGitBadge) {
          const st = entry.staged_status || entry.git_status;
          const badge = document.createElement('span');
          if (st === '?' || st === 'A') {
            badge.className = 'tree-badge added';
            badge.textContent = st === '?' ? 'U' : 'A';
          } else if (st === 'M') {
            badge.className = 'tree-badge modified';
            badge.textContent = 'M';
          } else if (st === 'D') {
            badge.className = 'tree-badge modified';
            badge.textContent = 'D';
          }
          if (badge.textContent) gitInfo.appendChild(badge);
        }
        if (hasStats) {
          const stats = document.createElement('span');
          stats.className = 'tree-stats';
          if (entry.insertions > 0) {
            const add = document.createElement('span');
            add.className = 'stat-add';
            add.textContent = '+' + entry.insertions;
            stats.appendChild(add);
          }
          if (entry.deletions > 0) {
            const del = document.createElement('span');
            del.className = 'stat-del';
            del.textContent = '-' + entry.deletions;
            stats.appendChild(del);
          }
          gitInfo.appendChild(stats);
        }
        item.appendChild(gitInfo);
      }

      item.addEventListener('click', () => selectFile(entry.path, entry, 'diff'));
      fileList.appendChild(item);
    }

    group.appendChild(fileList);

    // Toggle collapse on header click.
    header.addEventListener('click', () => {
      group.classList.toggle('collapsed');
    });

    container.appendChild(group);
  }
}

export function toggleDirectory(path, arrowEl, itemEl) {
  const data = state.fileTreeData.get(state.filesChannelId);
  if (!data) return;

  if (data.has(path)) {
    // Collapse: remove cached children.
    data.delete(path);
    renderFileTree();
  } else {
    // Expand: request listing.
    window.getE2EE?.(state.filesChannelId)?.filesList(state.filesChannelId, path);
  }
}

export function selectFile(path, entry, initialView) {
  const ext = (path.split('.').pop() || '').toLowerCase();
  state.filesCurrentIsMarkdown = (ext === 'md' || ext === 'markdown');
  state.filesCurrentIsSvg = (ext === 'svg');
  state.filesCurrentIsHtml = (ext === 'html' || ext === 'htm');
  state.filesCurrentHasDiff = !!(entry && (entry.git_status || entry.staged_status || entry.insertions || entry.deletions));
  state.filesCurrentPath = path;
  state.filesLastContent = null;

  // Determine initial view.
  if (initialView === 'diff' && state.filesCurrentHasDiff) {
    state.filesCurrentView = 'diff';
  } else if (state.filesCurrentIsMarkdown || state.filesCurrentIsSvg || state.filesCurrentIsHtml) {
    state.filesCurrentView = 'rendered';
  } else {
    state.filesCurrentView = 'source';
  }

  // Update active state in tree.
  fileTree.querySelectorAll('.tree-item.active').forEach(el => el.classList.remove('active'));
  const active = fileTree.querySelector(`.tree-item[data-path="${CSS.escape(path)}"]`);
  if (active) active.classList.add('active');

  // Update path bar.
  filesPathText.textContent = path;
  filesPathText.classList.remove('empty');
  fileReloadBtn.classList.remove('hc-hidden');

  // Close mobile dropdown.
  fileTreePanel.classList.remove('mobile-open');
  filesPathChevron.classList.remove('open');

  updateFloatingToggle();

  // Save to localStorage.
  window.saveChannelState?.(state.filesChannelId, { filesPath: path, filesView: state.filesCurrentView });

  // Load file content.
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
  const _fileConn = window.getE2EE?.(state.filesChannelId);
  if (state.filesCurrentView === 'diff') {
    _fileConn?.fileDiff(state.filesChannelId, path, false);
  } else {
    _fileConn?.fileRead(state.filesChannelId, path);
  }
}

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
  const _fvConn = window.getE2EE?.(state.filesChannelId);
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
  window.saveChannelState?.(state.filesChannelId, { filesView: state.filesCurrentView });
});

// ---- Line-wrap toggle ----
const fileWrapToggle = document.getElementById('file-wrap-toggle');
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
  ev.stopPropagation(); // Don't trigger path bar mobile toggle.
  if (!state.filesCurrentPath || !state.filesChannelId) return;
  state.filesLastContent = null;
  const conn = window.getE2EE?.(state.filesChannelId);
  if (!conn || !conn.connected) return;
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
  if (state.filesCurrentView === 'diff') {
    conn.fileDiff(state.filesChannelId, state.filesCurrentPath, false);
  } else {
    conn.fileRead(state.filesChannelId, state.filesCurrentPath);
  }
});

// ---- Live file-change refresh (fed by AGENT_FILE_CHANGES) ----
let _fileChangesRefreshTimer = null;
export function onAgentFileChanges(channelId, paths) {
  console.debug('[files] agent.file_changes', { channelId, paths, filesChannelId: state.filesChannelId, chatCurrentChannel: state.chatCurrentChannel });
  // Only react if this channel is the one currently displayed in the files view.
  if (state.filesChannelId && state.filesChannelId !== channelId) return;
  // Adopt the channel so the files_changes_result handler doesn't drop the
  // response if state.filesChannelId was still null at this point.
  if (!state.filesChannelId && state.chatCurrentChannel === channelId) {
    state.filesChannelId = channelId;
  }
  // Debounce bursts: coalesce multiple events within 150ms.
  clearTimeout(_fileChangesRefreshTimer);
  _fileChangesRefreshTimer = setTimeout(() => {
    const conn = window.getE2EE?.(state.filesChannelId || channelId);
    if (!conn || !conn.connected) return;
    // Always refresh the changes list so the Modified count stays live.
    conn.filesChanges(state.filesChannelId || channelId);
    // If the tree is showing, invalidate cached path entries so stale items are re-fetched.
    if (state.filesTreeTab === 'files') {
      const data = state.fileTreeData.get(state.filesChannelId);
      if (data) {
        for (const p of paths) {
          const parent = p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '';
          data.delete(parent);
        }
      }
      conn.filesList(state.filesChannelId || channelId, '');
    }
    // If the currently open file was touched, re-fetch its content/diff.
    if (state.filesCurrentPath) {
      const cur = state.filesCurrentPath.replace(/\\/g, '/');
      const touched = paths.some(p => {
        const pp = (p || '').replace(/\\/g, '/');
        if (!pp) return false;
        return pp === cur
          || cur.endsWith('/' + pp)
          || pp.endsWith('/' + cur)
          || pp.split('/').pop() === cur.split('/').pop();
      });
      if (touched) {
        if (state.filesCurrentView === 'diff') {
          conn.fileDiff(state.filesChannelId || channelId, state.filesCurrentPath, false);
        } else {
          conn.fileRead(state.filesChannelId || channelId, state.filesCurrentPath);
        }
      }
    }
  }, 150);
}

// ---- File Content Renderer ----

export function renderFileContent(content, path, size, truncated) {
  // Cache for toggle without re-fetch.
  state.filesLastContent = { content, path, size, truncated };
  fileContentBody.style.padding = '';

  // Preview mode for markdown/SVG/HTML.
  if (state.filesCurrentView === 'rendered' && (state.filesCurrentIsMarkdown || state.filesCurrentIsSvg || state.filesCurrentIsHtml)) {
    renderPreview(content, truncated, size);
    return;
  }

  const extMatch = path.match(/\.([^./]+)$/);
  const ext = extMatch ? extMatch[1].toLowerCase() : path.split('/').pop().toLowerCase();
  const lines = content.split('\n');
  // Remove trailing empty line from split.
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();

  const viewer = document.createElement('div');
  viewer.className = 'file-viewer';

  for (let i = 0; i < lines.length; i++) {
    const row = document.createElement('div');
    row.className = 'file-line';

    const numEl = document.createElement('span');
    numEl.className = 'fl-num';
    numEl.textContent = String(i + 1);

    const codeEl = document.createElement('span');
    codeEl.className = 'fl-content';
    codeEl.innerHTML = highlightLine(lines[i], ext);

    row.appendChild(numEl);
    row.appendChild(codeEl);
    viewer.appendChild(row);
  }

  fileContentBody.innerHTML = '';
  fileContentBody.appendChild(viewer);

  if (truncated) {
    const note = document.createElement('div');
    note.style.cssText = 'padding:.5rem .75rem;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border)';
    note.textContent = `File truncated (${formatBytes(size)} total)`;
    fileContentBody.appendChild(note);
  }
}

export function renderPreview(content, truncated, size) {
  if (state.filesCurrentIsSvg) {
    renderSvgPreview(content);
  } else if (state.filesCurrentIsHtml) {
    renderHtmlPreview(content, state.filesCurrentPath);
  } else {
    renderMarkdownFile(content, truncated, size);
  }
}

export function renderMarkdownFile(content, truncated, size) {
  const wrapper = document.createElement('div');
  wrapper.className = 'file-markdown-view msg-text';
  wrapper.innerHTML = renderMarkdown(content);
  fileContentBody.innerHTML = '';
  fileContentBody.appendChild(wrapper);
  if (truncated) {
    const note = document.createElement('div');
    note.style.cssText = 'padding:.5rem .75rem;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border)';
    note.textContent = `File truncated (${formatBytes(size)} total)`;
    fileContentBody.appendChild(note);
  }
}

export function renderSvgPreview(content) {
  const blob = new Blob([content], { type: 'image/svg+xml' });
  const url = URL.createObjectURL(blob);
  fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + url + '" alt="SVG preview"></div>';
}

// ---- HTML Preview with Asset Inlining ----

// Promise-based file read for fetching assets without conflicting with main file viewer.
export function readFileAsync(channelId, path) {
  return new Promise((resolve, reject) => {
    const conn = window.getE2EE?.(channelId);
    if (!conn || !conn.connected) return reject(new Error('not connected'));
    const chunks = {};
    function handler(evt) {
      const d = evt.detail;
      if (d.path !== path) return;
      if (d.error) { conn.removeEventListener('file_read_result', handler); return reject(new Error(d.error)); }
      // Handle chunked images.
      if (d.is_image && d.chunk_total && d.chunk_total > 1) {
        if (!chunks.arr) { chunks.arr = new Array(d.chunk_total); chunks.total = d.chunk_total; }
        chunks.arr[d.chunk_index] = d.content;
        if (chunks.arr.filter(Boolean).length < chunks.total) return;
        conn.removeEventListener('file_read_result', handler);
        return resolve({ ...d, content: chunks.arr.join('') });
      }
      conn.removeEventListener('file_read_result', handler);
      resolve(d);
    }
    conn.addEventListener('file_read_result', handler);
    conn.fileRead(channelId, path);
  });
}

const MIME_TYPES = {
  css: 'text/css', js: 'application/javascript', mjs: 'application/javascript',
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  webp: 'image/webp', ico: 'image/x-icon', svg: 'image/svg+xml',
  woff: 'font/woff', woff2: 'font/woff2', ttf: 'font/ttf', eot: 'application/vnd.ms-fontobject',
  json: 'application/json',
};

export function resolveAssetPath(htmlFilePath, assetHref) {
  if (!assetHref || assetHref.startsWith('data:') || assetHref.startsWith('http:') || assetHref.startsWith('https:') || assetHref.startsWith('//')) return null;
  // Strip query/hash.
  const clean = assetHref.split('?')[0].split('#')[0];
  // Resolve relative to HTML file's directory.
  const dir = htmlFilePath.substring(0, htmlFilePath.lastIndexOf('/') + 1);
  // Simple path resolution (handles ../ and ./).
  const parts = (dir + clean).split('/');
  const resolved = [];
  for (const p of parts) {
    if (p === '.' || p === '') continue;
    if (p === '..') { resolved.pop(); continue; }
    resolved.push(p);
  }
  return resolved.join('/');
}

// Console capture JS (raw code, no <script> tags — injected via DOM).
const CONSOLE_CAPTURE_JS = `(function(){
  function send(level, args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) {
      try { parts.push(typeof args[i] === 'string' ? args[i] : JSON.stringify(args[i], null, 2)); }
      catch(e) { parts.push(String(args[i])); }
    }
    parent.postMessage({ type: '__build_console', entry: { level: level, text: parts.join(' '), ts: Date.now() } }, '*');
  }
  var orig = {};
  ['log','warn','error','info','debug'].forEach(function(m){
    orig[m] = console[m];
    console[m] = function(){ send(m, arguments); if(orig[m]) orig[m].apply(console, arguments); };
  });
  window.onerror = function(msg, src, line, col) {
    send('error', [msg + (src ? ' at ' + src + ':' + line + ':' + col : '')]);
  };
  window.addEventListener('unhandledrejection', function(e) {
    send('error', ['Unhandled rejection: ' + (e.reason && e.reason.message || e.reason || 'unknown')]);
  });
  window.addEventListener('error', function(e) {
    if (e.target && e.target !== window) {
      var tag = e.target.tagName || '';
      var src = e.target.src || e.target.href || '';
      send('error', ['Failed to load ' + tag.toLowerCase() + (src ? ': ' + src : '')]);
    }
  }, true);
})();`;

// Script to intercept relative link clicks and navigate via parent.
const NAV_INTERCEPT_JS = `(function(){
  document.addEventListener('click', function(e) {
    var a = e.target.closest('a[href]');
    if (!a) return;
    var href = a.getAttribute('href');
    if (!href || href.startsWith('#') || href.startsWith('javascript:')) return;
    if (href.startsWith('http:') || href.startsWith('https:') || href.startsWith('//') || href.startsWith('mailto:')) return;
    e.preventDefault();
    parent.postMessage({ type: '__build_preview_navigate', href: href }, '*');
  });
})();`;

// ---- Browser Proxy View ----

export function urlFetchAsync(deviceId, url, tabId, method, body, contentType) {
  return new Promise((resolve, reject) => {
    const conn = state.e2eeConnections.get(deviceId);
    if (!conn || !conn.connected) return reject(new Error('not connected'));
    const requestId = 'rf-' + Date.now() + '-' + Math.random().toString(36).slice(2, 6);
    function handler(evt) {
      const d = evt.detail;
      if (d.request_id !== requestId) return;
      conn.removeEventListener('url_fetch_result', handler);
      if (d.error) return reject(new Error(d.error));
      resolve(d);
    }
    conn.addEventListener('url_fetch_result', handler);
    conn.urlFetch(url, requestId, tabId || '', method, body, contentType);
  });
}

export function resolveUrl(baseUrl, href) {
  if (!href || href.startsWith('data:') || href.startsWith('#') || href.startsWith('javascript:')) return null;
  try { return new URL(href, baseUrl).href; } catch { return null; }
}

async function fetchAndRenderBrowserPage(tab) {
  const browserContent = document.getElementById('browser-content');
  if (!browserContent) return;
  browserContent.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';

  const deviceId = tab.deviceId;
  const url = tab.url;
  let errors = [];
  const _dbg = (text) => errors.push({ level: 'debug', text });

  try {
    const method = tab._method || 'GET';
    const body = tab._body || undefined;
    const contentType = tab._contentType || undefined;
    // Clear one-shot POST data after use
    tab._method = undefined;
    tab._body = undefined;
    tab._contentType = undefined;
    _dbg('Fetching ' + method + ' ' + url);
    const result = await urlFetchAsync(deviceId, url, tab.id, method, body, contentType);
    // Update URL bar if server redirected us
    if (result.final_url && result.final_url !== url) {
      tab.url = result.final_url;
      const urlInput = document.getElementById('browser-url-input');
      if (urlInput) urlInput.value = result.final_url;
      _dbg('Redirected to ' + result.final_url);
    }
    if (result.is_binary) {
      browserContent.innerHTML = '<div class="empty-state"><p>Cannot display binary content</p></div>';
      return;
    }
    if (result.status && result.status >= 400) {
      _dbg('HTTP ' + result.status);
    }

    let html = result.content;
    const baseUrl = result.final_url || url;
    _dbg('Got ' + html.length + ' bytes');

    // Resolve and inline local assets (CSS, JS, images)
    const replacements = [];
    const isExternal = (u) => {
      if (!u) return true;
      try { const p = new URL(u, baseUrl); return p.origin !== new URL(baseUrl).origin; } catch { return true; }
    };

    // CSS links
    const linkRe = /<link\b[^>]*\brel\s*=\s*["']stylesheet["'][^>]*>|<link\b[^>]*\bhref\s*=\s*["'][^"']+["'][^>]*\brel\s*=\s*["']stylesheet["'][^>]*>/gi;
    const hrefRe = /\bhref\s*=\s*["']([^"']+)["']/i;
    for (const m of html.matchAll(linkRe)) {
      const tag = m[0];
      const hm = tag.match(hrefRe);
      if (!hm) continue;
      const href = hm[1];
      const resolved = resolveUrl(baseUrl, href);
      if (!resolved || isExternal(resolved)) { _dbg('CSS (ext): ' + href); continue; }
      _dbg('CSS: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.content && !r.is_binary) {
          // Resolve url() references inside CSS
          let css = r.content;
          css = css.replace(/url\(\s*["']?(?!data:|https?:|\/\/)([^"')]+)["']?\s*\)/g, (match, ref) => {
            const absRef = resolveUrl(resolved, ref);
            return absRef ? `url(${absRef})` : match;
          });
          return { original: tag, replacement: '<style>' + css + '</style>' };
        }
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'CSS failed: ' + href + ' (' + err.message + ')' }); return null; }));
    }

    // Script tags
    const scriptRe = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>\s*<\/script>/gi;
    for (const m of html.matchAll(scriptRe)) {
      const tag = m[0];
      const src = m[1];
      const resolved = resolveUrl(baseUrl, src);
      if (!resolved || isExternal(resolved)) { _dbg('JS (ext): ' + src); continue; }
      _dbg('JS: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.content && !r.is_binary) return { original: tag, replacement: '<script>' + r.content + '<\/script>' };
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'JS failed: ' + src + ' (' + err.message + ')' }); return null; }));
    }

    // Images
    const imgRe = /(<img\b[^>]*\bsrc\s*=\s*["'])([^"']+)(["'][^>]*>)/gi;
    for (const m of html.matchAll(imgRe)) {
      const tag = m[0];
      const src = m[2];
      if (src.startsWith('data:')) continue;
      const resolved = resolveUrl(baseUrl, src);
      if (!resolved || isExternal(resolved)) continue;
      _dbg('IMG: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.is_binary && r.content) {
          const ct = r.content_type || 'image/png';
          const mime = ct.split(';')[0].trim();
          return { original: tag, replacement: m[1] + 'data:' + mime + ';base64,' + r.content + m[3] };
        }
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'IMG failed: ' + src + ' (' + err.message + ')' }); return null; }));
    }

    _dbg('Fetching ' + replacements.length + ' assets...');
    const results = await Promise.all(replacements);
    for (const r of results) {
      if (r) html = html.replace(r.original, r.replacement);
    }

    // Bail if user navigated away
    if (state.activeBrowserTab !== tab.id) return;

    // Inject console capture + navigation intercept
    const headMatch = html.match(/<head[^>]*>/i);
    if (headMatch) {
      const idx = html.indexOf(headMatch[0]) + headMatch[0].length;
      const navJs = `(function(){
        // --- Patch fetch to proxy through Build ---
        var _origFetch = window.fetch;
        var _reqId = 0;
        var _pending = {};
        window.addEventListener('message', function(evt) {
          if (evt.data && evt.data.type === '__build_fetch_response' && _pending[evt.data.reqId]) {
            _pending[evt.data.reqId](evt.data);
            delete _pending[evt.data.reqId];
          }
        });
        window.fetch = function(input, init) {
          init = init || {};
          var url = typeof input === 'string' ? input : (input && input.url ? input.url : String(input));
          var method = (init.method || (input && input.method) || 'GET').toUpperCase();
          var body = init.body || null;
          var contentType = null;
          var headers = init.headers;
          if (headers) {
            if (typeof headers.get === 'function') contentType = headers.get('content-type');
            else if (headers['Content-Type']) contentType = headers['Content-Type'];
            else if (headers['content-type']) contentType = headers['content-type'];
          }
          if (body && typeof body !== 'string') {
            try { body = new URLSearchParams(body).toString(); if (!contentType) contentType = 'application/x-www-form-urlencoded'; } catch(e) { body = String(body); }
          }
          var id = '__bf_' + (++_reqId);
          return new Promise(function(resolve) {
            _pending[id] = function(data) {
              var respInit = { status: data.status || 200, headers: { 'Content-Type': data.contentType || 'text/plain' } };
              resolve(new Response(data.body || '', respInit));
            };
            parent.postMessage({ type: '__build_browser_fetch', reqId: id, url: url, method: method, body: body, contentType: contentType }, '*');
          });
        };

        // --- Patch XMLHttpRequest to proxy through Build ---
        var _OrigXHR = XMLHttpRequest;
        function ProxyXHR() {
          this._method = 'GET'; this._url = ''; this._headers = {}; this._async = true;
          this.readyState = 0; this.status = 0; this.statusText = '';
          this.responseText = ''; this.response = ''; this.responseType = '';
          this.onreadystatechange = null; this.onload = null; this.onerror = null;
          this._listeners = {};
        }
        ProxyXHR.prototype.open = function(method, url, async) { this._method = method; this._url = url; this._async = async !== false; this.readyState = 1; };
        ProxyXHR.prototype.setRequestHeader = function(k, v) { this._headers[k.toLowerCase()] = v; };
        ProxyXHR.prototype.getResponseHeader = function(k) { return this._responseHeaders ? (this._responseHeaders[k.toLowerCase()] || null) : null; };
        ProxyXHR.prototype.getAllResponseHeaders = function() { return ''; };
        ProxyXHR.prototype.addEventListener = function(e, fn) { if (!this._listeners[e]) this._listeners[e] = []; this._listeners[e].push(fn); };
        ProxyXHR.prototype.removeEventListener = function(e, fn) { if (this._listeners[e]) this._listeners[e] = this._listeners[e].filter(function(f){return f !== fn;}); };
        ProxyXHR.prototype._fire = function(e) { var fns = this._listeners[e] || []; for (var i = 0; i < fns.length; i++) fns[i].call(this, {}); };
        ProxyXHR.prototype.send = function(body) {
          var self = this;
          var id = '__bf_' + (++_reqId);
          _pending[id] = function(data) {
            self.status = data.status || 200;
            self.statusText = data.status ? String(data.status) : 'OK';
            self.responseText = data.body || '';
            self.response = data.body || '';
            self._responseHeaders = { 'content-type': data.contentType || 'text/plain' };
            self.readyState = 4;
            if (self.onreadystatechange) self.onreadystatechange();
            if (self.onload) self.onload();
            self._fire('readystatechange');
            self._fire('load');
            self._fire('loadend');
          };
          parent.postMessage({ type: '__build_browser_fetch', reqId: id, url: self._url, method: self._method, body: body || null, contentType: self._headers['content-type'] || null }, '*');
        };
        ProxyXHR.prototype.abort = function() {};
        ProxyXHR.prototype.overrideMimeType = function() {};
        window.XMLHttpRequest = ProxyXHR;

        // --- Navigation intercept (lower priority: skip if page already handled) ---
        document.addEventListener('click', function(e) {
          if (e.defaultPrevented) return;
          var a = e.target.closest('a[href]');
          if (!a) return;
          var href = a.getAttribute('href');
          if (!href || href.startsWith('#') || href.startsWith('javascript:')) return;
          e.preventDefault();
          parent.postMessage({ type: '__build_browser_navigate', href: href }, '*');
        });
        document.addEventListener('submit', function(e) {
          if (e.defaultPrevented) return;
          var form = e.target;
          if (!form || form.tagName !== 'FORM') return;
          e.preventDefault();
          var action = form.getAttribute('action') || window.location.href;
          var method = (form.getAttribute('method') || 'GET').toUpperCase();
          var fd = new FormData(form);
          if (method === 'GET') {
            var params = new URLSearchParams(fd).toString();
            var sep = action.indexOf('?') === -1 ? '?' : '&';
            parent.postMessage({ type: '__build_browser_navigate', href: action + sep + params }, '*');
          } else {
            parent.postMessage({ type: '__build_browser_form_submit', action: action, method: method, body: new URLSearchParams(fd).toString(), contentType: 'application/x-www-form-urlencoded' }, '*');
          }
        });
      })();`;
      html = html.slice(0, idx) + '<script>' + CONSOLE_CAPTURE_JS + navJs + '<\/script>' + html.slice(idx);
    }

    const inlinedHtml = html;

    // Build wrapper
    browserContent.innerHTML = '';
    const wrapper = document.createElement('div');
    wrapper.style.cssText = 'position:relative;width:100%;height:100%;display:flex;flex-direction:column';

    const iframe = document.createElement('iframe');
    iframe.style.cssText = 'width:100%;flex:1;border:none;background:#fff;min-height:0';
    wrapper.appendChild(iframe);

    // Console overlay (matches activity/terminal console style)
    const consoleBar = document.createElement('div');
    consoleBar.className = 'html-console-bar';
    consoleBar.innerHTML = '<span class="html-console-title">Console</span><span class="html-console-badge hc-hidden">0</span><span class="html-console-toggle"><svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 9l4-4 4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg></span>';
    wrapper.appendChild(consoleBar);

    const consolePanel = document.createElement('div');
    consolePanel.className = 'html-console-panel hc-hidden';
    wrapper.appendChild(consolePanel);

    browserContent.appendChild(wrapper);

    const badge = consoleBar.querySelector('.html-console-badge');
    const toggleIcon = consoleBar.querySelector('.html-console-toggle');
    let consoleOpen = false;
    let entryCount = 0;

    consoleBar.addEventListener('click', () => {
      consoleOpen = !consoleOpen;
      consolePanel.classList.toggle('hc-hidden', !consoleOpen);
      toggleIcon.innerHTML = consoleOpen
        ? '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 5l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>'
        : '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 9l4-4 4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
      if (consoleOpen) { badge.classList.add('hc-hidden'); consolePanel.scrollTop = consolePanel.scrollHeight; }
    });

    function addConsoleEntry(level, text) {
      entryCount++;
      if (!consoleOpen) { badge.classList.remove('hc-hidden'); badge.textContent = String(entryCount); }
      const row = document.createElement('div');
      row.className = 'html-console-entry html-console-' + (level || 'log');
      const levelSpan = document.createElement('span');
      levelSpan.className = 'html-console-level';
      levelSpan.textContent = level || 'log';
      row.appendChild(levelSpan);
      const textSpan = document.createElement('span');
      textSpan.textContent = text;
      row.appendChild(textSpan);
      consolePanel.appendChild(row);
      if (consoleOpen) consolePanel.scrollTop = consolePanel.scrollHeight;
    }

    // Populate errors but keep console collapsed by default
    for (const err of errors) addConsoleEntry(err.level, err.text);

    function onMsg(evt) {
      if (!evt.data) return;
      if (evt.data.type === '__build_console') {
        addConsoleEntry(evt.data.entry.level, evt.data.entry.text);
      } else if (evt.data.type === '__build_preview_ready') {
        iframe.contentWindow.postMessage({ type: '__build_preview', html: inlinedHtml }, '*');
      } else if (evt.data.type === '__build_browser_navigate') {
        const target = resolveUrl(baseUrl, evt.data.href);
        if (target && new URL(target).origin === new URL(baseUrl).origin) {
          tab.url = target;
          tab._method = undefined;
          tab._body = undefined;
          tab._contentType = undefined;
          document.getElementById('browser-url-input').value = target;
          renderChannelPanel();
          fetchAndRenderBrowserPage(tab);
        }
      } else if (evt.data.type === '__build_browser_form_submit') {
        const target = resolveUrl(baseUrl, evt.data.action);
        if (target && new URL(target).origin === new URL(baseUrl).origin) {
          tab.url = target;
          tab._method = evt.data.method;
          tab._body = evt.data.body;
          tab._contentType = evt.data.contentType;
          document.getElementById('browser-url-input').value = target;
          renderChannelPanel();
          fetchAndRenderBrowserPage(tab);
        }
      } else if (evt.data.type === '__build_browser_fetch') {
        const reqId = evt.data.reqId;
        const fetchUrl = resolveUrl(baseUrl, evt.data.url);
        if (!fetchUrl) {
          iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: 0, body: 'Invalid URL', contentType: 'text/plain' }, '*');
          return;
        }
        urlFetchAsync(deviceId, fetchUrl, tab.id, evt.data.method || 'GET', evt.data.body || undefined, evt.data.contentType || undefined)
          .then(r => {
            iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: r.status || 200, body: r.content || '', contentType: r.content_type || 'text/plain' }, '*');
          })
          .catch(err => {
            iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: 0, body: err.message, contentType: 'text/plain' }, '*');
          });
      }
    }
    window.addEventListener('message', onMsg);

    const observer = new MutationObserver(() => {
      if (!browserContent.contains(wrapper)) {
        window.removeEventListener('message', onMsg);
        observer.disconnect();
      }
    });
    observer.observe(browserContent, { childList: true });

    iframe.src = '/preview-frame';

  } catch (err) {
    browserContent.innerHTML = '<div class="empty-state"><p>Error: ' + escapeHtml(err.message) + '</p></div>';
  }
}

export function showBrowserView(tab) {
  // Hide all tab panels, show browser panel
  document.querySelectorAll('.tab-panel').forEach(p => p.classList.remove('active'));
  document.getElementById('tab-browser')?.classList.add('active');
  // Hide tab buttons active state
  document.querySelectorAll('.viewer-tab[data-tab]').forEach(btn => btn.classList.remove('active'));
  // Browser owns the full right column — hide everything else
  document.getElementById('viewer-tabs')?.classList.add('hidden');
  document.querySelector('.viewer-body')?.classList.add('hidden');
  document.querySelector('.comp-wrapper')?.classList.add('hidden');
  document.getElementById('console-bottom')?.classList.add('hidden');

  const urlInput = document.getElementById('browser-url-input');
  if (urlInput) urlInput.value = tab.url || '';

  // If URL looks valid, load it
  if (tab.url && tab.url.startsWith('http')) {
    fetchAndRenderBrowserPage(tab);
  } else {
    const browserContent = document.getElementById('browser-content');
    if (browserContent) browserContent.innerHTML = '<div class="empty-state"><p>Enter a localhost address and press Go</p></div>';
  }
}

// Browser URL bar handlers
document.getElementById('browser-url-go')?.addEventListener('click', () => {
  if (!state.activeBrowserTab) return;
  const urlInput = document.getElementById('browser-url-input');
  const url = urlInput?.value?.trim();
  if (!url) return;
  // Find the active tab and update its URL
  for (const [, tabs] of state.browserTabs) {
    const tab = tabs.find(t => t.id === state.activeBrowserTab);
    if (tab) {
      tab.url = url;
      renderChannelPanel();
      fetchAndRenderBrowserPage(tab);
      break;
    }
  }
});

document.getElementById('browser-url-input')?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    e.preventDefault();
    document.getElementById('browser-url-go')?.click();
  }
});

// Tracks asset-fetch errors to surface in the console overlay.
let _htmlPreviewErrors = [];

async function renderHtmlPreview(content, htmlPath) {
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading preview...</p></div>';
  _htmlPreviewErrors = [];
  const _dbg = (text) => _htmlPreviewErrors.push({ level: 'debug', text });

  const channelId = state.filesChannelId;
  let html = content;

  _dbg('Rendering ' + htmlPath + ' (' + content.length + ' bytes)');

  const isExternal = (url) => url && (url.startsWith('http:') || url.startsWith('https:') || url.startsWith('//'));

  // Use string-based asset resolution (avoids DOMParser mangling style/script content).
  const replacements = [];

  // Find local CSS <link> tags (external left as-is — blob iframe has no CSP restrictions).
  const linkRe = /<link\b[^>]*\brel\s*=\s*["']stylesheet["'][^>]*>|<link\b[^>]*\bhref\s*=\s*["'][^"']+["'][^>]*\brel\s*=\s*["']stylesheet["'][^>]*>/gi;
  const hrefRe = /\bhref\s*=\s*["']([^"']+)["']/i;
  for (const m of html.matchAll(linkRe)) {
    const tag = m[0];
    const hm = tag.match(hrefRe);
    if (!hm) continue;
    const href = hm[1];
    if (isExternal(href)) { _dbg('CSS (ext, kept): ' + href); continue; }
    const resolved = resolveAssetPath(htmlPath, href);
    _dbg('CSS (local): ' + href + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.content && !result.is_binary && !result.is_image) {
        _dbg('CSS OK: ' + result.content.length + ' bytes');
        return { original: tag, replacement: '<style>' + result.content + '</style>' };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'CSS failed: ' + href + ' (' + err.message + ')' });
      return null;
    }));
  }

  // Find local script[src] tags (external left as-is).
  const scriptRe = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>\s*<\/script>/gi;
  for (const m of html.matchAll(scriptRe)) {
    const tag = m[0];
    const src = m[1];
    if (isExternal(src)) { _dbg('JS (ext, kept): ' + src); continue; }
    const resolved = resolveAssetPath(htmlPath, src);
    _dbg('JS (local): ' + src + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.content && !result.is_binary && !result.is_image) {
        _dbg('JS OK: ' + result.content.length + ' bytes');
        return { original: tag, replacement: '<script>' + result.content + '<\/script>' };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'JS failed: ' + src + ' (' + err.message + ')' });
      return null;
    }));
  }

  // Find local <img src> tags.
  const imgRe = /(<img\b[^>]*\bsrc\s*=\s*["'])([^"']+)(["'][^>]*>)/gi;
  for (const m of html.matchAll(imgRe)) {
    const tag = m[0];
    const src = m[2];
    if (isExternal(src) || src.startsWith('data:')) continue;
    const resolved = resolveAssetPath(htmlPath, src);
    _dbg('IMG (local): ' + src + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.is_image && result.content) {
        _dbg('IMG OK: ' + resolved);
        return { original: tag, replacement: m[1] + result.content + m[3] };
      } else if (result.content && !result.is_binary) {
        const ext = resolved.split('.').pop().toLowerCase();
        const mime = MIME_TYPES[ext] || 'application/octet-stream';
        return { original: tag, replacement: m[1] + 'data:' + mime + ';base64,' + btoa(result.content) + m[3] };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'IMG failed: ' + src + ' (' + err.message + ')' });
      return null;
    }));
  }

  _dbg('Fetching ' + replacements.length + ' assets...');
  const results = await Promise.all(replacements);
  for (const r of results) {
    if (r) html = html.replace(r.original, r.replacement);
  }
  _dbg('Done. ' + _htmlPreviewErrors.filter(e => e.level === 'error').length + ' errors');

  // Bail if user navigated away.
  if (state.filesCurrentPath !== htmlPath || state.filesChannelId !== channelId) return;

  // Inject console capture script right after <head>.
  const headMatch = html.match(/<head[^>]*>/i);
  if (headMatch) {
    const idx = html.indexOf(headMatch[0]) + headMatch[0].length;
    html = html.slice(0, idx) + '<script>' + CONSOLE_CAPTURE_JS + NAV_INTERCEPT_JS + '<\/script>' + html.slice(idx);
  }

  const inlinedHtml = html;

  // Build wrapper with iframe and console overlay.
  fileContentBody.innerHTML = '';
  fileContentBody.style.padding = '0';

  const wrapper = document.createElement('div');
  wrapper.style.cssText = 'position:relative;width:100%;height:100%;display:flex;flex-direction:column';

  const iframe = document.createElement('iframe');
  // No sandbox attr — blob URL already has opaque origin (isolated from parent).
  iframe.style.cssText = 'width:100%;flex:1;border:none;background:#fff;border-radius:4px 4px 0 0;min-height:0';
  wrapper.appendChild(iframe);

  // Console overlay.
  const consoleBar = document.createElement('div');
  consoleBar.className = 'html-console-bar';
  consoleBar.innerHTML = '<span class="html-console-title">Console</span><span class="html-console-badge hc-hidden">0</span><span class="html-console-toggle">&#x25B2;</span>';
  wrapper.appendChild(consoleBar);

  const consolePanel = document.createElement('div');
  consolePanel.className = 'html-console-panel hc-hidden';
  wrapper.appendChild(consolePanel);

  fileContentBody.appendChild(wrapper);

  const badge = consoleBar.querySelector('.html-console-badge');
  const toggleIcon = consoleBar.querySelector('.html-console-toggle');
  let consoleOpen = false;
  let entryCount = 0;

  consoleBar.addEventListener('click', () => {
    consoleOpen = !consoleOpen;
    consolePanel.classList.toggle('hc-hidden', !consoleOpen);
    toggleIcon.innerHTML = consoleOpen ? '&#x25BC;' : '&#x25B2;';
    if (consoleOpen) { badge.classList.add('hc-hidden'); consolePanel.scrollTop = consolePanel.scrollHeight; }
  });

  function addConsoleEntry(level, text) {
    entryCount++;
    if (!consoleOpen) { badge.classList.remove('hc-hidden'); badge.textContent = String(entryCount); }
    const row = document.createElement('div');
    row.className = 'html-console-entry html-console-' + (level || 'log');
    const levelSpan = document.createElement('span');
    levelSpan.className = 'html-console-level';
    levelSpan.textContent = level || 'log';
    row.appendChild(levelSpan);
    const textSpan = document.createElement('span');
    textSpan.textContent = text;
    row.appendChild(textSpan);
    consolePanel.appendChild(row);
    if (consoleOpen) consolePanel.scrollTop = consolePanel.scrollHeight;
  }

  // Surface asset-fetch errors/debug that happened before iframe loaded.
  for (const err of _htmlPreviewErrors) addConsoleEntry(err.level, err.text);
  // Auto-open console if there are entries.
  if (_htmlPreviewErrors.length > 0) {
    consoleOpen = true;
    consolePanel.classList.remove('hc-hidden');
    toggleIcon.innerHTML = '&#x25BC;';
    badge.classList.add('hc-hidden');
  }

  // Listen for console messages and preview-ready signal from iframe.
  function onMsg(evt) {
    if (!evt.data) return;
    if (evt.data.type === '__build_console') {
      addConsoleEntry(evt.data.entry.level, evt.data.entry.text);
    } else if (evt.data.type === '__build_preview_ready') {
      // Preview frame is ready — send the HTML content.
      iframe.contentWindow.postMessage({ type: '__build_preview', html: inlinedHtml }, '*');
    } else if (evt.data.type === '__build_preview_navigate') {
      // Relative link clicked in preview — navigate to that file.
      const targetPath = resolveAssetPath(htmlPath, evt.data.href);
      if (targetPath) {
        // Find the entry in the file tree data and select it in preview mode.
        const data = state.fileTreeData.get(state.filesChannelId);
        const dir = targetPath.substring(0, targetPath.lastIndexOf('/') + 1);
        const dirKey = dir ? dir.slice(0, -1) : '';
        const entries = data && data.get(dirKey);
        const entry = entries && entries.entries && entries.entries.find(e => (e.path || e.name) === targetPath);
        selectFile(targetPath, entry || null, 'rendered');
        // Fetch and render the file.
        const conn = window.getE2EE?.(state.filesChannelId);
        if (conn && conn.connected) {
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
          conn.fileRead(state.filesChannelId, targetPath);
        }
      }
    }
  }
  window.addEventListener('message', onMsg);

  // Clean up listener when content changes.
  const observer = new MutationObserver(() => {
    if (!fileContentBody.contains(wrapper)) {
      window.removeEventListener('message', onMsg);
      observer.disconnect();
    }
  });
  observer.observe(fileContentBody, { childList: true });

  // Load preview frame (has its own permissive CSP).
  iframe.src = '/preview-frame';
}

// ---- Diff Renderer ----

export function renderDiffContent(diffText, truncated) {
  fileContentBody.style.padding = '';
  const lines = diffText.split('\n');
  const viewer = document.createElement('div');
  viewer.className = 'diff-viewer';

  // Derive file extension for syntax highlighting.
  const extMatch = state.filesCurrentPath ? state.filesCurrentPath.match(/\.([^./]+)$/) : null;
  const ext = extMatch ? extMatch[1].toLowerCase() : '';

  // Pre-parse lines into typed entries.
  const entries = [];
  let oldNum = 0, newNum = 0;
  for (const line of lines) {
    if (line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('---') || line.startsWith('+++')) continue;
    if (line.startsWith('@@')) {
      const m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) { oldNum = parseInt(m[1]); newNum = parseInt(m[2]); }
      entries.push({ type: 'hunk', text: line });
      continue;
    }
    if (line.startsWith('+')) {
      entries.push({ type: 'add', text: line.slice(1), num: String(newNum++) });
    } else if (line.startsWith('-')) {
      entries.push({ type: 'del', text: line.slice(1), num: String(oldNum++) });
    } else {
      entries.push({ type: 'ctx', text: line.startsWith(' ') ? line.slice(1) : line, num: String(newNum) });
      oldNum++; newNum++;
    }
  }

  function addRow(cls, num, html) {
    const row = document.createElement('div');
    row.className = 'diff-line ' + cls;
    const numEl = document.createElement('span');
    numEl.className = 'dl-num';
    numEl.textContent = num;
    const contentEl = document.createElement('span');
    contentEl.className = 'dl-content';
    contentEl.innerHTML = html;
    row.appendChild(numEl);
    row.appendChild(contentEl);
    viewer.appendChild(row);
  }

  for (let ei = 0; ei < entries.length; ei++) {
    const e = entries[ei];
    if (e.type === 'hunk') {
      const hdr = document.createElement('div');
      hdr.className = 'diff-hunk-header';
      hdr.textContent = e.text;
      viewer.appendChild(hdr);
      continue;
    }
    if (e.type === 'ctx') {
      addRow('context', e.num, highlightLine(e.text, ext));
      continue;
    }
    if (e.type === 'del') {
      const dels = [e];
      while (ei + 1 < entries.length && entries[ei + 1].type === 'del') dels.push(entries[++ei]);
      const adds = [];
      while (ei + 1 < entries.length && entries[ei + 1].type === 'add') adds.push(entries[++ei]);
      const pairs = Math.min(dels.length, adds.length);
      for (let pi = 0; pi < dels.length; pi++) {
        const html = pi < pairs ? wordDiffLine(dels[pi].text, adds[pi].text).oldHtml : highlightLine(dels[pi].text, ext);
        addRow('removed', dels[pi].num, html);
      }
      for (let ai = 0; ai < adds.length; ai++) {
        const html = ai < pairs ? wordDiffLine(dels[ai].text, adds[ai].text).newHtml : highlightLine(adds[ai].text, ext);
        addRow('added', adds[ai].num, html);
      }
      continue;
    }
    if (e.type === 'add') {
      addRow('added', e.num, highlightLine(e.text, ext));
    }
  }

  fileContentBody.innerHTML = '';
  fileContentBody.appendChild(viewer);

  if (truncated) {
    const note = document.createElement('div');
    note.style.cssText = 'padding:.5rem .75rem;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border)';
    note.textContent = 'Diff truncated';
    fileContentBody.appendChild(note);
  }
}


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
    const saved = window.loadChannelState?.(state.chatCurrentChannel);
    if (saved.filesPath) {
      state.filesPendingRestore = saved.filesPath;
      state.filesPendingView = saved.filesView || 'source';
    } else {
      state.filesPendingRestore = null;
    }

    filesLoadRoot();
    const _ftaConn = window.getActiveE2EE?.();
    if (state.filesTreeTab === 'changes' && _ftaConn && _ftaConn.connected) {
      _ftaConn.filesChanges(state.chatCurrentChannel);
    }
  }
}

// Post-switch hook — dashboard's switchTab calls this so Files can react.
// The shell/tabs module invokes window.onTabSwitched after the DOM toggle.
window.onTabSwitched = function(tab) {
  if (tab === 'files') onFilesTabActivated();
  if (state.chatCurrentChannel) window.saveChannelState?.(state.chatCurrentChannel, { activeTab: tab });
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
