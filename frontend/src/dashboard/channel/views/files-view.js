// FilesView — tree + viewer (source / diff / preview). Mounts into #v2-tab-files.
// See planning/dashboard/05-views.md § FilesView.

import { bus } from '../../core/bus.js';
import { filesStore } from '../../domain/files-store.js';
import { escapeHtml, formatBytes } from '../../util/html.js';
import { uploadFile } from '../../transport/index.js';
import { highlightLine } from '../../util/syntax.js';
import { renderDiff } from '../../util/diff.js';
import { renderMarkdown } from '../../util/markdown.js';
import { showToast } from '../../util/toast.js';

const MODE_LABELS = { source: 'Source', diff: 'Diff', preview: 'Preview' };

export class FilesView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.treeEl = null;
    this.viewerEl = null;
    this.modeBarEl = null;
    this.unsubs = [];
    this._requestedCommitRepos = new Set();
  }

  activate() {
    this.root = document.getElementById('v2-viewer-main');
    if (!this.root) return;
    this._buildShell();
    // Initial data fetch now lives in Channel.load(), which waits for
    // sessionStore.awaitChannelReady before firing files_list /
    // files_changes intents. The view just renders whatever lands in
    // filesStore.
    this.unsubs.push(filesStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      if (e.kind === 'tree' || e.kind === 'changes' || e.kind === 'commits') this._renderTree();
      if (e.kind === 'read_result' || e.kind === 'read_result_progress') this._renderReadResult();
      if (e.kind === 'diff_result') this._renderDiffResult();
    }));
    // Live refresh when the agent reports file changes.
    this.unsubs.push(bus.on('agent.file_changes', ({ channelId, paths }) => {
      if (channelId !== this.channel.id) return;
      this._scheduleRefresh(paths || []);
    }));
    // Git-status complications fire on commits (and on index changes).
    // Use them to clear/refresh the Modified list even when no files
    // actually changed on disk.
    this.unsubs.push(bus.on('complication.upserted', ({ channelId }) => {
      if (channelId !== this.channel.id) return;
      this._scheduleRefresh([]);
    }));
    this.unsubs.push(bus.on('complications.bulk', ({ channelId }) => {
      if (channelId !== this.channel.id) return;
      this._scheduleRefresh([]);
    }));
    this._renderTree();
    this._renderViewer();
    this._restoreWorkspace();

    // Persist scroll positions as the user scrolls (debounced) — gives
    // page-reload unload a fresh value to save.
    this._onTreeScroll = () => {
      if (this.treeEl) this.channel.viewState.filesTreeScrollTop = this.treeEl.scrollTop;
    };
    this._onViewerScroll = () => {
      if (this.viewerEl) this.channel.viewState.filesViewerScrollTop = this.viewerEl.scrollTop;
    };
    this.treeEl?.addEventListener('scroll', this._onTreeScroll, { passive: true });
    this.viewerEl?.addEventListener('scroll', this._onViewerScroll, { passive: true });
  }

  deactivate() {
    // Capture scroll positions one last time so the persist snapshot
    // in Channel.deactivate() has the freshest values.
    if (this.treeEl)   this.channel.viewState.filesTreeScrollTop   = this.treeEl.scrollTop;
    if (this.viewerEl) this.channel.viewState.filesViewerScrollTop = this.viewerEl.scrollTop;
    this.treeEl?.removeEventListener('scroll', this._onTreeScroll);
    this.viewerEl?.removeEventListener('scroll', this._onViewerScroll);
    this.root?.removeEventListener('change', this._onChange);
    if (this.treePanelEl) {
      this.treePanelEl.removeEventListener('dragenter', this._onTreeDragEnter);
      this.treePanelEl.removeEventListener('dragover',  this._onTreeDragOver);
      this.treePanelEl.removeEventListener('dragleave', this._onTreeDragLeave);
      this.treePanelEl.removeEventListener('drop',      this._onTreeDrop);
    }
    this._uploadUnsub?.();      this._uploadUnsub = null;
    this._uploadDoneUnsub?.();  this._uploadDoneUnsub = null;
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this._refreshTimer) { clearTimeout(this._refreshTimer); this._refreshTimer = null; }
    this._pendingPaths = null;
    this.root = null;
    this.treeEl = null;
    this.viewerEl = null;
    this.modeBarEl = null;
    this.treePanelEl = null;
    this.fileInputEl = null;
    this.uploadChipsEl = null;
    this._activeUploads = null;
  }

  /** After initial activate / render, restore the user's workspace:
   *  re-fetch expanded dirs, re-fetch the selected file's content, and
   *  snap the scroll positions back into place. */
  _restoreWorkspace() {
    const vs = this.channel.viewState;
    const tree = filesStore.treeFor(this.channel.id);
    // Ensure viewState.filesExpandedDirs is an array (may be a stale
    // primitive if defaultViewState() migration ran).
    if (!Array.isArray(vs.filesExpandedDirs)) vs.filesExpandedDirs = [];

    // Kick fetches for any expanded dir we don't already have entries
    // for (page reload case). The store update will re-run _renderTree.
    for (const p of vs.filesExpandedDirs) {
      if (!tree.has(p)) bus.emit('intent.files_list', { channelId: this.channel.id, path: p });
    }

    // Re-fetch the open file if its result isn't cached.
    if (vs.filesPath) {
      if (vs.filesView === 'diff') {
        const dr = filesStore.diffResultFor(this.channel.id);
        if (!dr || dr.path !== vs.filesPath) {
          this._emitFileDiff(vs.filesPath);
        }
      } else {
        const rr = filesStore.readResultFor(this.channel.id);
        if (!rr || rr.path !== vs.filesPath) {
          bus.emit('intent.file_read', { channelId: this.channel.id, path: vs.filesPath });
        }
      }
    }

    // Restore scroll positions after the browser has laid out the
    // freshly-mounted panels. scroll assignments BEFORE layout are
    // no-ops, so defer one frame.
    requestAnimationFrame(() => {
      if (this.treeEl)   this.treeEl.scrollTop   = vs.filesTreeScrollTop   || 0;
      if (this.viewerEl) this.viewerEl.scrollTop = vs.filesViewerScrollTop || 0;
    });
  }

  /** Debounce agent.file_changes bursts; refetch on trailing edge. */
  _scheduleRefresh(paths) {
    if (!this._pendingPaths) this._pendingPaths = new Set();
    for (const p of paths) this._pendingPaths.add(p);
    clearTimeout(this._refreshTimer);
    this._refreshTimer = setTimeout(() => this._doRefresh(), 150);
  }

  _doRefresh() {
    const paths = [...(this._pendingPaths || [])];
    this._pendingPaths = null;
    this._refreshTimer = null;
    const channelId = this.channel?.id;
    if (!channelId) return;

    // 1) Refresh the Modified list.
    bus.emit('intent.files_changes', { channelId });

    // 2) If the tree is showing "All", re-fetch root so new / deleted
    //    files in the tree reflect the change.
    if (this.channel.viewState.filesTreeTab === 'all') {
      bus.emit('intent.files_list', { channelId, path: '' });
    }

    // 3) If a file is currently being viewed and was touched, re-fetch.
    const current = this.channel.viewState.filesPath;
    if (!current) return;
    const norm = (p) => (p || '').replace(/\\/g, '/');
    const cur = norm(current);
    const curBase = cur.split('/').pop();
    const touched = paths.some(p => {
      const pp = norm(p);
      if (!pp) return false;
      if (pp === cur) return true;
      if (cur.endsWith('/' + pp)) return true;
      if (pp.endsWith('/' + cur)) return true;
      return pp.split('/').pop() === curBase;
    });
    if (!touched) return;
    if (this.channel.viewState.filesView === 'diff') {
      this._emitFileDiff(current);
    } else {
      bus.emit('intent.file_read', { channelId, path: current });
    }
  }

  _buildShell() {
    // Populate the two pre-built panels in the layout.
    const treePanel = document.getElementById('v2-files-tree-panel');
    const contentInner = document.getElementById('v2-files-content-inner');
    if (!treePanel || !contentInner) return;
    treePanel.innerHTML = `
      <header class="v2-files-tree-tabs">
        <button class="v2-path-bar-btn v2-path-bar-menu v2-tree-tabs-menu"
                id="v2-tree-tabs-menu"
                type="button"
                title="Open menu"
                aria-label="Open menu">
          <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M2.5 4h11M2.5 8h11M2.5 12h11" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
        </button>
        <button class="v2-files-tree-tab" data-tree-tab="changes" type="button">
          Modified <span class="v2-files-tree-tab-count"></span>
        </button>
        <button class="v2-files-tree-tab" data-tree-tab="all" type="button">All</button>
        <button class="v2-files-tree-upload" data-dir-upload="" type="button"
                title="Upload file into workspace root" aria-label="Upload file">
          <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
            <path d="M8 10V2M5 5l3-3 3 3M3 10v3a1 1 0 001 1h8a1 1 0 001-1v-3"
                  stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
      </header>
      <div class="v2-files-tree-body" data-slot="tree"></div>
      <div class="v2-files-tree-skeleton" aria-hidden="true">
        <div class="v2-skeleton v2-skeleton-tree-row"></div>
        <div class="v2-skeleton v2-skeleton-tree-row indent"></div>
        <div class="v2-skeleton v2-skeleton-tree-row indent"></div>
        <div class="v2-skeleton v2-skeleton-tree-row"></div>
        <div class="v2-skeleton v2-skeleton-tree-row indent"></div>
      </div>
      <div class="v2-files-upload-chips" data-slot="upload-chips" hidden></div>
      <input type="file" class="v2-files-file-input" multiple hidden>
    `;
    contentInner.innerHTML = `
      <header class="v2-files-mode-bar" data-slot="mode"></header>
      <div class="v2-files-viewer-body" data-slot="viewer"></div>
      <div class="v2-files-viewer-skeleton" aria-hidden="true">
        <div class="v2-skeleton v2-skeleton-line"></div>
        <div class="v2-skeleton v2-skeleton-line"></div>
        <div class="v2-skeleton v2-skeleton-line short"></div>
        <div class="v2-skeleton v2-skeleton-line"></div>
        <div class="v2-skeleton v2-skeleton-line short"></div>
      </div>
    `;
    this.treeEl = treePanel.querySelector('[data-slot="tree"]');
    this.viewerEl = contentInner.querySelector('[data-slot="viewer"]');
    this.modeBarEl = contentInner.querySelector('[data-slot="mode"]');
    this.treePanelEl = treePanel;
    this.fileInputEl = treePanel.querySelector('.v2-files-file-input');
    this.uploadChipsEl = treePanel.querySelector('[data-slot="upload-chips"]');
    this._activeUploads = new Map();  // fileId → chip element

    this.root.addEventListener('click', this._onClick);
    this.root.addEventListener('change', this._onChange);
    // Hidden <input type="file"> for the per-dir upload button.
    this.fileInputEl.addEventListener('change', this._onFileInputChange);

    // Drag-and-drop uploads into the tree. Depth counter mirrors
    // chat-view so flicker doesn't happen as the drag moves between
    // nested rows.
    this._treeDragDepth = 0;
    this.treePanelEl.addEventListener('dragenter', this._onTreeDragEnter);
    this.treePanelEl.addEventListener('dragover',  this._onTreeDragOver);
    this.treePanelEl.addEventListener('dragleave', this._onTreeDragLeave);
    this.treePanelEl.addEventListener('drop',      this._onTreeDrop);

    // Upload-progress wiring: one chip per in-flight file, removed
    // on `upload.done`. Channel-scoped via upload id lifecycle.
    this._uploadUnsub = bus.on('upload.progress', this._onUploadProgress);
    this._uploadDoneUnsub = bus.on('upload.done', this._onUploadDone);
  }

  // ----- Tree -----

  _renderTree() {
    if (!this.treeEl) return;
    const tab = this.channel.viewState.filesTreeTab;
    // Highlight tabs
    this.root.querySelectorAll('[data-tree-tab]').forEach(el => {
      el.classList.toggle('active', el.getAttribute('data-tree-tab') === tab);
    });

    // Modified count badge on the tab.
    const modCount = (filesStore.changesFor(this.channel.id) || [])
      .reduce((n, r) => n + (r.entries?.length || 0), 0);
    const modBadge = this.root.querySelector('[data-tree-tab="changes"] .v2-files-tree-tab-count');
    if (modBadge) modBadge.textContent = modCount > 0 ? String(modCount) : '';

    if (tab === 'changes') {
      const repos = filesStore.changesFor(this.channel.id);
      const visibleRepos = (repos || []).filter(Boolean);
      if (!visibleRepos.length) {
        this.treeEl.innerHTML = `
          <div class="v2-files-placeholder v2-files-placeholder-compact">
            <svg class="v2-files-placeholder-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M6 3v18M18 3v18"/>
              <path d="M6 9h12M6 15h12"/>
            </svg>
            <div class="v2-files-placeholder-title">No changes yet</div>
            <div class="v2-files-placeholder-body">Edits the agent makes will appear here.</div>
          </div>
        `;
        return;
      }
      for (const repo of visibleRepos) this._ensureRepoCommits(repo.path || '.');
      // Auto-select the first changed file when nothing is selected yet
      // (e.g. the user just landed on a channel and the first change
      // just arrived). Keeps their selection sticky on subsequent
      // tree refreshes.
      if (!this.channel.viewState.filesPath) {
        const firstRepo = visibleRepos.find(r => (r.entries || []).length);
        const firstEntry = firstRepo?.entries.find(Boolean);
        if (firstEntry?.path) {
          // Defer so the tree finishes rendering before _selectFile's
          // own re-render runs.
          queueMicrotask(() => this._selectFile(firstEntry.path, true, firstRepo.path || '.'));
        }
      }
      const repoParts = visibleRepos.map(repo => {
        const repoPath = repo.path || '.';
        const collapsed = this._collapsedReposSet().has(repoPath);
        const comparison = this._repoComparison(repoPath);
        const commitsResult = filesStore.commitsFor(this.channel.id, repoPath);
        const commits = commitsResult?.commits || [];
        const activePath = this.channel.viewState.filesPath;
        // Group entries by parent directory.
        const byDir = new Map();       // dir → entries[]
        for (const c of repo.entries || []) {
          const dir = c.path.includes('/') ? c.path.slice(0, c.path.lastIndexOf('/')) : '';
          if (!byDir.has(dir)) byDir.set(dir, []);
          byDir.get(dir).push(c);
        }
        const dirs = [...byDir.keys()].sort((a, b) => a.localeCompare(b));
        const dirChunks = dirs.map(dir => {
          const entries = byDir.get(dir).slice().sort((a, b) => a.path.localeCompare(b.path));
          const rows = entries.map(c => {
            const status = c.git_status || 'M';
            const letter = status === '?' ? '?' : status[0].toUpperCase();
            const cls = status === '?' ? 'untracked' : status.toLowerCase();
            const ins = c.insertions || 0;
            const del = c.deletions || 0;
            const basename = c.path.split('/').pop();
            return `
              <button class="v2-files-row ${activePath === c.path ? 'active' : ''}"
                      type="button"
                      data-repo-path="${escapeHtml(repoPath)}"
                      data-file-path="${escapeHtml(c.path)}"
                      data-has-diff="1">
                <span class="v2-files-status v2-files-status-${escapeHtml(cls)}">${escapeHtml(letter)}</span>
                <span class="v2-files-name">${escapeHtml(basename)}</span>
                <span class="v2-files-stats">
                  ${ins ? `<span class="v2-files-add">+${ins}</span>` : ''}
                  ${del ? `<span class="v2-files-del">-${del}</span>` : ''}
                </span>
              </button>
            `;
          }).join('');
          const dirHeader = dir
            ? `<div class="v2-files-dir-header">${escapeHtml(dir)}/</div>`
            : '';
          return `<div class="v2-files-dir-group">${dirHeader}${rows}</div>`;
        });
        const meta = [
          repo.branch ? escapeHtml(repo.branch) : '',
          repo.remote ? escapeHtml(repo.remote) : '',
        ].filter(Boolean).join(' · ');
        const count = repo.entries?.length || 0;
        return `
          <div class="v2-files-repo ${collapsed ? 'collapsed' : ''}" data-repo="${escapeHtml(repoPath)}">
            <button class="v2-files-repo-header" type="button" data-repo-toggle="${escapeHtml(repoPath)}" aria-expanded="${collapsed ? 'false' : 'true'}">
              <span class="v2-files-repo-chev ${collapsed ? '' : 'expanded'}" aria-hidden="true">▸</span>
              <span class="v2-files-repo-title">${escapeHtml(repoPath)}</span>
              <span class="v2-files-repo-meta">${meta}</span>
              <span class="v2-files-repo-count">${count}</span>
            </button>
            ${collapsed ? '' : `
              ${this._renderRepoRevisionControls(repoPath, comparison, commits, commitsResult?.error)}
              ${repo.error ? `<div class="v2-files-repo-error">${escapeHtml(repo.error)}</div>` : ''}
              ${dirChunks.length ? dirChunks.join('') : '<div class="v2-files-empty">No changes for this comparison.</div>'}
            `}
          </div>
        `;
      });
      this.treeEl.innerHTML = repoParts.join('');
      return;
    }

    // All files → render tree rooted at ''
    this.treeEl.innerHTML = this._renderTreeLevel('', 0);
  }

  _renderRepoRevisionControls(repoPath, comparison, commits, error) {
    const newerOptions = [
      { value: 'worktree', label: 'Unstaged' },
      { value: 'HEAD', label: 'HEAD' },
      ...commits.map(c => ({ value: c.sha, label: `${c.short_sha || c.sha.slice(0, 7)} ${c.subject || ''}`.trim() })),
    ];
    const olderOptions = this._olderRefOptions(comparison.newerRef, commits);
    const renderOptions = (options, selected) => options.map(o => `
      <option value="${escapeHtml(o.value)}" ${o.value === selected ? 'selected' : ''}>${escapeHtml(o.label)}</option>
    `).join('');
    return `
      <div class="v2-files-rev-controls" data-repo-revisions="${escapeHtml(repoPath)}">
        <label class="v2-files-rev-field">
          <span>From</span>
          <select data-rev-menu="newer" data-repo-path="${escapeHtml(repoPath)}">
            ${renderOptions(newerOptions, comparison.newerRef)}
          </select>
        </label>
        <label class="v2-files-rev-field">
          <span>To</span>
          <select data-rev-menu="older" data-repo-path="${escapeHtml(repoPath)}">
            ${renderOptions(olderOptions, comparison.olderRef)}
          </select>
        </label>
        ${error ? `<div class="v2-files-rev-error">${escapeHtml(error)}</div>` : ''}
      </div>
    `;
  }

  _olderRefOptions(newerRef, commits) {
    const commitOptions = commits.map(c => ({
      value: c.sha,
      label: `${c.short_sha || c.sha.slice(0, 7)} ${c.subject || ''}`.trim(),
    }));
    if (newerRef === 'worktree') return [{ value: 'HEAD', label: 'HEAD' }, ...commitOptions];
    if (newerRef === 'HEAD') return commitOptions.slice(1);
    const newerIndex = commits.findIndex(c => c.sha === newerRef);
    return newerIndex >= 0 ? commitOptions.slice(newerIndex + 1) : commitOptions;
  }

  _renderTreeLevel(path, depth) {
    const tree = filesStore.treeFor(this.channel.id);
    const slot = tree.get(path);
    if (!slot) return '<div class="v2-files-empty">Loading…</div>';
    if (!slot.entries?.length) return '<div class="v2-files-empty">(empty)</div>';

    // Nested-wrapper approach: each sub-level sits inside a
    // `.v2-files-level` div that adds 16px padding-left + a faint
    // left-border guide line. Indent + hierarchy come from the DOM
    // structure, so dirs and files line up identically.
    const expandedSet = this._expandedDirsSet();
    const parts = [];
    for (const entry of slot.entries) {
      const fullPath = path ? `${path}/${entry.name}` : entry.name;
      if (entry.type === 'dir' || entry.type === 'directory') {
        const expanded = expandedSet.has(fullPath);
        parts.push(`
          <button class="v2-files-row v2-files-dir"
                  type="button"
                  data-dir-path="${escapeHtml(fullPath)}">
            <span class="v2-files-chev ${expanded ? 'expanded' : ''}" aria-hidden="true">▸</span>
            <span class="v2-files-name">${escapeHtml(entry.name)}/</span>
            <span class="v2-files-dir-upload"
                  role="button" tabindex="0"
                  data-dir-upload="${escapeHtml(fullPath)}"
                  title="Upload into ${escapeHtml(entry.name)}/"
                  aria-label="Upload into ${escapeHtml(entry.name)}/">
              <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <path d="M8 10V2M5 5l3-3 3 3M3 10v3a1 1 0 001 1h8a1 1 0 001-1v-3"
                      stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            </span>
          </button>
          ${expanded ? `<div class="v2-files-level">${this._renderTreeLevel(fullPath, depth + 1)}</div>` : ''}
        `);
      } else {
        const selected = this.channel.viewState.filesPath === fullPath;
        parts.push(`
          <button class="v2-files-row v2-files-file ${selected ? 'active' : ''}"
                  type="button"
                  data-file-path="${escapeHtml(fullPath)}">
            <span class="v2-files-chev v2-files-chev-spacer" aria-hidden="true"></span>
            <span class="v2-files-name">${escapeHtml(entry.name)}</span>
          </button>
        `);
      }
    }
    if (slot.truncated) parts.push('<div class="v2-files-trunc">… truncated</div>');
    return parts.join('');
  }

  // ----- Viewer -----

  _renderViewer() {
    if (!this.viewerEl || !this.modeBarEl) return;
    const path = this.channel.viewState.filesPath;
    let mode = this.channel.viewState.filesView;
    const previewable = isHtmlPreviewable(path);
    if (mode === 'preview' && !previewable) {
      mode = 'source';
      this.channel.viewState.filesView = mode;
    }

    // Mode bar only appears when a file is selected (v1 parity).
    if (!path) {
      this.modeBarEl.innerHTML = '';
      this.modeBarEl.hidden = true;
    } else {
      this.modeBarEl.hidden = false;
      const wrapActive = this.channel.viewState.filesLineWrap;
      this.modeBarEl.innerHTML = `
        <div class="v2-files-mode-tabs">
          ${['source', 'diff', 'preview'].map(m => {
            const disabled = m === 'preview' && !previewable;
            const title = disabled ? 'HTML preview is available for .html and .htm files' : '';
            return `<button class="v2-files-mode-tab ${m === mode ? 'active' : ''}" type="button" data-mode="${m}" ${disabled ? 'disabled' : ''} ${title ? `title="${title}"` : ''}>${MODE_LABELS[m]}</button>`;
          }).join('')}
          <button class="v2-files-mode-tab v2-files-wrap-btn ${wrapActive ? 'active' : ''}"
                  type="button"
                  data-toggle="wrap"
                  aria-pressed="${wrapActive ? 'true' : 'false'}"
                  title="Toggle line wrapping">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M3 4h10M3 8h7a2 2 0 010 4H8l1.5-1.5M3 12h3"/>
            </svg>
            Wrap
          </button>
        </div>
      `;
    }

    // Path bar at top of viewer echoes the selected path.
    const pathText = document.getElementById('v2-path-text');
    if (pathText) {
      pathText.textContent = path || 'No file selected';
      pathText.classList.toggle('empty', !path);
    }

    if (!path) {
      this.viewerEl.innerHTML = `
        <div class="v2-files-placeholder">
          <svg class="v2-files-placeholder-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M6 3h8l4 4v14H6z"/>
            <path d="M14 3v4h4"/>
            <path d="M9 13h6M9 17h4"/>
          </svg>
          <div class="v2-files-placeholder-title">No file selected</div>
          <div class="v2-files-placeholder-body">Pick a file from the tree on the left to view its source, diff, or preview.</div>
        </div>
      `;
      return;
    }
    if (mode === 'diff') {
      this._renderDiffResult();
    } else if (mode === 'preview') {
      this._renderHtmlPreview();
    } else {
      this._renderReadResult();
    }
  }

  _renderHtmlPreview() {
    if (!this.viewerEl) return;
    const d = filesStore.readResultFor(this.channel.id);
    if (!d || d.path !== this.channel.viewState.filesPath) {
      this.viewerEl.innerHTML = '<div class="v2-files-empty">Loading preview…</div>';
      return;
    }
    if (d.error) {
      this.viewerEl.innerHTML = `<div class="v2-files-empty">${escapeHtml(d.error)}</div>`;
      return;
    }
    if (d.is_binary) {
      this.viewerEl.innerHTML = `<div class="v2-files-empty">Binary file (${escapeHtml(formatBytes(d.size || 0))})</div>`;
      return;
    }
    if (!isHtmlPreviewable(d.path || '')) {
      this.viewerEl.innerHTML = '<div class="v2-files-empty">Preview is available for HTML files.</div>';
      return;
    }

    const host = document.createElement('div');
    host.className = 'v2-files-html-preview';
    if (d.truncated) {
      const note = document.createElement('div');
      note.className = 'v2-files-html-note';
      note.textContent = `Previewing the first ${formatBytes((d.content || '').length)} of ${formatBytes(d.size || 0)}.`;
      host.appendChild(note);
    }
    const iframe = document.createElement('iframe');
    iframe.className = 'v2-files-html-frame';
    iframe.title = `${d.path || 'HTML'} preview`;
    iframe.setAttribute('sandbox', 'allow-scripts allow-forms allow-popups allow-modals');
    iframe.setAttribute('referrerpolicy', 'no-referrer');
    iframe.addEventListener('load', () => {
      iframe.contentWindow?.postMessage({
        type: '__build_preview',
        html: d.content || '',
      }, '*');
    }, { once: true });
    iframe.src = '/preview-frame';
    host.appendChild(iframe);

    this.viewerEl.innerHTML = '';
    this.viewerEl.appendChild(host);
  }

  _renderReadResult() {
    if (!this.viewerEl) return;
    const d = filesStore.readResultFor(this.channel.id);
    if (!d || d.path !== this.channel.viewState.filesPath) {
      this.viewerEl.innerHTML = '<div class="v2-files-empty">Loading…</div>';
      return;
    }
    if (d._progress) {
      this.viewerEl.innerHTML = `<div class="v2-files-empty">Loading image… (${d.chunk_received}/${d.chunk_total})</div>`;
      return;
    }
    if (d.error) {
      this.viewerEl.innerHTML = `<div class="v2-files-empty">${escapeHtml(d.error)}</div>`;
      return;
    }
    if (d.is_binary) {
      this.viewerEl.innerHTML = `<div class="v2-files-empty">Binary file (${escapeHtml(formatBytes(d.size || 0))})</div>`;
      return;
    }
    const ext = extOf(d.path || '');
    if (d.is_image && d.content) {
      this.viewerEl.innerHTML = `<div class="v2-files-image"><img src="${d.content}" alt="${escapeHtml(d.path || '')}"></div>`;
      return;
    }
    if (ext === 'svg' && d.content) {
      const host = document.createElement('div');
      host.className = 'v2-files-svg-host';
      host.innerHTML = d.content;
      this.viewerEl.innerHTML = '';
      this.viewerEl.appendChild(host);
      return;
    }
    if (ext === 'md' || ext === 'markdown') {
      this.viewerEl.innerHTML = `<div class="v2-files-markdown">${renderMarkdown(d.content || '')}</div>`;
      return;
    }
    // Source view
    const wrap = this.channel.viewState.filesLineWrap;
    const lines = (d.content || '').split('\n');
    const rows = lines.map((line, i) => `
      <div class="v2-src-line">
        <span class="v2-src-num">${i + 1}</span>
        <span class="v2-src-text">${highlightLine(line, ext) || '&nbsp;'}</span>
      </div>
    `).join('');
    this.viewerEl.innerHTML = `<div class="v2-src ${wrap ? 'wrap' : ''}">${rows}</div>`;
    if (d.truncated) {
      const note = document.createElement('div');
      note.className = 'v2-files-trunc-note';
      note.textContent = `… truncated (${formatBytes(d.size || 0)})`;
      this.viewerEl.appendChild(note);
    }
  }

  _renderDiffResult() {
    if (!this.viewerEl) return;
    const d = filesStore.diffResultFor(this.channel.id);
    if (!d || d.path !== this.channel.viewState.filesPath) {
      this.viewerEl.innerHTML = '<div class="v2-files-empty">Loading diff…</div>';
      return;
    }
    const repo = this._repoForPath(d.path);
    if (repo?.path && this.channel.viewState.filesTreeTab === 'changes') {
      const comparison = this._repoComparison(repo.path);
      if ((d.newer_ref && d.newer_ref !== comparison.newerRef) ||
          (d.older_ref && d.older_ref !== comparison.olderRef)) {
        this.viewerEl.innerHTML = '<div class="v2-files-empty">Loading diff…</div>';
        return;
      }
    }
    if (!d.diff) {
      this.viewerEl.innerHTML = '<div class="v2-files-empty">No changes.</div>';
      return;
    }
    const ext = extOf(d.path || '');
    const wrap = this.channel.viewState.filesLineWrap;
    const diffEl = renderDiff(d.diff, ext);
    diffEl.classList.toggle('wrap', !!wrap);
    this.viewerEl.innerHTML = '';
    this.viewerEl.appendChild(diffEl);
    if (d.truncated) {
      const note = document.createElement('div');
      note.className = 'v2-files-trunc-note';
      note.textContent = '… diff truncated';
      this.viewerEl.appendChild(note);
    }
  }

  // ----- Click routing -----

  _onClick = (e) => {
    // Upload affordances first — checked before the dir-expand handler
    // because the per-dir upload icon sits inside the dir button.
    const uploadTrigger = e.target.closest('[data-dir-upload]');
    if (uploadTrigger) {
      e.stopPropagation();
      e.preventDefault();
      const destDir = uploadTrigger.getAttribute('data-dir-upload') || '';
      this._openFilePicker(destDir);
      return;
    }
    const treeTab = e.target.closest('[data-tree-tab]');
    if (treeTab) {
      this.channel.viewState.filesTreeTab = treeTab.getAttribute('data-tree-tab');
      this._renderTree();
      return;
    }
    const dirBtn = e.target.closest('[data-dir-path]');
    if (dirBtn) {
      const path = dirBtn.getAttribute('data-dir-path');
      const expanded = this._expandedDirsSet();
      if (expanded.has(path)) {
        expanded.delete(path);
        this._commitExpandedDirs(expanded);
        this._renderTree();
      } else {
        expanded.add(path);
        this._commitExpandedDirs(expanded);
        // Fetch fresh entries if we don't have them cached; the store
        // update will trigger _renderTree. Render now too so the chevron
        // flips to "open" immediately.
        const tree = filesStore.treeFor(this.channel.id);
        if (!tree.has(path)) bus.emit('intent.files_list', { channelId: this.channel.id, path });
        this._renderTree();
      }
      return;
    }
    const fileBtn = e.target.closest('[data-file-path]');
    if (fileBtn) {
      const path = fileBtn.getAttribute('data-file-path');
      this._selectFile(path, fileBtn.getAttribute('data-has-diff') === '1', fileBtn.getAttribute('data-repo-path'));
      return;
    }
    const repoToggle = e.target.closest('[data-repo-toggle]');
    if (repoToggle) {
      const repoPath = repoToggle.getAttribute('data-repo-toggle') || '.';
      const collapsed = this._collapsedReposSet();
      if (collapsed.has(repoPath)) collapsed.delete(repoPath);
      else collapsed.add(repoPath);
      this._commitCollapsedRepos(collapsed);
      this._renderTree();
      return;
    }
    const modeBtn = e.target.closest('[data-mode]');
    if (modeBtn && !modeBtn.disabled) {
      this.channel.viewState.filesView = modeBtn.getAttribute('data-mode');
      if (this.channel.viewState.filesView === 'diff' && this.channel.viewState.filesPath) {
        this._emitFileDiff(this.channel.viewState.filesPath);
      } else if (this.channel.viewState.filesView === 'preview' && this.channel.viewState.filesPath) {
        const rr = filesStore.readResultFor(this.channel.id);
        if (!rr || rr.path !== this.channel.viewState.filesPath) {
          bus.emit('intent.file_read', { channelId: this.channel.id, path: this.channel.viewState.filesPath });
        }
      }
      this._renderViewer();
      return;
    }
    const wrapBtn = e.target.closest('[data-toggle="wrap"]');
    if (wrapBtn) {
      const next = !this.channel.viewState.filesLineWrap;
      this.channel.viewState.filesLineWrap = next;
      this._renderViewer();
      return;
    }
  };

  _onChange = (e) => {
    const menu = e.target.closest?.('[data-rev-menu]');
    if (!menu) return;
    const repoPath = menu.getAttribute('data-repo-path') || '.';
    const kind = menu.getAttribute('data-rev-menu');
    const patch = kind === 'newer' ? { newerRef: menu.value } : { olderRef: menu.value };
    const comparison = this._commitRepoComparison(repoPath, patch);
    const commits = filesStore.commitsFor(this.channel.id, repoPath)?.commits || [];
    const olderOptions = this._olderRefOptions(comparison.newerRef, commits);
    if (!olderOptions.some(o => o.value === comparison.olderRef) && olderOptions[0]) {
      comparison.olderRef = olderOptions[0].value;
    }
    this._emitRevisionChanges(repoPath);
    const selectedRepo = this._repoForPath(this.channel.viewState.filesPath);
    if (selectedRepo?.path === repoPath && this.channel.viewState.filesView === 'diff') {
      this._emitFileDiff(this.channel.viewState.filesPath, repoPath);
    }
    this._renderTree();
  };

  // ── Upload ─────────────────────────────────────────────────────────

  /** Open the native file picker, stamping the destination dir onto
   *  the hidden input so `_onFileInputChange` knows where the files
   *  should land. */
  _openFilePicker(destDir) {
    if (!this.fileInputEl) return;
    this.fileInputEl.dataset.destDir = destDir || '';
    this.fileInputEl.value = '';   // allow picking the same file twice
    this.fileInputEl.click();
  }

  _onFileInputChange = (e) => {
    const destDir = this.fileInputEl?.dataset.destDir || '';
    const files = [...(e.target.files || [])];
    this.fileInputEl.value = '';
    for (const f of files) this._dispatchUpload(f, destDir);
  };

  /** Actually start the upload. Overridable in tests via view hookup —
   *  Playwright spies on this method to assert the (file, destDir)
   *  arguments without having to plumb through the real bridge. */
  async _dispatchUpload(file, destDir) {
    const channelId = this.channel?.id;
    if (!channelId || !file) return;
    try {
      await uploadFile(channelId, file, destDir || '');
    } catch (err) {
      // Always log the full error so devtools keeps a durable record
      // even when the toast fades.
      console.error('[files-view] upload failed', { file: file.name, destDir, err });
      const msg = String(err?.message || err || '');
      const prefix = destDir ? `${destDir}/` : '';
      if (/file exists/i.test(msg)) {
        showToast(`${prefix}${file.name} already exists — rename and try again`, { kind: 'error' });
      } else if (msg) {
        showToast(`Upload of ${prefix}${file.name} failed: ${msg}`, { kind: 'error' });
      }
    } finally {
      // Whether the upload succeeded or failed, the tree + modified
      // list are the fastest way for the user to see the current
      // state of the workspace.
      this._scheduleRefresh([]);
    }
  }

  // Drag-and-drop
  _onTreeDragEnter = (e) => {
    if (!this._dragHasFiles(e)) return;
    e.preventDefault();
    this._treeDragDepth++;
    this.treePanelEl?.classList.add('drag-active');
    this._highlightDropTarget(e.target);
  };

  _onTreeDragOver = (e) => {
    if (!this._dragHasFiles(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
    this._highlightDropTarget(e.target);
  };

  _onTreeDragLeave = (e) => {
    if (!this._dragHasFiles(e)) return;
    this._treeDragDepth = Math.max(0, this._treeDragDepth - 1);
    if (this._treeDragDepth === 0) {
      this.treePanelEl?.classList.remove('drag-active');
      this._clearDropTargetHighlight();
    }
  };

  _onTreeDrop = (e) => {
    if (!this._dragHasFiles(e)) return;
    e.preventDefault();
    this._treeDragDepth = 0;
    this.treePanelEl?.classList.remove('drag-active');
    this._clearDropTargetHighlight();
    const dirBtn = e.target.closest('[data-dir-path]');
    const destDir = dirBtn ? dirBtn.getAttribute('data-dir-path') : '';
    const files = [...(e.dataTransfer?.files || [])];
    for (const f of files) this._dispatchUpload(f, destDir);
  };

  _dragHasFiles(e) {
    // `dataTransfer.types` is the only reliable signal during dragover
    // in modern browsers — `files` is empty until drop.
    const types = e.dataTransfer?.types;
    if (!types) return false;
    return Array.from(types).includes('Files');
  }

  _highlightDropTarget(target) {
    const row = target?.closest?.('[data-dir-path]');
    if (this._dropTarget === row) return;
    this._clearDropTargetHighlight();
    if (row) row.classList.add('drop-target');
    this._dropTarget = row || null;
  }

  _clearDropTargetHighlight() {
    if (this._dropTarget) this._dropTarget.classList.remove('drop-target');
    this._dropTarget = null;
  }

  // Upload progress chips — one per in-flight file.
  _onUploadProgress = ({ file_id, filename, progress }) => {
    if (!this.uploadChipsEl) return;
    let chip = this._activeUploads.get(file_id);
    if (!chip) {
      chip = document.createElement('div');
      chip.className = 'v2-files-upload-chip';
      chip.dataset.fileId = file_id;
      chip.innerHTML = `
        <span class="v2-files-upload-name"></span>
        <span class="v2-files-upload-pct"></span>
      `;
      this.uploadChipsEl.appendChild(chip);
      this._activeUploads.set(file_id, chip);
      this.uploadChipsEl.hidden = false;
    }
    chip.querySelector('.v2-files-upload-name').textContent = filename || '…';
    chip.querySelector('.v2-files-upload-pct').textContent =
      `${Math.min(100, Math.round((progress || 0) * 100))}%`;
  };

  _onUploadDone = ({ file_id }) => {
    const chip = this._activeUploads.get(file_id);
    if (chip) {
      chip.remove();
      this._activeUploads.delete(file_id);
    }
    if (this.uploadChipsEl && this._activeUploads.size === 0) {
      this.uploadChipsEl.hidden = true;
    }
    // New file just landed on the device — refresh so it appears.
    this._scheduleRefresh([]);
  };

  _expandedDirsSet() {
    const list = this.channel.viewState.filesExpandedDirs;
    return new Set(Array.isArray(list) ? list : []);
  }

  _collapsedReposSet() {
    const list = this.channel.viewState.filesCollapsedRepos;
    return new Set(Array.isArray(list) ? list : []);
  }

  _commitCollapsedRepos(set) {
    this.channel.viewState.filesCollapsedRepos = [...set];
  }

  _commitExpandedDirs(set) {
    this.channel.viewState.filesExpandedDirs = [...set];
  }

  _repoComparison(repoPath) {
    if (!this.channel.viewState.filesRepoComparisons || typeof this.channel.viewState.filesRepoComparisons !== 'object') {
      this.channel.viewState.filesRepoComparisons = {};
    }
    const key = repoPath || '.';
    const existing = this.channel.viewState.filesRepoComparisons[key] || {};
    const comparison = {
      newerRef: existing.newerRef || 'worktree',
      olderRef: existing.olderRef || 'HEAD',
    };
    this.channel.viewState.filesRepoComparisons[key] = comparison;
    return comparison;
  }

  _commitRepoComparison(repoPath, patch) {
    const key = repoPath || '.';
    const current = this._repoComparison(key);
    this.channel.viewState.filesRepoComparisons[key] = { ...current, ...patch };
    return this.channel.viewState.filesRepoComparisons[key];
  }

  _ensureRepoCommits(repoPath) {
    const key = repoPath || '.';
    if (this._requestedCommitRepos.has(key)) return;
    this._requestedCommitRepos.add(key);
    bus.emit('intent.files_commits', { channelId: this.channel.id, repoPath: key, limit: 25 });
  }

  _repoForPath(path) {
    const repos = filesStore.changesFor(this.channel.id) || [];
    return repos.find(repo => (repo.entries || []).some(e => e.path === path)) || null;
  }

  _emitRevisionChanges(repoPath) {
    const comparison = this._repoComparison(repoPath);
    bus.emit('intent.files_revision_changes', {
      channelId: this.channel.id,
      repoPath,
      newerRef: comparison.newerRef,
      olderRef: comparison.olderRef,
    });
  }

  _emitFileDiff(path, repoPath = null) {
    const repo = repoPath ? { path: repoPath } : this._repoForPath(path);
    if (repo?.path && this.channel.viewState.filesTreeTab === 'changes') {
      const comparison = this._repoComparison(repo.path);
      bus.emit('intent.file_diff', {
        channelId: this.channel.id,
        path,
        repoPath: repo.path,
        newerRef: comparison.newerRef,
        olderRef: comparison.olderRef,
      });
      return;
    }
    bus.emit('intent.file_diff', { channelId: this.channel.id, path });
  }

  _selectFile(path, hasDiff, repoPath = null) {
    this.channel.viewState.filesPath = path;
    // Default to diff if there's a pending change, otherwise source.
    const nextMode = hasDiff && this.channel.viewState.filesTreeTab === 'changes' ? 'diff' : 'source';
    this.channel.viewState.filesView = nextMode;
    if (nextMode === 'diff') {
      this._emitFileDiff(path, repoPath);
    } else {
      bus.emit('intent.file_read', { channelId: this.channel.id, path });
    }
    this._renderTree();
    this._renderViewer();
  }
}

function extOf(path) {
  const m = /\.([^./]+)$/.exec(path || '');
  return m ? m[1].toLowerCase() : '';
}

export function isHtmlPreviewable(path) {
  const ext = extOf(path);
  return ext === 'html' || ext === 'htm';
}
