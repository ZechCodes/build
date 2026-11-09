import { folderDraftError, wireFolderDrafts } from './createWorkspaceFolders.js';
import { initialEnvironmentDraft, draftEnvironmentTarget, environmentDraftError, wireEnvironmentDraft, refusalDraftPage } from './createWorkspaceEnvironmentDraft.js';
import { creationSupportForMethod, creationMethodForTarget } from './writeOperationProgress.js';
import { environmentWizardSupported } from './environmentSupport.js';
import { go } from '../app.js';
import { messageOf } from './text.js';
import { openModal } from './modal.js';
import { branchNamePreview } from './toolbarModel.js';
import { uiAddress, watchUiState, updateUiState } from './localUiState.js';
import { readUiRecord, deleteUiDraftsIfUnwritten } from './localUiStore.js';
import { createWorkspaceWizardBodyHtml, createWorkspaceWizardHtml, wireWizardAdvanced, workspaceSourcesHtml, targetText, wizardTargets, sameTarget } from './createWorkspaceWizardRender.js';
export { createWorkspaceWizardBodyHtml, createWorkspaceWizardHtml, createWorkspaceTargetSummaryHtml, wireWizardAdvanced } from './createWorkspaceWizardRender.js';
import { optionalProjectName, createWorkspaceWizardModel } from './createWorkspaceWizardModel.js';
import { openCreateWorkspaceProgress } from './createWorkspaceProgress.js';
import { openCreateWorkspaceLegacy } from './createWorkspaceLegacy.js';
import { readCreateWorkspaceTargets, followCreateWorkspaceTargets, submitCreateWorkspace,
  createLegacyWorkspace, cacheCreatedResources, createdWorkspaceRoute, discardCreatedProject, discardCreatedEnvironment, consumeCreateWorkspaceAttempt } from './createWorkspaceTargets.js';
