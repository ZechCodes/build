// FilesView — tree + viewer (source / diff). Mounts into #v2-tab-files.
// HTML preview + image preview ports arrive in Wave 6.
// See planning/dashboard-v2/05-views.md § FilesView.

import { bus } from '../../core/bus.js';
import { filesStore } from '../../domain/files-store.js';
import { escapeHtml, formatBytes } from '../../util/html.js';
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
  }

  activate() {
    this.root = document.getElementById('v2-viewer-main');
    if (!this.root) return;
    this._buildShell();
    this._fetchInitial();
    this.unsubs.push(filesStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      if (e.kind === 'tree' || e.kind === 'changes') this._renderTree();
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
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this._refreshTimer) { clearTimeout(this._refreshTimer); this._refreshTimer = null; }
    this._pendingPaths = null;
    this.root = null;
    this.treeEl = null;
    this.viewerEl = null;
    this.modeBarEl = null;
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
      bus.emit('intent.file_diff', { channelId, path: current });
    } else {
      bus.emit('intent.file_read', { channelId, path: current });
    }
  }

  _fetchInitial() {
    bus.emit('intent.files_list', { channelId: this.channel.id, path: '' });
    bus.emit('intent.files_changes', { channelId: this.channel.id });
  }

  _buildShell() {
    // Populate the two pre-built panels in the layout.
    const treePanel = document.getElementById('v2-files-tree-panel');
    const contentInner = document.getElementById('v2-files-content-inner');
    if (!treePanel || !contentInner) return;
    treePanel.innerHTML = `
      <header class="v2-files-tree-tabs">
        <button class="v2-files-tree-tab" data-tree-tab="changes" type="button">
          Modified <span class="v2-files-tree-tab-count"></span>
        </button>
        <button class="v2-files-tree-tab" data-tree-tab="all" type="button">All</button>
      </header>
      <div class="v2-files-tree-body" data-slot="tree"></div>
    `;
    contentInner.innerHTML = `
      <header class="v2-files-mode-bar" data-slot="mode"></header>
      <div class="v2-files-viewer-body" data-slot="viewer"></div>
    `;
    this.treeEl = treePanel.querySelector('[data-slot="tree"]');
    this.viewerEl = contentInner.querySelector('[data-slot="viewer"]');
    this.modeBarEl = contentInner.querySelector('[data-slot="mode"]');

    this.root.addEventListener('click', this._onClick);
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
      const nonEmpty = (repos || []).filter(r => (r.entries || []).length);
      if (!nonEmpty.length) {
        this.treeEl.innerHTML = '<div class="v2-files-empty">No changes.</div>';
        return;
      }
      // Auto-select the first changed file when nothing is selected yet
      // (e.g. the user just landed on a channel and the first change
      // just arrived). Keeps their selection sticky on subsequent
      // tree refreshes.
      if (!this.channel.viewState.filesPath) {
        const firstEntry = nonEmpty[0].entries.find(Boolean);
        if (firstEntry?.path) {
          // Defer so the tree finishes rendering before _selectFile's
          // own re-render runs.
          queueMicrotask(() => this._selectFile(firstEntry.path, true));
        }
      }
      const repoParts = nonEmpty.map(repo => {
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
        const label = repo.branch
          ? `${escapeHtml(repo.path || '.')} · ${escapeHtml(repo.branch)}`
          : escapeHtml(repo.path || '.');
        return `
          <div class="v2-files-repo">
            <div class="v2-files-repo-path">${label}</div>
            ${dirChunks.join('')}
          </div>
        `;
      });
      this.treeEl.innerHTML = repoParts.join('');
      return;
    }

    // All files → render tree rooted at ''
    this.treeEl.innerHTML = this._renderTreeLevel('', 0);
  }

  _renderTreeLevel(path, depth) {
    const tree = filesStore.treeFor(this.channel.id);
    const slot = tree.get(path);
    if (!slot) return '<div class="v2-files-empty">Loading…</div>';
    if (!slot.entries?.length) return `<div class="v2-files-empty" style="padding-left:${depth * 10}px">(empty)</div>`;

    const parts = [];
    for (const entry of slot.entries) {
      const fullPath = path ? `${path}/${entry.name}` : entry.name;
      if (entry.type === 'dir' || entry.type === 'directory') {
        const expanded = tree.has(fullPath);
        parts.push(`
          <button class="v2-files-row v2-files-dir"
                  type="button"
                  data-dir-path="${escapeHtml(fullPath)}"
                  style="padding-left:${depth * 12 + 8}px">
            <span class="v2-files-chev ${expanded ? 'expanded' : ''}">▸</span>
            <span class="v2-files-name">${escapeHtml(entry.name)}/</span>
          </button>
        `);
        if (expanded) parts.push(this._renderTreeLevel(fullPath, depth + 1));
      } else {
        parts.push(`
          <button class="v2-files-row ${this.channel.viewState.filesPath === fullPath ? 'active' : ''}"
                  type="button"
                  data-file-path="${escapeHtml(fullPath)}"
                  style="padding-left:${depth * 12 + 22}px">
            <span class="v2-files-name">${escapeHtml(entry.name)}</span>
          </button>
        `);
      }
    }
    if (slot.truncated) {
      parts.push(`<div class="v2-files-trunc" style="padding-left:${depth * 12 + 22}px">… truncated</div>`);
    }
    return parts.join('');
  }

  // ----- Viewer -----

  _renderViewer() {
    if (!this.viewerEl || !this.modeBarEl) return;
    const path = this.channel.viewState.filesPath;
    const mode = this.channel.viewState.filesView;

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
            const disabled = m === 'preview';
            return `<button class="v2-files-mode-tab ${m === mode ? 'active' : ''}" type="button" data-mode="${m}" ${disabled ? 'disabled' : ''}>${MODE_LABELS[m]}</button>`;
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
      this.viewerEl.innerHTML = '<div class="v2-files-empty">Select a file to view changes</div>';
      return;
    }
    if (mode === 'diff') {
      this._renderDiffResult();
    } else {
      this._renderReadResult();
    }
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
    if (!d.diff) {
      this.viewerEl.innerHTML = '<div class="v2-files-empty">No changes.</div>';
      return;
    }
    const ext = extOf(d.path || '');
    this.viewerEl.innerHTML = '';
    this.viewerEl.appendChild(renderDiff(d.diff, ext));
    if (d.truncated) {
      const note = document.createElement('div');
      note.className = 'v2-files-trunc-note';
      note.textContent = '… diff truncated';
      this.viewerEl.appendChild(note);
    }
  }

  // ----- Click routing -----

  _onClick = (e) => {
    const treeTab = e.target.closest('[data-tree-tab]');
    if (treeTab) {
      this.channel.viewState.filesTreeTab = treeTab.getAttribute('data-tree-tab');
      this._renderTree();
      return;
    }
    const dirBtn = e.target.closest('[data-dir-path]');
    if (dirBtn) {
      const path = dirBtn.getAttribute('data-dir-path');
      const tree = filesStore.treeFor(this.channel.id);
      if (tree.has(path)) {
        // Collapse: remove from tree cache.
        tree.delete(path);
        this._renderTree();
      } else {
        bus.emit('intent.files_list', { channelId: this.channel.id, path });
      }
      return;
    }
    const fileBtn = e.target.closest('[data-file-path]');
    if (fileBtn) {
      const path = fileBtn.getAttribute('data-file-path');
      this._selectFile(path, fileBtn.getAttribute('data-has-diff') === '1');
      return;
    }
    const modeBtn = e.target.closest('[data-mode]');
    if (modeBtn && !modeBtn.disabled) {
      this.channel.viewState.filesView = modeBtn.getAttribute('data-mode');
      if (this.channel.viewState.filesView === 'diff' && this.channel.viewState.filesPath) {
        bus.emit('intent.file_diff', { channelId: this.channel.id, path: this.channel.viewState.filesPath });
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

  _selectFile(path, hasDiff) {
    this.channel.viewState.filesPath = path;
    // Default to diff if there's a pending change, otherwise source.
    const nextMode = hasDiff && this.channel.viewState.filesTreeTab === 'changes' ? 'diff' : 'source';
    this.channel.viewState.filesView = nextMode;
    if (nextMode === 'diff') {
      bus.emit('intent.file_diff', { channelId: this.channel.id, path });
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
