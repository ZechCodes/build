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
    this.root = document.getElementById('v2-tab-files');
    if (!this.root) return;
    this._buildShell();
    this._fetchInitial();
    this.unsubs.push(filesStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      if (e.kind === 'tree' || e.kind === 'changes') this._renderTree();
      if (e.kind === 'read_result' || e.kind === 'read_result_progress') this._renderReadResult();
      if (e.kind === 'diff_result') this._renderDiffResult();
    }));
    this._renderTree();
    this._renderViewer();
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.root = null;
    this.treeEl = null;
    this.viewerEl = null;
    this.modeBarEl = null;
  }

  _fetchInitial() {
    bus.emit('intent.files_list', { channelId: this.channel.id, path: '' });
    bus.emit('intent.files_changes', { channelId: this.channel.id });
  }

  _buildShell() {
    this.root.innerHTML = `
      <div class="v2-files">
        <aside class="v2-files-tree">
          <header class="v2-files-tree-tabs">
            <button class="v2-files-tree-tab" data-tree-tab="changes" type="button">Changes</button>
            <button class="v2-files-tree-tab" data-tree-tab="all" type="button">All files</button>
          </header>
          <div class="v2-files-tree-body" data-slot="tree"></div>
        </aside>
        <section class="v2-files-viewer">
          <header class="v2-files-mode-bar" data-slot="mode"></header>
          <div class="v2-files-viewer-body" data-slot="viewer"></div>
        </section>
      </div>
    `;
    this.treeEl = this.root.querySelector('[data-slot="tree"]');
    this.viewerEl = this.root.querySelector('[data-slot="viewer"]');
    this.modeBarEl = this.root.querySelector('[data-slot="mode"]');

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

    if (tab === 'changes') {
      const repos = filesStore.changesFor(this.channel.id);
      if (!repos?.length) {
        this.treeEl.innerHTML = '<div class="v2-files-empty">No changes.</div>';
        return;
      }
      const parts = repos.map(repo => {
        const rows = (repo.changes || []).map(c => `
          <button class="v2-files-row ${this.channel.viewState.filesPath === c.path ? 'active' : ''}"
                  type="button"
                  data-file-path="${escapeHtml(c.path)}"
                  data-has-diff="${c.has_diff ? '1' : '0'}">
            <span class="v2-files-status-${escapeHtml(c.status || 'mod')}">${escapeHtml((c.status || 'm')[0].toUpperCase())}</span>
            <span class="v2-files-name">${escapeHtml(c.path)}</span>
          </button>
        `).join('');
        return `
          <div class="v2-files-repo">
            <div class="v2-files-repo-path">${escapeHtml(repo.path || '.')}</div>
            ${rows}
          </div>
        `;
      });
      this.treeEl.innerHTML = parts.join('');
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
      if (entry.type === 'directory') {
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

    // Mode bar
    const pathLabel = path ? escapeHtml(path) : '(no file selected)';
    this.modeBarEl.innerHTML = `
      <span class="v2-files-path">${pathLabel}</span>
      <div class="v2-files-mode-tabs">
        ${['source', 'diff', 'preview'].map(m => {
          const disabled = m === 'preview';
          return `<button class="v2-files-mode-tab ${m === mode ? 'active' : ''}" type="button" data-mode="${m}" ${disabled ? 'disabled' : ''}>${MODE_LABELS[m]}</button>`;
        }).join('')}
        <label class="v2-files-wrap">
          <input type="checkbox" data-toggle="wrap" ${this.channel.viewState.filesLineWrap ? 'checked' : ''}>
          wrap
        </label>
      </div>
      <div class="v2-files-review">
        <button class="v2-files-review-btn" type="button" data-review="approve-all">Approve all</button>
        <button class="v2-files-review-btn" type="button" data-review="view-pr">View PR</button>
      </div>
    `;

    if (!path) {
      this.viewerEl.innerHTML = '<div class="v2-files-empty">Select a file from the tree.</div>';
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
    const wrapToggle = e.target.closest('[data-toggle="wrap"]');
    if (wrapToggle) {
      this.channel.viewState.filesLineWrap = wrapToggle.checked;
      this._renderViewer();
      return;
    }
    const reviewBtn = e.target.closest('[data-review]');
    if (reviewBtn) {
      // v1 parity: no backend for review flow yet; toast stub.
      showToast('Review flow not wired up yet');
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
