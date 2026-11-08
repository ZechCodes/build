// Shared local folder/remote draft rules for the legacy sheet and the wizard.
export const inferredSourceName = value => (value.trim().replace(/[\\/]+$/, '').replace(/\.git$/i, '').split(/[\\/:]/).pop() || 'folder').replace(/[^a-zA-Z0-9._-]+/g, '-');
export function uniqueSourceName(value, sources, except) {
  const base = inferredSourceName(value);
  const used = new Set(sources.filter(source => source !== except).map(source => source.name.trim().toLowerCase()));
  let name = base;
  for (let n = 2; used.has(name.toLowerCase()); n++) name = `${base}-${n}`;
  return name;
}
export function sourceDraftError(source, names, { validateRemote = false } = {}) {
  const value = source[source.kind].trim();
  if (!value) return source.kind === 'remote' ? 'Enter a Git remote URL.' : 'Choose a folder.';
  if (validateRemote && source.kind === 'remote' && !validGitRemote(value)) return 'Enter a valid Git remote URL.';
  return folderLabelError(source.name.trim(), names);
}
function folderLabelError(name, names) {
  if (!name) return 'Enter a folder label.';
  if (/[\\/]/.test(name) || name === '.' || name === '..') return 'Use a single folder name without / or \\.';
  if (names.has(name.toLowerCase())) return `Folder labels must be unique. “${name}” is used more than once.`;
  names.add(name.toLowerCase());
  return '';
}
function validGitRemote(value) {
  if (/^[\w.-]+@[\w.-]+:.+/.test(value)) return !/\s/.test(value);
  try {
    const url = new URL(value);
    return ['https:', 'http:', 'ssh:', 'git:', 'file:'].includes(url.protocol) && Boolean(url.pathname.replace(/\//g, ''));
  } catch { return false; }
}
export function sourceDraftParams(source) {
  return { [source.kind]: source[source.kind].trim(), name: source.name.trim(), ...(source.base_branch.trim() ? { base_branch: source.base_branch.trim() } : {}) };
}
