import { folderDraftsHtml } from './createWorkspaceFolders.js';
import { environmentDraftHtml } from './createWorkspaceEnvironmentDraft.js';
import { kindGlyphHtml } from './environmentIdentity.js';
import { esc } from './text.js';
import { fieldTraits } from './fieldTraits.js';
import { isolationOptionsHtml } from './isolation.js';
import { branchNamePreview } from './toolbarModel.js';
import { modalDialogHtml } from './modal.js';
const glyph = kind => `<span class="create-kind" aria-hidden="true">${kindGlyphHtml(kind === 'project' ? 'project' : 'environment')}</span>`;
const deviceHtml = target => `${target.deviceLabel ? `<span class="dim inbox-device">${esc(target.deviceLabel)}</span>` : ''}${target.offline ? '<span class="dim inbox-away">Offline</span>' : ''}`;
const targetLabel = target => target.summaryLabel || target.label || target.name || target.id;
export function createWorkspaceTargetSummaryHtml(target) {
  return `${glyph(target.kind)}<span>${esc(targetLabel(target))}</span>${deviceHtml(target)}`;
}
export const targetText = target => [targetLabel(target),target.deviceLabel,target.offline ? 'Offline':''].filter(Boolean).join(' · ');
export const sameTarget = (a,b) => !!a && !!b && a.kind===b.kind && a.deviceId===b.deviceId && a.id===b.id;
export function wizardTargets(state) {
  const draft=state.draftTarget ? [{kind:'draft',title:'New environment',targets:[state.draftTarget]}]:[];
  return [...draft,...(state.sections || [])];
}
function targetHtml(target,index,state) {
  const selected=sameTarget(target,state.target);
  return `<div class="create-target-row"><button type="button" class="create-target" data-create-target="${index}" data-kind="${target.kind}" data-create-key="${esc(target.key || `${target.deviceId}/${target.id}`)}" aria-pressed="${selected}"${state.busy ? ' disabled':''}>
    ${glyph(target.kind)}<span class="create-target-copy"><span>${esc(target.label || target.name)}</span><span class="sub">${esc(target.description || 'Creates a new task board')}${target.empty ? ' · Resume…' : ''}</span></span>${deviceHtml(target)}<span class="create-selected" aria-hidden="true">${selected ? '✓':''}</span></button>
    ${target.kind==='new-environment' ? `<button type="button" class="btn" data-create-edit${state.busy ? ' disabled':''} aria-label="Edit new environment">Edit</button>`:''}</div>`;
}
function pickerHtml(state) {
  let index=0;
  return `<div class="create-targets">${wizardTargets(state).map(section=>`<section data-create-section="${section.kind}"><h4>${esc(section.title)}</h4>
    ${section.helper ? `<p class="sub">${esc(section.helper)}</p>`:''}${section.targets.map(target=>targetHtml(target,index++,state)).join('')}</section>`).join('')}
    ${state.emptyText ? `<p class="sub">${esc(state.emptyText)}</p>`:''}
    ${state.legacyNote ? `<p class="sub">${esc(state.legacyNote)}</p>`:''}</div>`;
}
function nameHtml(state) {
  const target=state.target;
  const outcome=target?.kind === 'project' ? `Uses ${target.name}'s task board · ${target.taskText || '0 tasks'}`:'Creates a new task board';
  return `<div class="create-name"><div class="create-name-heading"><label class="create-label" for="create-work-input">Name (optional)</label>
    ${target ? `<p class="create-target-summary">${createWorkspaceTargetSummaryHtml(target)}</p>`:''}</div>
    ${target ? `<p class="sub create-outcome">${esc(outcome)}</p>`:''}
    <input id="create-work-input" type="text" ${fieldTraits('identifier','go')} value="${esc(state.name || '')}"${state.busy ? ' disabled':''}>
    <p class="sub create-preview" data-branch-preview>${esc(branchNamePreview(state.name))}</p></div>`;
}
export function workspaceSourcesHtml(state) {
  const added=state.target.kind==='project' ? []:state.addedSources || [];
  return [...(state.target.sources || []), ...added].map(source=>`<li>${esc(source.path || source.remote || source.name)}</li>`).join('');
}
function advancedBodyHtml(state) {
  if(!state.target)return '<p>Choose a project or environment first.</p>';
  const target=state.target;
  return `<label class="create-label" for="create-work-isolation">Isolation</label><select id="create-work-isolation"${state.busy ? ' disabled':''}>
    ${isolationOptionsHtml(state.isolation,null,{inheritLabel:target.kind === 'project' ? 'Inherit project setting':"Inherit environment's setting"})}</select>
    <h4>Directories this workspace will contain</h4><ul class="create-sources">${workspaceSourcesHtml(state)}</ul>
    ${state.showAdvanced ? `<p>Also add folders to this project only</p>${folderDraftsHtml(state.addedSources || [],{deviceId:target.deviceId,snapshot:state.folderReferences || {},advanced:true})}`:''}`;
}
function advancedHtml(state) {
  return `<details data-create-advanced${state.advancedExpanded ? ' open':''}><summary id="create-advanced-summary" aria-controls="create-advanced-body">Advanced</summary>
    <div id="create-advanced-body" class="create-advanced-body" role="region" aria-labelledby="create-advanced-summary"><fieldset${state.busy ? ' disabled':''}>${advancedBodyHtml(state)}</fieldset></div></details>`;
}
function primaryActionHtml(state) {
  if(state.page===2)return `<button class="btn primary" type="button" data-create-next${state.busy ? ' disabled':''}>Use environment</button>`;
  return `<button class="btn primary" type="button" data-create-go${state.busy || state.operationId ? ' disabled':''}>${state.busy ? 'Creating…':'Create workspace'}</button>`;
}
function wizardActionsHtml(state) {
  const disabled=state.busy ? ' disabled':'';
  const back=state.page === 2 ? `<button class="btn" type="button" data-create-back${disabled}>Back</button>`:'';
  const fresh=state.page!==2 && !state.presetTarget && state.newEnvironmentDevices?.length ? `<button class="btn create-fresh" type="button" data-create-new-environment${disabled}>New environment…</button>`:'';
  return `<div class="row create-row">${fresh}${back}<button class="btn" type="button" data-create-cancel${disabled}>${state.operationId ? 'Close':'Cancel'}</button>${primaryActionHtml(state)}</div>`;
}
function searchHtml(state) {
  if(state.page===2 || state.presetTarget)return '';
  return `<label class="create-label" for="create-workspace-search">Find a project or environment</label><input id="create-workspace-search" type="search" ${fieldTraits('search')} value="${esc(state.query || '')}"${state.busy ? ' disabled':''}>`;
}
function resumeHtml(state) {
  return state.operationId ? `<p class="create-resume" role="status">Still creating ‹${esc(state.name || 'workspace')}›… <button class="btn" type="button" data-create-resume>View progress</button></p>`:'';
}
function pageContentHtml(state) {
  if(state.page===2)return environmentDraftHtml(state,state.folderReferences || {});
  return state.presetTarget ? '':pickerHtml(state);
}
export function createWorkspaceWizardBodyHtml(state) {
  return `<div class="create-workspace-page${state.pageTransition ? ' create-workspace-page-enter':''}"><header class="create-workspace-top">${state.page===2 ? '':`<h3 id="create-workspace-title">New workspace</h3>`}${searchHtml(state)}</header>
    <div class="create-workspace-scroll" data-create-scroll>${pageContentHtml(state)}</div>
    <div class="create-workspace-bottom">${state.page===2 ? '':nameHtml(state)+advancedHtml(state)}${resumeHtml(state)}<div class="warn create-error" role="alert"${state.error ? '':' hidden'}>${esc(state.error || '')}</div>${wizardActionsHtml(state)}</div></div>`;
}
export function createWorkspaceWizardHtml(state) {
  return modalDialogHtml(createWorkspaceWizardBodyHtml(state),{className:'modal-create create-workspace-sheet create-workspace-wizard'});
}
export function wireWizardAdvanced(body,onToggle=()=>{},busy=false) {
  const details=body.querySelector('[data-create-advanced]');
  if(!details)return;
  let wasOpen=details.open;
  details.querySelector('summary').onclick=event=>{if(busy)event.preventDefault();};
  details.ontoggle=()=>{
    if(wasOpen===details.open)return;
    wasOpen=details.open;onToggle(wasOpen);
  };
}
