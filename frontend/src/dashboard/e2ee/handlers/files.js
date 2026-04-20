import { state } from '../../state.js';
import { escapeHtml, formatBytes } from '../../util/html.js';
import { fileContentBody } from '../../files/refs.js';
import { renderFileContent } from '../../files/content.js';
import { renderDiffContent } from '../../files/diff.js';
import { renderFileTree, selectFile } from '../../files/tree.js';
import { updateFilesModifiedCount } from '../../files/mode.js';

export function bindFileHandlers(instance, deviceId) {
  instance.addEventListener('files_list_result', (evt) => {
    const { channel_id, path, entries, error, truncated } = evt.detail;
    if (error) { console.warn('files_list error:', error); return; }
    if (channel_id !== state.filesChannelId) return;

    if (!state.fileTreeData.has(channel_id)) state.fileTreeData.set(channel_id, new Map());
    const data = state.fileTreeData.get(channel_id);
    data.set(path || '', { entries: entries || [], truncated: !!truncated });
    renderFileTree();

    // Restore saved file selection after root listing loads.
    if (state.filesPendingRestore && (path || '') === '') {
      const pendingPath = state.filesPendingRestore;
      const pendingView = state.filesPendingView;
      state.filesPendingRestore = null;
      state.filesPendingView = 'source';
      // Find the entry in the root listing.
      const match = (entries || []).find(e => e.name === pendingPath || e.path === pendingPath);
      if (match && match.type !== 'directory') {
        selectFile(match.path || match.name, match, pendingView);
      } else if (pendingPath.includes('/')) {
        // Nested path — select directly (tree won't highlight but file loads).
        selectFile(pendingPath, null, pendingView);
      }
    }
  });

  instance.addEventListener('files_changes_result', (evt) => {
    const { channel_id, repos } = evt.detail;
    console.debug('[files] files_changes_result', { channel_id, repos_count: repos?.length, filesChannelId: state.filesChannelId, chatCurrentChannel: state.chatCurrentChannel });
    // Accept if it matches state.filesChannelId, OR if state.filesChannelId is unset
    // but this is the currently-active chat channel (self-heal race).
    if (state.filesChannelId) {
      if (channel_id !== state.filesChannelId) return;
    } else if (channel_id !== state.chatCurrentChannel) {
      return;
    } else {
      state.filesChannelId = channel_id;
    }
    state.filesChangesData.set(channel_id, repos || []);
    updateFilesModifiedCount();
    if (state.filesTreeTab === 'changes') renderFileTree();
  });

  let _imageChunks = {};  // path -> { chunks: [], total: N }

  instance.addEventListener('file_read_result', (evt) => {
    const d = evt.detail;
    if (d.channel_id !== state.filesChannelId || d.path !== state.filesCurrentPath) return;
    if (state.filesCurrentView === 'diff') return;

    if (d.error) {
      fileContentBody.innerHTML = `<div class="empty-state"><p>${escapeHtml(d.error)}</p></div>`;
      return;
    }
    if (d.is_image && d.content) {
      // Handle chunked images.
      if (d.chunk_total && d.chunk_total > 1) {
        if (!_imageChunks[d.path] || _imageChunks[d.path].total !== d.chunk_total) {
          _imageChunks[d.path] = { chunks: new Array(d.chunk_total), total: d.chunk_total };
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading image... (0/' + d.chunk_total + ')</p></div>';
        }
        const chunkRec = _imageChunks[d.path];
        chunkRec.chunks[d.chunk_index] = d.content;
        const received = chunkRec.chunks.filter(Boolean).length;
        if (received < chunkRec.total) {
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading image... (' + received + '/' + chunkRec.total + ')</p></div>';
          return;
        }
        // All chunks received — reassemble.
        const fullDataUri = chunkRec.chunks.join('');
        delete _imageChunks[d.path];
        fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + fullDataUri + '" alt="' + escapeHtml(d.path) + '"></div>';
        return;
      }
      fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + d.content + '" alt="' + escapeHtml(d.path) + '"></div>';
      return;
    }
    if (d.is_binary) {
      fileContentBody.innerHTML = '<div class="empty-state"><p>Binary file (' + formatBytes(d.size) + ')</p></div>';
      return;
    }
    renderFileContent(d.content, d.path, d.size, d.truncated);
  });

  instance.addEventListener('file_diff_result', (evt) => {
    const d = evt.detail;
    if (d.channel_id !== state.filesChannelId || d.path !== state.filesCurrentPath) return;
    if (state.filesCurrentView !== 'diff') return;

    if (!d.diff) {
      fileContentBody.innerHTML = '<div class="empty-state"><p>No changes</p></div>';
      return;
    }
    renderDiffContent(d.diff, d.truncated);
  });
}
