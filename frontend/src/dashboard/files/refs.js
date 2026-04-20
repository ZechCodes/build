// Shared DOM refs for the Files tab. All sub-modules import from here so the
// document.getElementById lookups happen once at module load.

export const fileTree = document.getElementById('file-tree');
export const fileTreeInner = document.getElementById('file-tree-inner');
export const fileTreeEmpty = document.getElementById('file-tree-empty');
export const filesPathText = document.getElementById('files-path-text');
export const filesPathBar = document.getElementById('files-path-bar');
export const filesPathChevron = document.getElementById('files-path-chevron');
export const fileReloadBtn = document.getElementById('file-reload-btn');
export const fileTreePanel = document.getElementById('file-tree-panel');
export const fileContentBody = document.getElementById('file-content-body');
export const fileFloatToggle = document.getElementById('file-float-toggle');
export const fileWrapToggle = document.getElementById('file-wrap-toggle');
