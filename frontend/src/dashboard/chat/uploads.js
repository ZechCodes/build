import { state, MAX_FILE_SIZE } from '../state.js';
import { escapeHtml, formatFileSize } from '../util/html.js';

export function addPendingFiles(files) {
  for (const file of files) {
    if (file.size > MAX_FILE_SIZE) {
      console.warn(`File too large: ${file.name} (${formatFileSize(file.size)})`);
      continue;
    }
    state.pendingFiles.push(file);
  }
  renderPendingFiles();
}

export function removePendingFile(index) {
  state.pendingFiles.splice(index, 1);
  renderPendingFiles();
}

export function clearPendingFiles() {
  state.pendingFiles.length = 0;
  renderPendingFiles();
}

export function renderPendingFiles() {
  const staging = document.getElementById('upload-staging');
  if (!staging) return;
  if (!state.pendingFiles.length) {
    staging.innerHTML = '';
    staging.classList.remove('has-files');
    return;
  }
  staging.classList.add('has-files');
  staging.innerHTML = state.pendingFiles.map((f, i) =>
    `<div class="upload-pill">
      <span class="up-name">${escapeHtml(f.name)}</span>
      <span class="up-size">${formatFileSize(f.size)}</span>
      <span class="up-remove" data-index="${i}">&times;</span>
    </div>`
  ).join('');
  staging.querySelectorAll('.up-remove').forEach(btn => {
    btn.addEventListener('click', () => removePendingFile(parseInt(btn.dataset.index)));
  });
}

// File picker (attach button lives in chat/input.js's commands tray).
document.getElementById('chat-file-input')?.addEventListener('change', (e) => {
  if (e.target.files.length) {
    addPendingFiles(e.target.files);
    e.target.value = ''; // Reset so same file can be re-selected.
  }
});

// Upload progress: bound per-E2EE-instance from legacy.js's 'connected' handler.
export function _bindUploadProgress(client) {
  client.addEventListener('upload_progress', (evt) => {
    const { filename, progress, total_chunks, chunks_done } = evt.detail;
    const bar = document.getElementById('upload-progress');
    const fill = document.getElementById('upload-progress-fill');
    const label = document.getElementById('upload-progress-label');
    if (!bar) return;
    bar.classList.add('active');
    fill.style.width = (progress * 100) + '%';
    label.textContent = `Uploading ${filename}… ${chunks_done || 0}/${total_chunks}`;
    if (progress >= 1) {
      setTimeout(() => { bar.classList.remove('active'); }, 1500);
    }
  });
}

// Drag-and-drop + paste on the chat area.
const chatPanel = document.querySelector('.chat-main');
if (chatPanel) {
  let dragCounter = 0;
  const overlay = document.getElementById('chat-drop-overlay');

  chatPanel.addEventListener('dragenter', (e) => {
    e.preventDefault();
    dragCounter++;
    if (overlay) overlay.classList.add('visible');
  });

  chatPanel.addEventListener('dragleave', (e) => {
    e.preventDefault();
    dragCounter--;
    if (dragCounter <= 0) {
      dragCounter = 0;
      if (overlay) overlay.classList.remove('visible');
    }
  });

  chatPanel.addEventListener('dragover', (e) => {
    e.preventDefault();
  });

  chatPanel.addEventListener('drop', (e) => {
    e.preventDefault();
    dragCounter = 0;
    if (overlay) overlay.classList.remove('visible');
    if (e.dataTransfer?.files?.length) {
      addPendingFiles(e.dataTransfer.files);
    }
  });

  chatPanel.addEventListener('paste', (e) => {
    const files = e.clipboardData?.files;
    if (files && files.length) {
      e.preventDefault();
      addPendingFiles(files);
    }
  });
}
