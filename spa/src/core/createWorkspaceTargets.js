// The wizard's only boundary to environment/support plumbing and device writes.
// Cached support selects the legacy flow without consulting connection state.
import { sourceDraftParams } from './sourceDraft.js';
import { bridgeCapabilities } from './changeEvents.js';
import { App } from '../appState.js';
import { readCached, subscribeCache, mergeCachedAtomically } from './localCache.js';
import { readUiRecord, subscribeUiRecords } from './localUiStore.js';
import { uiAddress } from './localUiState.js';
import { deviceWriteOperations } from './writeOperationRuntime.js';
import { deviceCall } from './inboxDevices.js';
import { stampProject, stampWorkspace } from './feedMerge.js';
import { upsertSessionRow } from './sessionListCache.js';
import { workspaceRoute } from './projectModel.js';
import { isCreationWrite, creationSupportForMethod, creationMethodForTarget, creationMethodForRecovery } from './writeOperationProgress.js';
import { readEnvironmentSupport } from './environmentSupport.js';
import { readCachedEnvironments } from './environmentCache.js';
import { taskCountText } from './taskCountText.js';

export { taskCountText };
const visitsAddress = uiAddress({view:'navigation',kind:'history',sub:'visits'});
const listAddress = (deviceId,kind) => ({deviceId,entityId:'',kind});
const cachedList = async (deviceId,kind) => (await readCached(listAddress(deviceId,kind)))?.value || [];
async function readDeviceTargets(device) {
  const [projects,environments,workspaces,support,attempts] = await Promise.all([
    cachedList(device.id,'projects'),readCachedEnvironments(device.id),cachedList(device.id,'workspaces'),readEnvironmentSupport(device.id),deviceWriteOperations(device.id)?.list() || [],
  ]);
  return { projects:projects.map(row=>stampProject(row,device.id)),
    environments:environments.map(row=>({...row,id:row.environment_id || row.id,deviceId:device.id})),
    workspaces:workspaces.map(row=>stampWorkspace(row,device.id)), environmentsSupported:creationSupportForMethod(support,"environment.create_project"),features:support,attempts:attempts.filter(recoverableCreation) };
}
// A result is still recoverable until the user explicitly consumes it. Closing
// the screen is not consumption: the operation may have finished off-screen.
function recoverableCreation(row) {
  return isCreationWrite(row.request?.method) && !row.dismissed && Boolean(row.draft?.target);
}
export async function consumeCreateWorkspaceAttempt(deviceId,operationId) {
  const service=deviceWriteOperations(deviceId);
  if(await service?.get(operationId))await service.dismiss(operationId);
}
export async function readCreateWorkspaceTargets() {
  const devices = App.devices || [];
  const rows = await Promise.all(devices.map(readDeviceTargets));
  return { devices, projects:rows.flatMap(row=>row.projects), environments:rows.flatMap(row=>row.environments),
    workspaces:rows.flatMap(row=>row.workspaces), support:Object.fromEntries(rows.map((row,index)=>[devices[index].id,row.environmentsSupported])),
    features:Object.fromEntries(rows.map((row,index)=>[devices[index].id,row.features])),
    visits:(await readUiRecord(visitsAddress))?.value || [],attempts:rows.flatMap(row=>row.attempts) };
}
export function followCreateWorkspaceTargets(listener) {
  const stop = subscribeCache({},listener);
  const stopVisits = subscribeUiRecords(visitsAddress,listener);
  return ()=>{stop();stopVisits();};
}
export function workspaceCreateParams({target,name='',isolation='',continues}) {
  return {project_id:target.id,name,...(isolation ? {isolation}:{}),...(continues ? {continues}:{})};
}
function creationRequest(input) {
  const method=creationMethodForTarget(input.target);
  if(!method)throw new Error('Unknown creation target');
  if(input.target.kind === 'project') return {method,params:workspaceCreateParams(input)};
  const {target,name,isolation,continues}=input;
  const added = input.addedSources?.length ? {added_sources:input.addedSources.map(sourceDraftParams)} : {};
  const project = {name,workspace:{name,...(isolation ? {isolation}:{})},...added};
  const continuation = continues ? {continues} : {};
  if(target.kind !== 'new-environment') return {method,params:{environment_id:target.id,...project,...continuation}};
  const environment = input.environmentDraft;
  return {method,params:{name:environment.name.trim() || target.name,sources:environment.sources.map(sourceDraftParams),
    ...(environment.isolation ? {isolation:environment.isolation}:{}),project,...continuation}};
}
export function submitCreateWorkspace(input) {
  const service = deviceWriteOperations(input.target.deviceId);
  if(!service) throw new Error('Sign in before creating a workspace.');
  const operationId = input.operationId || crypto.randomUUID();
  const completion = service.submit({...creationRequest(input),operationId,draft:input.draft || input});
  // The screen is mounted asynchronously; retain early admission refusals.
  void completion.catch(()=>{});
  return {operationId,completion,ready:createdIntentReady(service,operationId,completion),get:()=>service.get(operationId)};
}
function createdIntentReady(service,operationId,completion) {
  let stop=()=>{},settled=false;
  const ready=new Promise((resolve,reject)=>{
    const finish=(error,row)=>{if(settled)return;settled=true;stop();if(error)reject(error);else resolve(row);};
    const found=row=>{if(row)finish(null,row);};
    stop=service.subscribe(rows=>found(rows.find(row=>row.operationId===operationId)));
    if(settled)stop();
    void service.get(operationId).then(found,error=>finish(error));
    void completion.then(async()=>{
      const row=await service.get(operationId);
      if(row)found(row);else finish(new Error('The creation attempt could not be retained.'));
    },error=>finish(error)).catch(error=>finish(error));
  });
  void ready.catch(()=>{});
  return ready;
}
export async function cacheCreatedWorkspace(deviceId,answer) {
  const workspace=answer?.workspace || answer;
  await upsertSessionRow(listAddress(deviceId,'workspaces'),'workspaces',stampWorkspace(workspace,deviceId));
}
export async function cacheCreatedResources(deviceId,answer) {
  if(answer?.project) await upsertSessionRow(listAddress(deviceId,'projects'),'projects',stampProject(answer.project,deviceId));
  if(answer?.environment) await mergeCachedAtomically(listAddress(deviceId,'environments'),held=>{
    const rows=Array.isArray(held) ? held:[];
    const id=answer.environment.environment_id || answer.environment.id;
    return [...rows.filter(row=>(row.environment_id || row.id)!==id),answer.environment];
  });
  if(hasWorkspaceResult(answer)) await cacheCreatedWorkspace(deviceId,answer);
}
export function createdWorkspaceRoute(deviceId,answer) {
  const workspace=answer?.workspace || answer;
  if(workspace?.status === 'failed') throw new Error(workspace.directories?.find(row=>row.error)?.error || 'Workspace creation failed.');
  const route=workspace && workspaceRoute(workspace);
  if(!route) throw new Error('The workspace was created but returned no usable identity.');
  return {...route,deviceId};
}
export async function createLegacyWorkspace(input) {
  const answer=await deviceCall(input.target.deviceId,'create a workspace')('workspace.create',workspaceCreateParams(input));
  const route=createdWorkspaceRoute(input.target.deviceId,answer);
  await cacheCreatedWorkspace(input.target.deviceId,answer);
  return route;
}
export async function discardCreatedProject({target,project,operationId}) {
  const projectId=project.project_id || project.id;
  const method=requireRecoveryMethod(target.deviceId,'discard');
  await deviceCall(target.deviceId,'discard this project')(method,{project_id:projectId,creation_operation_id:operationId});
  await mergeCachedAtomically(listAddress(target.deviceId,'projects'),held=>(held || []).filter(row=>(row.project_id || row.id)!==projectId));
}

export async function discardCreatedEnvironment({target,environment,operationId}) {
  const environmentId=environment.environment_id || environment.id;
  const method=requireRecoveryMethod(target.deviceId,'discardEnvironment');
  await deviceCall(target.deviceId,'discard the environment')(method,{environment_id:environmentId,creation_operation_id:operationId});
  await mergeCachedAtomically(listAddress(target.deviceId,'environments'),held=>(held || []).filter(row=>(row.environment_id || row.id)!==environmentId));
}

function hasWorkspaceResult(answer) {
  return answer?.workspace || (!answer?.environment && !answer?.project);
}

function requireRecoveryMethod(deviceId,action) {
  const method=creationMethodForRecovery(action);
  if(!creationSupportForMethod(bridgeCapabilities(deviceId).environments,method))throw new Error('Update Build on this device to use this recovery action.');
  return method;
}
