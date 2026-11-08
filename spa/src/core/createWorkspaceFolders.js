import { esc } from './text.js';
import { fieldTraits } from './fieldTraits.js';
import { deviceCall } from './inboxDevices.js';
import { openBrowser } from '../sheets/browser.js';
import { uniqueSourceName, sourceDraftError } from './sourceDraft.js';
import { addedFolderLabel } from './createWorkspaceWizardModel.js';

export function folderDraftsHtml(sources, { deviceId, snapshot, advanced = false } = {}) {
  return `<div data-folder-drafts>${sources.map((source, index) => folderHtml(source, index, advanced ? addedFolderLabel(source, deviceId, snapshot) : '')).join('')}</div>
    <div class="row create-row"><button class="btn" type="button" data-folder-add="remote">Add Git remote</button><button class="btn" type="button" data-folder-add="path">Add folder</button></div>
    <div data-folder-browser></div>`;
}
function folderHtml(source, index, label) {
  const field = source.kind === 'remote'
    ? `<label for="create-folder-value-${index}">Git remote URL</label><input id="create-folder-value-${index}" data-folder-value="${index}" ${fieldTraits('identifier','next')} value="${esc(source.remote)}">`
    : `<p>${esc(source.path || 'No folder selected')}</p><button class="btn" type="button" data-folder-choose="${index}">Choose folder…</button>`;
  return `<fieldset class="field"><legend>Folder ${index + 1}${label ? ` · ${esc(label)}` : ''}</legend>${field}
    <label for="create-folder-label-${index}">Folder label</label><input id="create-folder-label-${index}" data-folder-name="${index}" ${fieldTraits('identifier','next')} value="${esc(source.name)}">
    <label for="create-folder-branch-${index}">Base branch (optional)</label><input id="create-folder-branch-${index}" data-folder-branch="${index}" ${fieldTraits('identifier')} value="${esc(source.base_branch)}">
    <button class="btn" type="button" data-folder-remove="${index}">Remove folder ${index + 1}</button></fieldset>`;
}
export function folderDraftError(sources, required = false) {
  if (required && !sources.length) return 'Add a folder or Git remote.';
  const names = new Set();
  for (const source of sources) {
    const error = sourceDraftError(source, names, { validateRemote: true });
    if (error) return error;
  }
  return '';
}
// The embedded browser never offers mkdir. Only fs.list may cross this draft boundary.
export function wireFolderDrafts(host, { sources, deviceId, changed, repaint, fail }) {
  const rename = source => { if (source.automaticName) source.name = uniqueSourceName(source[source.kind], sources, source); };
  const browse = async index => {
    const source = sources[index];
    if (!deviceId) return fail('Choose a device.');
    const container = host.querySelector('[data-folder-browser]');
    try {
      await openBrowser({ container, deviceId, callRpc: deviceCall(deviceId, 'browse folders'), gitOnly: false,
        startPath: source.path || undefined, onChoose: path => {
          if (!container.isConnected || sources[index] !== source) return;
          source.path = path; rename(source); changed(); repaint();
        } });
    } catch (error) { if (container.isConnected) fail(error.message); }
  };
  host.querySelectorAll('[data-folder-add]').forEach(button => { button.onclick = () => {
    if (!deviceId) return fail('Choose a device.');
    const source = { kind: button.dataset.folderAdd, path: '', remote: '', name: '', base_branch: '', automaticName: true };
    sources.push(source); changed(); repaint();
    if (source.kind === 'path') void browse(sources.length - 1);
    else host.querySelector('[data-folder-value]:last-of-type')?.focus();
  }; });
  wireSourceInputs(host, sources, rename, changed);
  host.querySelectorAll('[data-folder-remove]').forEach(button => { button.onclick = () => { sources.splice(Number(button.dataset.folderRemove), 1); changed(); repaint(); }; });
  host.querySelectorAll('[data-folder-choose]').forEach(button => { button.onclick = () => { void browse(Number(button.dataset.folderChoose)); }; });
}
function wireSourceInputs(host, sources, rename, changed) {
  for (const [attribute, property] of [['value', 'remote'], ['name', 'name'], ['branch', 'base_branch']]) {
    host.querySelectorAll(`[data-folder-${attribute}]`).forEach(input => { input.oninput = () => {
      const index = Number(input.dataset[`folder${attribute[0].toUpperCase()}${attribute.slice(1)}`]);
      const source = sources[index]; source[property] = input.value;
      if (property === 'name') source.automaticName = false;
      if (property === 'remote') { rename(source); host.querySelector(`[data-folder-name="${index}"]`).value = source.name; }
      changed();
    }; });
  }
}
