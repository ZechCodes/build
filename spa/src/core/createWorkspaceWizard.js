import { folderDraftsHtml, folderDraftError, wireFolderDrafts } from './createWorkspaceFolders.js';
import { environmentDraftHtml, initialEnvironmentDraft, draftEnvironmentTarget, environmentDraftError, wireEnvironmentDraft, refusalDraftPage } from './createWorkspaceEnvironmentDraft.js';
import { go } from '../app.js';
import { esc } from './text.js';
import { fieldTraits } from './fieldTraits.js';
import { isolationOptionsHtml } from './isolation.js';
import { modalDialogHtml, openModal } from './modal.js';
import { branchNamePreview } from './toolbarModel.js';
import { uiAddress, watchUiState, updateUiState } from './localUiState.js';
import { readUiRecord, deleteUiDraftsIfUnwritten } from './localUiStore.js';
import { createWorkspaceWizardModel } from './createWorkspaceWizardModel.js';
import { openCreateWorkspaceProgress } from './createWorkspaceProgress.js';
import { openCreateWorkspaceLegacy } from './createWorkspaceLegacy.js';
import { readCreateWorkspaceTargets, followCreateWorkspaceTargets, submitCreateWorkspace,
  createLegacyWorkspace, cacheCreatedResources, createdWorkspaceRoute, discardCreatedProject, discardCreatedEnvironment, consumeCreateWorkspaceAttempt } from './createWorkspaceTargets.js';
