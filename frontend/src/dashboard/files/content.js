import { state } from '../state.js';
import { escapeHtml, formatBytes } from '../util/html.js';
import { fileContentBody } from './refs.js';
import { highlightLine } from './syntax.js';
import { renderMarkdown } from '../vendor/markdown.js';
import { getE2EE } from '../e2ee/bridge.js';
import { renderHtmlPreview } from './html-preview.js';

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

// Promise-based file read for fetching assets without conflicting with main file viewer.
export function readFileAsync(channelId, path) {
  return new Promise((resolve, reject) => {
    const conn = getE2EE(channelId);
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