export { createWorkspaceWizardModel } from './createWorkspaceWizardModel.js';
const draftOf = state => ({target:state.target,page:state.page,name:state.name,isolation:state.isolation,operationId:state.operationId,continues:state.continues,environmentDraft:state.environmentDraft,addedSources:state.addedSources,advancedExpanded:state.advancedExpanded});
async function deleteDraft(address) {
  const record=await readUiRecord(address);
  if(record) await deleteUiDraftsIfUnwritten([{address,record}],()=>true);
}
function trapTab(event,body) {
  const controls=[...body.querySelectorAll('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),summary')].filter(el=>!el.closest('[hidden]') && (!el.closest('details:not([open])') || el.tagName==='SUMMARY')).sort((a,b)=>a.compareDocumentPosition(b)&Node.DOCUMENT_POSITION_FOLLOWING ? -1:1);
  const edge=event.shiftKey ? controls[0]:controls.at(-1);
  if(document.activeElement !== edge) return;
  event.preventDefault();(event.shiftKey ? controls.at(-1):controls[0])?.focus();
}
function pickerKey(event,body) {
  if(event.key === '/' && !event.target.closest('input,textarea,select,[contenteditable]')) {event.preventDefault();body.querySelector('#create-workspace-search')?.focus();return;}
  const direction={ArrowDown:1,ArrowUp:-1}[event.key];
  if(!direction || event.target.closest('input,textarea,select,[contenteditable],.create-error,.create-resume')) return;
  const rows=[...body.querySelectorAll('[data-create-target], [data-create-new-environment]')];
  if(!rows.length) return;
  event.preventDefault();const at=rows.indexOf(document.activeElement);rows[(at+direction+rows.length)%rows.length].focus();
}
function initialFocusSelector(state,name=false) {
  if(state.page===2)return '#create-environment-device, [data-folder-add]';
  return state.presetTarget || name ? '#create-work-input':'#create-workspace-search';
}
function rememberWizardFocus(body) {
  const focused=document.activeElement;
  if(!body.contains(focused))return {};
  return {id:focused.id,kind:focused.dataset.kind,key:focused.dataset.createKey,selection:focused.selectionStart};
}
function restoreWizardFocus(body,{id,kind,key,selection}) {
  let restored=id && body.querySelector(`#${id}`);
  if(key)restored=[...body.querySelectorAll('[data-create-target]')].find(row=>row.dataset.kind===kind && row.dataset.createKey===key)
    || body.querySelector('#create-workspace-search');
  if(!restored)return;
  restored.focus({preventScroll:true});
  if(restored.type==='text' && selection!=null)restored.setSelectionRange(selection,selection);
}
function deviceSupportsWizard(snapshot,deviceId) {
  const features=snapshot.features?.[deviceId];
  return features ? environmentWizardSupported(features):snapshot.support?.[deviceId] === true;
}
function targetSupportsDurableCreation(snapshot,target) {
  const flags=snapshot.features?.[target.deviceId] || {createProject:snapshot.support?.[target.deviceId] === true};
  return creationSupportForMethod(flags,creationMethodForTarget(target));
}
function supportsWorkspaceWizard(snapshot,options) {
  if(options.deviceId)return deviceSupportsWizard(snapshot,options.deviceId);
  const ids=new Set([...Object.keys(snapshot.support || {}),...Object.keys(snapshot.features || {})]);
  return [...ids].some(id=>deviceSupportsWizard(snapshot,id));
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
  if(supportsWorkspaceWizard(snapshot,options))return mountCreateWorkspaceWizard(options,snapshot,null,savedDraft);
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
function mountCreateWorkspaceWizard(options,initialSnapshot,recoveryAttempt=null,savedDraft=null) {
  const fixedProject=options.fixedProject ?? Boolean(options.projectId);
  let snapshot=initialSnapshot;
  const initial=createWorkspaceWizardModel({...snapshot,...options,fixedProject});
  const state={...initial,query:'',name:'',isolation:'',error:options.error || '',busy:false,operationId:null,fixedProject,
    environmentDraft:initialEnvironmentDraft(options,initial),addedSources:[],advancedExpanded:options.advancedExpanded === true,folderReferences:snapshot};
  state.presetTarget=Boolean(initial.target);
  if(savedDraft)Object.assign(state,savedDraft);
  if(state.page===3)state.page=1;
  updateAdvanced();
  const opener=document.activeElement;
  const address=uiAddress({deviceId:options.deviceId,entityId:options.projectId || options.environmentId,view:'create-workspace',kind:'draft'});
  if(recoveryAttempt)return recoverProgress(recoveryAttempt);
  let legacyScreen=null;
  let detached=false,submitting=false,readGeneration=0;
  let draft,stop=()=>{};
  const modal=openModal({dialogHtml:createWorkspaceWizardHtml(state),scrimId:'create-scrim',canDismiss:()=>!state.busy,onClose:cancelled,initialFocus:initialFocusSelector(state)});
  modal.body.setAttribute('aria-labelledby','create-workspace-title');
  modal.body.dataset.createPage=String(state.page);
  draft=watchUiState(address,restoreDraft,{active:()=>!detached});
  stop=followCreateWorkspaceTargets(()=>{void refresh();});
  wire();
  const ready=draft.ready.then(async()=>{await recoverAttempt();if(!detached)focusPage();});
  return {ready,close:()=>legacyScreen ? legacyScreen.close():cancel(),
    detach:()=>legacyScreen ? legacyScreen.detach():detach(),flushDraft:()=>draft.flush()};

  function recoverProgress(attempt) {
    Object.assign(state,attempt.draft,{operationId:attempt.operationId,page:1});
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
    Object.assign(state,attempt.draft,{operationId:attempt.operationId,page:1});updateModel();paint();
    await draft.write(draftOf(state));
    if(['succeeded','failed','result_expired'].includes(attempt.state))await showProgress(attempt.operationId);
  }
  function restoreDraft(value) {
    if(!value || detached || JSON.stringify(value) === JSON.stringify(draftOf(state))) return;
    Object.assign(state,value);if(state.page===3)state.page=1;updateModel();paint();
  }
  function save() {syncEnvironmentTarget();void draft.write(draftOf(state));}
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
    if(!state.target && model.target){state.target=model.target;state.page=1;}
    if(state.target) refreshTarget(model);
    updateAdvanced();
  }
  function syncEnvironmentTarget() {
    state.draftTarget=draftEnvironmentTarget(state.environmentDraft,snapshot.devices);
    if(state.target?.kind==='new-environment'){
      changeAddedDevice(state.draftTarget.deviceId);state.target=state.draftTarget;
    }
    if(!state.environmentDraft.sources.length && state.target?.kind!=='new-environment')state.draftTarget=null;
  }
  function updateAdvanced() {
    syncEnvironmentTarget();
    state.showAdvanced=state.target?.kind !== 'project' && snapshot.features?.[state.target?.deviceId]?.addedSources === true;
  }
  function refreshTarget(model) {
    const targets=model.sections.flatMap(section=>section.targets);
    state.target=targets.find(target=>target.kind===state.target.kind && target.key===state.target.key) || state.target;
  }
  function focusPage(shortcut=false) {
    const selector=initialFocusSelector(state,shortcut);
    modal.body.querySelector(selector)?.focus();
  }
  function paint(keepPicker=false) {
    if(detached) return;
    const focus=rememberWizardFocus(modal.body);
    const scrollTop=modal.body.querySelector('[data-create-scroll]')?.scrollTop || 0;
    state.pageTransition=modal.body.dataset.createPage !== String(state.page);
    modal.body.dataset.createPage=String(state.page);
    if(keepPicker)paintSelection();else modal.body.innerHTML=createWorkspaceWizardBodyHtml(state);
    wire();
    restoreWizardFocus(modal.body,focus);
    modal.body.querySelector('[data-create-scroll]').scrollTop=scrollTop;
  }
  function paintSelection() {
    const template=document.createElement('template');template.innerHTML=createWorkspaceWizardBodyHtml(state);
    modal.body.querySelector('#create-advanced-body')?.remove();
    modal.body.querySelector('.create-workspace-bottom').replaceWith(template.content.querySelector('.create-workspace-bottom'));
    const targets=wizardTargets(state).flatMap(section=>section.targets);
    modal.body.querySelectorAll('[data-create-target]').forEach(button=>{
      const selected=sameTarget(targets[Number(button.dataset.createTarget)],state.target);
      button.setAttribute('aria-pressed',String(selected));button.querySelector('.create-selected').textContent=selected ? '✓':'';
    });
  }
  function changeAddedDevice(deviceId) {
    if(!state.target || state.target.deviceId===deviceId)return;
    state.addedSources.forEach(source=>{if(source.kind==='path')source.path='';});
  }
  function selectTarget(target) {
    if(target.kind==='new-environment' && !validEnvironmentDraft())return;
    if(!targetSupportsDurableCreation(snapshot,target)){
      if(target.kind==='project')void handoffLegacy(target);
      else {state.error='This device needs an update to create a project in this environment.';paint();}
      return;
    }
    changeAddedDevice(target.deviceId);state.target=target;state.page=1;state.error='';updateAdvanced();save();paint(true);
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
    const back=modal.body.querySelector('[data-create-back]');if(back)back.onclick=()=>{state.page=1;updateModel();save();paint();focusPage();};
    const next=modal.body.querySelector('[data-create-next]');if(next)next.onclick=nextPage;
    const create=modal.body.querySelector('[data-create-go]');if(create)create.onclick=()=>{void submit();};
    const resume=modal.body.querySelector('[data-create-resume]');if(resume)resume.onclick=()=>{void showProgress(state.operationId);};
  }
  function wireFolders() {
    const fail=error=>{state.error=error;paint();};
    if(state.page===2) {wireEnvironmentDraft(modal.body,state,{save,paint,fail});return;}
    const advanced=modal.body.querySelector('[data-create-advanced]');
    if(!advanced)return;
    wireWizardAdvanced(modal.body,expanded=>{state.advancedExpanded=expanded;save();},state.busy);
    if(!state.target || !state.showAdvanced)return;
    wireFolderDrafts(modal.body,{sources:state.addedSources,deviceId:state.target.deviceId,changed:()=>{save();modal.body.querySelector('.create-sources').innerHTML=workspaceSourcesHtml(state);},repaint:paint,fail});
  }
  function nextPage() {
    if(!validEnvironmentDraft())return;
    changeAddedDevice(state.environmentDraft.deviceId);state.target=draftEnvironmentTarget(state.environmentDraft,snapshot.devices);state.draftTarget=state.target;state.page=1;updateAdvanced();save();paint();focusPage(true);
  }
  function wirePicker() {
    const targets=wizardTargets(state).flatMap(section=>section.targets);
    modal.body.querySelectorAll('[data-create-target]').forEach(button=>{button.onclick=()=>selectTarget(targets[Number(button.dataset.createTarget)]);});
    const fresh=modal.body.querySelector('[data-create-new-environment]');
    if(fresh)fresh.onclick=()=>{state.page=2;state.error='';save();paint();focusPage();};
    const edit=modal.body.querySelector('[data-create-edit]');if(edit)edit.onclick=()=>fresh?.click();
    const search=modal.body.querySelector('#create-workspace-search');
    if(search)search.oninput=()=>{state.query=search.value;updateModel();paint();};
  }
  function wireName() {
    const input=modal.body.querySelector('#create-work-input');
    if(input)input.oninput=()=>{state.name=input.value;modal.body.querySelector('[data-branch-preview]').textContent=branchNamePreview(state.name);save();};
    const isolation=modal.body.querySelector('#create-work-isolation');if(isolation)isolation.onchange=()=>{state.isolation=isolation.value;save();};
  }
  function validEnvironmentDraft() {
    state.error=environmentDraftError(state.environmentDraft);
    if(!state.error)return true;
    state.page=2;save();paint();focusPage();return false;
  }
  function validSubmission() {
    syncEnvironmentTarget();
    if(state.target.kind==='new-environment' && !validEnvironmentDraft())return false;
    const error=state.showAdvanced ? folderDraftError(state.addedSources):'';
    if(!error)return true;
    state.error=error;state.advancedExpanded=true;paint();return false;
  }
  async function submit() {
    if(submitting || state.operationId) return;
    if(!state.target){state.error='Choose a project or environment.';paint();modal.body.querySelector('#create-workspace-search')?.focus();return;}
    if(!validSubmission())return;
    submitting=true;state.busy=true;state.error='';paint();
    try {
      await draft.write(draftOf(state));
      if(!targetSupportsDurableCreation(snapshot,state.target)) await submitLegacy();
      else await submitDurable();
    }catch(error){if(!detached){state.busy=false;state.error=messageOf(error);refusalPage(error);save();paint();}}
    finally{submitting=false;}
  }
  async function submitLegacy() {
    if(state.target.kind !== 'project') throw new Error('This device needs an update to create environments here');
    const route=await createLegacyWorkspace({target:state.target,name:state.name,isolation:state.isolation});
    await deleteDraft(address);dispose();await modal.close();(options.navigate || go)(route);
  }
  async function submitDurable(continues=state.continues) {
    const attempt=submitCreateWorkspace({...draftOf(state),projectName:optionalProjectName(state,snapshot),continues,draft:draftOf(state)});
    await attempt.ready;
    state.operationId=attempt.operationId;state.busy=false;
    await draft.write(draftOf(state));
    const screen=await showProgress(attempt.operationId);
    void attempt.completion.catch(async error=>{
      if(await attempt.get())return;
      await screen.close();await returnToWizard(error);
    });
  }
  function refusalPage(error) {
    state.page=refusalDraftPage(state.target,error);
    state.advancedExpanded=state.page===1;
  }
  async function returnToWizard(error='',continues=state.operationId) {
    state.error=messageOf(error);refusalPage(error);
    await updateUiState(address,held=>({...held,...draftOf(state),operationId:null,continues}));
    if(continues)await consumeCreateWorkspaceAttempt(state.target.deviceId,continues);
    const next=openCreateWorkspaceWizard({...options,error:state.error});await next.ready;
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
    const nextDraft={...draftOf(state),operationId:null,continues:operationId};
    const attempt=submitCreateWorkspace({...nextDraft,projectName:optionalProjectName(state,snapshot),operationId:crypto.randomUUID(),draft:nextDraft});
    await attempt.ready;
    state.operationId=attempt.operationId;state.continues=operationId;
    await updateUiState(address,()=>draftOf(state));
    await consumeCreateWorkspaceAttempt(state.target.deviceId,operationId);
    const screen=openProgress(attempt.operationId);
    void attempt.completion.catch(async error=>{
      if(await attempt.get())return;
      await screen.close();await returnToWizard(error,operationId);
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
      onBack:payload=>returnToWizard(payload?.record?.confirmation?.error || payload?.record?.error || ''),onRetryWorkspace:retryWorkspace,
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
