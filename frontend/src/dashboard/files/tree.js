import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { getE2EE, getActiveE2EE } from '../e2ee/bridge.js';
import { saveChannelState } from '../channels/state-store.js';
import {
  fileTree,
  fileTreeInner,
  fileTreeEmpty,
  filesPathText,
  filesPathChevron,
  fileReloadBtn,
  fileTreePanel,
  fileContentBody,
} from './refs.js';
import { onFilesTabActivated, updateFloatingToggle } from './mode.js';

export function filesLoadRoot() {
  const _flConn = getActiveE2EE();
  if (!state.chatCurrentChannel || !_flConn || !_flConn.connected) return;
  state.filesChannelId = state.chatCurrentChannel;
  _flConn.filesList(state.chatCurrentChannel, '');
}

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

    if (entry.is_gitignored) item.classList.add('gitignored');
    if (entry.git_status === '?' || entry.staged_status === '?') item.classList.add('untracked');
    if (entry.path === state.filesCurrentPath) item.classList.add('active');

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

    const icon = document.createElement('span');
    icon.className = 'tree-icon' + (entry.type === 'dir' ? ' folder' : '');
    icon.innerHTML = entry.type === 'dir'
      ? '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M1.5 3.5v7a1 1 0 001 1h9a1 1 0 001-1v-5a1 1 0 00-1-1H7L5.5 2.5h-3a1 1 0 00-1 1z" stroke="currentColor" stroke-width="1.2"/></svg>'
      : '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3.5 1.5h4l3 3v7a1 1 0 01-1 1h-6a1 1 0 01-1-1v-9a1 1 0 011-1z" stroke="currentColor" stroke-width="1.2"/><path d="M7.5 1.5v3h3" stroke="currentColor" stroke-width="1.2"/></svg>';
    item.appendChild(icon);

    const label = document.createElement('span');
    label.className = 'tree-label';
    label.textContent = entry.name;
    label.title = entry.path || entry.name;
    item.appendChild(label);

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

    item.addEventListener('click', (ev) => {
      ev.stopPropagation();
      if (entry.type === 'dir') {
        toggleDirectory(entry.path, arrow, item);
      } else {
        selectFile(entry.path, entry);
      }
    });

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
    data.delete(path);
    renderFileTree();
  } else {
    getE2EE(state.filesChannelId)?.filesList(state.filesChannelId, path);
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

  if (initialView === 'diff' && state.filesCurrentHasDiff) {
    state.filesCurrentView = 'diff';
  } else if (state.filesCurrentIsMarkdown || state.filesCurrentIsSvg || state.filesCurrentIsHtml) {
    state.filesCurrentView = 'rendered';
  } else {
    state.filesCurrentView = 'source';
  }

  fileTree.querySelectorAll('.tree-item.active').forEach(el => el.classList.remove('active'));
  const active = fileTree.querySelector(`.tree-item[data-path="${CSS.escape(path)}"]`);
  if (active) active.classList.add('active');

  filesPathText.textContent = path;
  filesPathText.classList.remove('empty');
  fileReloadBtn.classList.remove('hc-hidden');

  fileTreePanel.classList.remove('mobile-open');
  filesPathChevron.classList.remove('open');

  updateFloatingToggle();

  saveChannelState(state.filesChannelId, { filesPath: path, filesView: state.filesCurrentView });

  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
  const _fileConn = getE2EE(state.filesChannelId);
  if (state.filesCurrentView === 'diff') {
    _fileConn?.fileDiff(state.filesChannelId, path, false);
  } else {
    _fileConn?.fileRead(state.filesChannelId, path);
  }
}

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
    const conn = getE2EE(state.filesChannelId || channelId);
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
