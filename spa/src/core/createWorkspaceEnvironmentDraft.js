import { esc } from './text.js';
import { fieldTraits } from './fieldTraits.js';
import { isolationOptionsHtml } from './isolation.js';
import { folderDraftsHtml, folderDraftError, wireFolderDrafts } from './createWorkspaceFolders.js';

export function environmentDraftHtml(state, snapshot) {
  const draft = state.environmentDraft;
  const choices = state.newEnvironmentDevices || [];
  const devices = choices.length > 1 ? `<label for="create-environment-device">Device</label><select id="create-environment-device"><option value="">Choose a device</option>${choices.map(device => `<option value="${esc(device.id)}"${device.id === draft.deviceId ? ' selected' : ''}>${esc(device.name)}</option>`).join('')}</select>` : '';
  return `<h3 id="create-workspace-title">New environment</h3><p class="sub">Choose folders or Git remotes. Nothing is created until Create workspace.</p>${devices}
    ${folderDraftsHtml(draft.sources, {deviceId:draft.deviceId,snapshot})}
    <label for="create-environment-name">Environment name (optional)</label><input id="create-environment-name" ${fieldTraits('line','next')} value="${esc(draft.name)}">
    <label for="create-environment-isolation">Isolation</label><select id="create-environment-isolation">${isolationOptionsHtml(draft.isolation,null,{inheritLabel:'Inherit device setting'})}</select>`;
}
export function initialEnvironmentDraft(options, model) {
  const choices = model.newEnvironmentDevices;
  const deviceId = options.deviceId || (choices.length === 1 ? choices[0].id : '');
  return { deviceId, name:'', nameAutomatic:true, isolation:'', sources:[] };
}
export function draftEnvironmentTarget(draft, devices) {
  const name = draft.name.trim() || draft.sources[0]?.name || 'New environment';
  return {kind:'new-environment',id:'draft',key:`${draft.deviceId}/draft`,deviceId:draft.deviceId,
    name,label:`${name} (new)`,summaryLabel:`${name} (new environment)`,sources:draft.sources,
    deviceLabel:devices.length > 1 ? devices.find(row => row.id === draft.deviceId)?.name || draft.deviceId : ''};
}
export function environmentDraftError(draft) {
  return draft.deviceId ? folderDraftError(draft.sources, true) : 'Choose a device.';
}
export function wireEnvironmentDraft(host, state, {save,paint,fail}) {
  const draft = state.environmentDraft;
  const changed = () => {
    if (draft.nameAutomatic) {
      draft.name = draft.sources[0]?.name || '';
      host.querySelector('#create-environment-name').value = draft.name;
    }
    save();
  };
  wireFolderDrafts(host,{sources:draft.sources,deviceId:draft.deviceId,changed,repaint:paint,fail});
  host.querySelector('#create-environment-name').oninput = event => {draft.name=event.target.value;draft.nameAutomatic=!draft.name.trim();save();};
  host.querySelector('#create-environment-isolation').onchange = event => {draft.isolation=event.target.value;save();};
  const device = host.querySelector('#create-environment-device');
  if (device) device.onchange = () => {
    draft.deviceId=device.value;
    draft.sources.forEach(source => {if(source.kind==='path')source.path='';});
    state.addedSources=[];changed();paint();
  };
}

export function refusalDraftPage(target, error, addedSources = []) {
  if (target?.kind !== 'new-environment') return 3;
  const message = error?.message || String(error);
  const added = addedSources.some(source => [source.path,source.remote].some(value => value && message.includes(value)));
  if (added || /project name|workspace|line of work/i.test(message)) return 3;
  return 2;
}