export { createWorkspaceWizardModel } from './createWorkspaceWizardModel.js';
const glyph = kind => `<span class="create-kind" aria-hidden="true">${kind === 'project' ? '◇' : '▣'}</span>`;
const deviceHtml = target => `${target.deviceLabel ? `<span class="dim inbox-device">${esc(target.deviceLabel)}</span>` : ''}${target.offline ? '<span class="dim inbox-away">Offline</span>' : ''}`;
const targetLabel = target => target.summaryLabel || target.label || target.name || target.id;
export function createWorkspaceTargetSummaryHtml(target) {
  return `${glyph(target.kind)}<span>${esc(targetLabel(target))}</span>${deviceHtml(target)}`;
}
const targetText = target => [targetLabel(target),target.deviceLabel,target.offline ? 'Offline':''].filter(Boolean).join(' · ');
function targetHtml(target,index) {
  return `<button type="button" class="create-target" data-create-target="${index}" data-kind="${target.kind}">
    ${glyph(target.kind)}<span class="create-target-copy"><span>${esc(target.label)}</span><span class="sub">${esc(target.description)}${target.empty ? ' · Resume…' : ''}</span></span>${deviceHtml(target)}</button>`;
}
function pickerHtml(state) {
  let index=0;
  return `<h3 id="create-workspace-title">Where?</h3>
    <label class="create-label" for="create-workspace-search">Find a project or environment</label>
    <input id="create-workspace-search" type="search" ${fieldTraits('search')} value="${esc(state.query || '')}">
    <div class="create-targets">${(state.sections || []).map(section=>`<section data-create-section="${section.kind}"><h4>${esc(section.title)}</h4>
      ${section.helper ? `<p class="sub">${esc(section.helper)}</p>`:''}${section.targets.map(target=>targetHtml(target,index++)).join('')}</section>`).join('')}
    ${state.emptyText ? `<p class="sub">${esc(state.emptyText)}</p><p class="sub">Build keeps your code in environments. Make one from a folder or a Git URL.</p>`:''}</div>
    ${state.newEnvironmentDevices?.length ? '<button class="btn" type="button" data-create-new-environment>New environment…</button>' : ''}
    ${state.legacyNote ? `<p class="sub">${esc(state.legacyNote)}</p>`:''}`;
}
function nameHtml(state) {
  const target=state.target;
  const outcome=target.kind === 'project' ? `Uses ${target.name}'s task board · ${target.taskText || '0 tasks'}`:'Creates a new task board';
  return `<h3 id="create-workspace-title">New workspace in ${esc(targetLabel(target))}</h3>
    <p class="sub">What are you working on?</p><p class="create-target-summary">${createWorkspaceTargetSummaryHtml(target)}</p>
    <p class="sub">${esc(outcome)}</p>
    <label class="create-label" for="create-work-input">Name${target.kind === 'project' ? ' (optional)':''}</label>
    <input id="create-work-input" type="text" ${fieldTraits('identifier','go')} value="${esc(state.name || '')}"${target.kind !== 'project' ? ' required':''}${state.busy ? ' disabled':''}>
    <p class="sub create-preview" data-branch-preview>${esc(branchNamePreview(state.name))}</p>
    <label class="create-label" for="create-work-isolation">Isolation</label><select id="create-work-isolation"${state.busy ? ' disabled':''}>
    ${isolationOptionsHtml(state.isolation,null,{inheritLabel:target.kind === 'project' ? 'Inherit project setting':"Inherit environment's setting"})}</select>
    <h4>Directories this workspace will contain</h4><ul class="create-sources">${workspaceSourcesHtml(state)}</ul>${advancedHtml(state)}`;
}
function workspaceSourcesHtml(state) {
  const added=state.target.kind==='project' ? []:state.addedSources || [];
  return [...(state.target.sources || []), ...added].map(source=>`<li>${esc(source.path || source.remote || source.name)}</li>`).join('');
}
function advancedHtml(state) {
  if (!state.showAdvanced || state.target?.kind === 'project') return '';
  return `<details data-create-advanced${state.advancedExpanded ? ' open' : ''}><summary>Advanced</summary><p>Also add folders to this project only</p>${folderDraftsHtml(state.addedSources || [],{deviceId:state.target.deviceId,snapshot:state.folderReferences || {},advanced:true})}</details>`;
}
function wizardActionsHtml(state) {
  const back=state.page !== 1 && !state.fixedProject ? '<button class="btn" type="button" data-create-back>Back</button>':'';
  const create=state.page === 3 ? `<button class="btn primary" type="button" data-create-go${state.busy || state.operationId ? ' disabled':''}>${state.busy ? 'Creating…':'Create workspace'}</button>`:'';
  return `<div class="row create-row">${back}<button class="btn" type="button" data-create-cancel${state.busy ? ' disabled':''}>${state.operationId ? 'Close':'Cancel'}</button>${state.page === 2 ? '<button class="btn primary" type="button" data-create-next>Next</button>' : ''}${create}</div>`;
}
export function createWorkspaceWizardBodyHtml(state) {
  const page=wizardPageHtml(state);
  const progress=state.operationId ? `<p role="status">Still creating ‹${esc(state.name || 'workspace')}›… <button class="btn" type="button" data-create-resume>View progress</button></p>`:'';
  return `<div class="create-workspace-page${state.pageTransition ? ' create-workspace-page-enter':''}">${page}</div>${progress}<div class="warn create-error" role="alert"${state.error ? '':' hidden'}>${esc(state.error || '')}</div>${wizardActionsHtml(state)}`;
}
function wizardPageHtml(state) {
  if (state.page === 2) return environmentDraftHtml(state,state.folderReferences || {});
  return state.page === 3 && state.target ? nameHtml(state):pickerHtml(state);
}
export function createWorkspaceWizardHtml(state) {
  return modalDialogHtml(createWorkspaceWizardBodyHtml(state),{className:'modal-create create-workspace-sheet'});
}
const draftOf = state => ({target:state.target,page:state.page,name:state.name,isolation:state.isolation,operationId:state.operationId,continues:state.continues,environmentDraft:state.environmentDraft,addedSources:state.addedSources,advancedExpanded:state.advancedExpanded});
async function deleteDraft(address) {
  const record=await readUiRecord(address);
  if(record) await deleteUiDraftsIfUnwritten([{address,record}],()=>true);
}
function trapTab(event,body) {
  const controls=[...body.querySelectorAll('button:not(:disabled),input:not(:disabled),select:not(:disabled)')];
  const edge=event.shiftKey ? controls[0]:controls.at(-1);
  if(document.activeElement !== edge) return;
  event.preventDefault();(event.shiftKey ? controls.at(-1):controls[0])?.focus();
}
function pickerKey(event,body) {
  if(event.key === '/' && event.target.tagName !== 'INPUT') {event.preventDefault();body.querySelector('#create-workspace-search')?.focus();return;}
  const direction={ArrowDown:1,ArrowUp:-1}[event.key];
  if(!direction) return;
  const rows=[...body.querySelectorAll('[data-create-target]')];
  if(!rows.length) return;
  event.preventDefault();const at=rows.indexOf(document.activeElement);rows[(at+direction+rows.length)%rows.length].focus();
}
function validName(input,target,name) {
  input.setCustomValidity(target.kind !== 'project' && !name.trim() ? 'Enter a name for this line of work.':'');
  if(input.checkValidity()) return true;
  input.reportValidity();return false;
}
function supportsWorkspaceWizard(snapshot,options) {
  return options.deviceId ? snapshot.support?.[options.deviceId] === true : Object.values(snapshot.support || {}).some(Boolean);
}
function retainedCreationAttempt(snapshot,options) {
  const fixedProject=options.fixedProject ?? Boolean(options.projectId);
  return (snapshot.attempts || []).filter(row=>!fixedProject ||
    row.draft.target.id===options.projectId && row.draft.target.deviceId===options.deviceId)
    .sort((left,right)=>(right.localOrder ?? right.createdAt ?? 0)-(left.localOrder ?? left.createdAt ?? 0))[0];
}
const wizardDraftAddress=options=>uiAddress({deviceId:options.deviceId,entityId:options.projectId || options.environmentId,view:'create-workspace',kind:'draft'});
function openLegacySurface(options,savedDraft) {
  const target=!options.projectId && savedDraft?.target?.kind === 'project' ? savedDraft.target:null;
  return openCreateWorkspaceLegacy({...options,...(target ? {projectId:target.id,deviceId:target.deviceId,projectName:target.name}:{}),
    name:savedDraft?.name || '',isolation:savedDraft?.isolation || '',onClose:()=>{void deleteDraft(wizardDraftAddress(options));}});
}
function openCreationSurface(options,snapshot,savedDraft) {
  if(supportsWorkspaceWizard(snapshot,options))return mountCreateWorkspaceWizard(options,snapshot);
  const attempt=retainedCreationAttempt(snapshot,options);
  if(attempt)return mountCreateWorkspaceWizard(options,snapshot,attempt);
  return options.fromShortcut ? null : openLegacySurface(options,savedDraft?.continues ? savedDraft:undefined);
}
export function openCreateWorkspaceWizard(options = {}) {
  let screen=null,closed=false;
  const ready=Promise.all([readCreateWorkspaceTargets(),readUiRecord(wizardDraftAddress(options))]).then(async([snapshot,record])=>{
    if(closed)return;
    screen=openCreationSurface(options,snapshot,record?.value);
    await screen?.ready;
  });
  return {ready,
    close:()=>{closed=true;return screen?.close?.();},
    detach:()=>{closed=true;return (screen?.detach || screen?.close)?.();},
    flushDraft:()=>ready.then(()=>screen?.flushDraft?.()),
  };
}
function mountCreateWorkspaceWizard(options,initialSnapshot,recoveryAttempt=null) {
  const fixedProject=options.fixedProject ?? Boolean(options.projectId);
  let snapshot=initialSnapshot;
  const initial=createWorkspaceWizardModel({...snapshot,...options,fixedProject});
  const state={...initial,query:'',name:'',isolation:'',error:options.error || '',busy:false,operationId:null,fixedProject,
    environmentDraft:initialEnvironmentDraft(options,initial),addedSources:[],advancedExpanded:options.advancedExpanded === true,folderReferences:snapshot};
  updateAdvanced();
  const opener=document.activeElement;
  const address=uiAddress({deviceId:options.deviceId,entityId:options.projectId || options.environmentId,view:'create-workspace',kind:'draft'});
  if(recoveryAttempt)return recoverProgress(recoveryAttempt);
  let legacyScreen=null;
  let detached=false,submitting=false,readGeneration=0;
  let draft,stop=()=>{};
  const modal=openModal({dialogHtml:createWorkspaceWizardHtml(state),scrimId:'create-scrim',canDismiss:()=>!state.busy,onClose:cancelled,initialFocus:':not(*)'});
  modal.body.setAttribute('aria-labelledby','create-workspace-title');
  modal.body.dataset.createPage=String(state.page);
  draft=watchUiState(address,restoreDraft,{active:()=>!detached});
  stop=followCreateWorkspaceTargets(()=>{void refresh();});
  wire();
  const ready=draft.ready.then(async()=>{await recoverAttempt();if(!detached)focusPage(options.fromShortcut);});
  return {ready,close:()=>legacyScreen ? legacyScreen.close():cancel(),
    detach:()=>legacyScreen ? legacyScreen.detach():detach(),flushDraft:()=>draft.flush()};

  function recoverProgress(attempt) {
    Object.assign(state,attempt.draft,{operationId:attempt.operationId,page:3});
    let progress=null,closed=false;
    const ready=updateUiState(address,()=>draftOf(state)).then(async()=>{
      if(closed)return;
      progress=openProgress(attempt.operationId);await progress.ready;
    });
    const close=()=>{closed=true;return progress?.close();};
    return {ready,close,detach:close};
  }

  async function recoverAttempt() {
    if(detached)return;
    const attempts=(snapshot.attempts || []).filter(row=>!fixedProject ||
      row.draft.target.id===options.projectId && row.draft.target.deviceId===options.deviceId);
    const latest=[...attempts].sort((left,right)=>(right.localOrder ?? right.createdAt ?? 0)-(left.localOrder ?? left.createdAt ?? 0));
    const attempt=state.operationId ? latest.find(row=>row.operationId===state.operationId):latest[0];
    if(!attempt)return;
    Object.assign(state,attempt.draft,{operationId:attempt.operationId,page:3});updateModel();paint();
    await draft.write(draftOf(state));
    if(['succeeded','failed','result_expired'].includes(attempt.state))await showProgress(attempt.operationId);
  }
  function restoreDraft(value) {
    if(!value || detached || JSON.stringify(value) === JSON.stringify(draftOf(state))) return;
    Object.assign(state,value);updateModel();paint();
  }
  function save() {void draft.write(draftOf(state));}
  function dispose() {if(detached)return;detached=true;stop();draft.dispose();}
  async function detach() {await draft.flush();dispose();await modal.close();}
  async function cancelled() {
    if(detached) return;
    await draft.flush();dispose();if(!state.operationId)await deleteDraft(address);opener?.focus();
  }
  async function cancel() {
    if(state.busy) return;
    await cancelled();await modal.close();
  }
  async function refresh() {
    const requested=++readGeneration;
    const next=await readCreateWorkspaceTargets();
    if(detached || requested !== readGeneration) return;
    snapshot=next;updateModel();paint();
  }
  function updateModel() {
    const model=createWorkspaceWizardModel({...snapshot,...options,fixedProject,query:state.query});
    state.sections=model.sections;state.emptyText=model.emptyText;state.legacyNote=model.legacyNote;state.newEnvironmentDevices=model.newEnvironmentDevices;state.folderReferences=snapshot;
    if(!state.target && model.target){state.target=model.target;state.page=3;}
    if(state.target) refreshTarget(model);
    updateAdvanced();
  }
  function updateAdvanced() {
    state.showAdvanced=state.target?.kind !== 'project' && snapshot.features?.[state.target?.deviceId]?.addedSources === true;
  }
  function refreshTarget(model) {
    const targets=model.sections.flatMap(section=>section.targets);
    state.target=targets.find(target=>target.kind===state.target.kind && target.key===state.target.key) || state.target;
  }
  function focusPage(shortcut=false) {
    const selector=state.page===3 ? '#create-work-input':state.page===2 ? '#create-environment-device, [data-folder-add]':shortcut ? '[data-create-target], [data-create-new-environment]':'#create-workspace-search';
    modal.body.querySelector(selector)?.focus();
  }
  function paint() {
    if(detached) return;
    const focused=document.activeElement;
    const id=modal.body.contains(focused) ? focused.id:null;
    const selection=focused?.selectionStart;
    state.pageTransition=modal.body.dataset.createPage !== String(state.page);
    modal.body.dataset.createPage=String(state.page);
    modal.body.innerHTML=createWorkspaceWizardBodyHtml(state);wire();
    const restored=id ? modal.body.querySelector(`#${id}`):null;
    restored?.focus();if(restored?.type==='text' && selection!==null) restored.setSelectionRange(selection,selection);
  }
  function changeAddedDevice(deviceId) {
    if(!state.target || state.target.deviceId===deviceId)return;
    state.addedSources.forEach(source=>{if(source.kind==='path')source.path='';});
  }
  function selectTarget(target) {
    if(!snapshot.support?.[target.deviceId]){void handoffLegacy(target);return;}
    changeAddedDevice(target.deviceId);state.target=target;state.page=3;state.error='';updateAdvanced();save();paint();focusPage();
  }
  async function handoffLegacy(target) {
    await cancel();
    legacyScreen=openCreateWorkspaceLegacy({projectId:target.id,deviceId:target.deviceId,projectName:target.name,navigate:options.navigate || go});
  }
  function wire() {
    const body=modal.body;
    body.onkeydown=event=>{
      if(event.key==='Tab')trapTab(event,body);
      if(state.page===1)pickerKey(event,body);
      if(event.key==='Enter' && event.target.id==='create-work-input'){event.preventDefault();void submit();}
      if(event.key==='Enter' && event.target.hasAttribute('data-create-target')){event.preventDefault();event.target.click();}
    };
    wirePicker();wireName();wireActions();wireFolders();
  }
  function wireActions() {
    modal.body.querySelector('[data-create-cancel]').onclick=()=>{void cancel();};
    const back=modal.body.querySelector('[data-create-back]');if(back)back.onclick=()=>{state.page=state.page===3 && state.target?.kind==='new-environment' ? 2:1;save();paint();focusPage();};
    const next=modal.body.querySelector('[data-create-next]');if(next)next.onclick=nextPage;
    const create=modal.body.querySelector('[data-create-go]');if(create)create.onclick=()=>{void submit();};
    const resume=modal.body.querySelector('[data-create-resume]');if(resume)resume.onclick=()=>{void showProgress(state.operationId);};
  }
  function wireFolders() {
    const fail=error=>{state.error=error;paint();};
    if(state.page===2) {wireEnvironmentDraft(modal.body,state,{save,paint,fail});return;}
    const advanced=modal.body.querySelector('[data-create-advanced]');
    if(!advanced)return;
    advanced.ontoggle=()=>{state.advancedExpanded=advanced.open;save();};
    wireFolderDrafts(modal.body,{sources:state.addedSources,deviceId:state.target.deviceId,changed:save,repaint:paint,fail});
  }
  function nextPage() {
    state.error=environmentDraftError(state.environmentDraft);
    if(state.error){paint();return;}
    state.target=draftEnvironmentTarget(state.environmentDraft,snapshot.devices);state.page=3;updateAdvanced();save();paint();focusPage();
  }
  function wirePicker() {
    const targets=(state.sections || []).flatMap(section=>section.targets);
    modal.body.querySelectorAll('[data-create-target]').forEach(button=>{button.onclick=()=>selectTarget(targets[Number(button.dataset.createTarget)]);});
    const fresh=modal.body.querySelector('[data-create-new-environment]');
    if(fresh)fresh.onclick=()=>{changeAddedDevice(state.environmentDraft.deviceId);state.page=2;state.error='';save();paint();focusPage();};
    const search=modal.body.querySelector('#create-workspace-search');
    if(search)search.oninput=()=>{state.query=search.value;updateModel();paint();};
  }
  function wireName() {
    const input=modal.body.querySelector('#create-work-input');
    if(input)input.oninput=()=>{state.name=input.value;modal.body.querySelector('[data-branch-preview]').textContent=branchNamePreview(state.name);save();};
    const isolation=modal.body.querySelector('#create-work-isolation');if(isolation)isolation.onchange=()=>{state.isolation=isolation.value;save();};
  }
  function validSubmission(input) {
    if(!validName(input,state.target,state.name))return false;
    const error=state.showAdvanced ? folderDraftError(state.addedSources):'';
    if(!error)return true;
    state.error=error;state.advancedExpanded=true;paint();return false;
  }
  async function submit() {
    if(submitting || state.operationId || !state.target) return;
    const input=modal.body.querySelector('#create-work-input');
    if(!validSubmission(input))return;
    submitting=true;state.busy=true;state.error='';paint();
    try {
      await draft.write(draftOf(state));
      if(!snapshot.support?.[state.target.deviceId]) await submitLegacy();
      else await submitDurable();
    }catch(error){if(!detached){state.busy=false;state.error=error.message || String(error);refusalPage();save();paint();}}
    finally{submitting=false;}
  }
  async function submitLegacy() {
    if(state.target.kind !== 'project') throw new Error('This device needs an update to create environments here');
    const route=await createLegacyWorkspace({target:state.target,name:state.name,isolation:state.isolation});
    await deleteDraft(address);dispose();await modal.close();(options.navigate || go)(route);
  }
  async function submitDurable(continues=state.continues) {
    const attempt=submitCreateWorkspace({...draftOf(state),continues,draft:draftOf(state)});
    await attempt.ready;
    state.operationId=attempt.operationId;state.busy=false;
    await draft.write(draftOf(state));
    const screen=await showProgress(attempt.operationId);
    void attempt.completion.catch(async error=>{
      if(await attempt.get())return;
      await screen.close();await returnToWizard(error.message || String(error));
    });
  }
  function refusalPage() {
    state.page=refusalDraftPage(state.target,state.error,state.addedSources);
    state.advancedExpanded=state.page===3;
  }
  async function returnToWizard(error='',continues=state.operationId) {
    state.error=error;refusalPage();
    await updateUiState(address,held=>({...held,...draftOf(state),operationId:null,continues}));
    if(continues)await consumeCreateWorkspaceAttempt(state.target.deviceId,continues);
    const next=openCreateWorkspaceWizard({...options,error});await next.ready;
  }
  function landedProjectTarget(project) {
    const deviceId=state.target.deviceId;
    const id=project.project_id || project.id;
    const projects=[...(snapshot.projects || []).filter(row=>row.deviceId !== deviceId || row.id !== id),{...project,id,deviceId}];
    return createWorkspaceWizardModel({...snapshot,projects,projectId:id,deviceId,fixedProject:true}).target;
  }
  async function retryWorkspace({project,operationId}) {
    state.target=landedProjectTarget(project);
    await submitContinuation(operationId);
  }
  async function submitContinuation(operationId) {
    const attempt=submitCreateWorkspace({...draftOf(state),continues:operationId,draft:draftOf(state)});
    await attempt.ready;
    state.operationId=attempt.operationId;
    await updateUiState(address,()=>draftOf(state));
    await consumeCreateWorkspaceAttempt(state.target.deviceId,operationId);
    const screen=openProgress(attempt.operationId);
    void attempt.completion.catch(async error=>{
      if(await attempt.get())return;
      await screen.close();await returnToWizard(error.message || String(error),operationId);
    });
  }
  async function resumeEnvironment({environment,operationId}) {
    const id=environment.environment_id || environment.id;
    state.target={...environment,id,key:`${state.target.deviceId}/${id}`,deviceId:state.target.deviceId,kind:'environment',summaryLabel:environment.name};
    state.operationId=null;state.continues=operationId;
    await submitContinuation(operationId);
  }
  function openProgress(operationId) {
    return openCreateWorkspaceProgress({operationId,deviceId:state.target.deviceId,target:state.target,targetSummary:targetText(state.target),
      onKeepProject:async()=>{await consumeCreateWorkspaceAttempt(state.target.deviceId,operationId);await deleteDraft(address);},onClose:()=>{if(!document.activeElement?.closest('[role="dialog"]'))opener?.focus();},
      onBack:payload=>returnToWizard(payload?.record?.confirmation?.error?.message || ''),onRetryWorkspace:retryWorkspace,
      onResumeEnvironment:resumeEnvironment,
      onKeepEnvironment:async()=>{await consumeCreateWorkspaceAttempt(state.target.deviceId,operationId);await deleteDraft(address);},
      onDiscardEnvironment:async payload=>{await discardCreatedEnvironment({...payload,target:state.target});await consumeCreateWorkspaceAttempt(state.target.deviceId,operationId);await deleteDraft(address);},
      onDiscardProject:async payload=>{await discardCreatedProject({...payload,target:state.target});await consumeCreateWorkspaceAttempt(state.target.deviceId,operationId);await deleteDraft(address);},
      onOpenWorkspace:async({record,workspace})=>{
        const result=record.confirmation?.result || workspace;
        const route=createdWorkspaceRoute(state.target.deviceId,result);
        await cacheCreatedResources(state.target.deviceId,result);await consumeCreateWorkspaceAttempt(state.target.deviceId,operationId);await deleteDraft(address);(options.navigate || go)(route);
      }});
  }
  async function showProgress(operationId) {await detach();return openProgress(operationId);}
}
