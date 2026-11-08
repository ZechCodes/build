// @vitest-environment jsdom
import {beforeEach,afterEach,it,expect,vi} from 'vitest';
import {IDBFactory,IDBKeyRange} from 'fake-indexeddb';
const mocks=vi.hoisted(()=>({service:null,liveFlags:{},progress:vi.fn(),legacy:vi.fn(),navigate:vi.fn()}));
vi.mock('../src/core/changeEvents.js',async original=>({...await original(),bridgeCapabilities:()=>({environments:mocks.liveFlags})}));
vi.mock('../src/app.js',()=>({go:mocks.navigate}));
vi.mock('../src/core/writeOperationRuntime.js',async original=>({...await original(),deviceWriteOperations:()=>mocks.service}));
vi.mock('../src/core/inboxDevices.js',()=>({deviceCall:()=>mocks.legacy}));
vi.mock('../src/core/createWorkspaceProgress.js',()=>({openCreateWorkspaceProgress:mocks.progress}));
vi.mock('../src/core/motion.js',()=>({reveal:async el=>{el.hidden=false;},hide:async el=>{el.hidden=true;},settleHidden:()=>{}}));
import {App} from '../src/appState.js';
import {openCreateWorkspaceWizard} from '../src/core/createWorkspaceWizard.js';
import {readCreateWorkspaceTargets} from '../src/core/createWorkspaceTargets.js';
import {createWorkspaceWizardModel} from '../src/core/createWorkspaceWizardModel.js';
import {createWriteOperationCache,wipePendingOperations} from '../src/core/writeOperationCache.js';
import {createWriteOperationService} from '../src/core/writeOperations.js';
import {rememberEnvironmentSupport} from '../src/core/environmentSupport.js';
import {writeCached,wipeCache} from '../src/core/localCache.js';
import {wipeUiRecords} from '../src/core/localUiStore.js';
const identity={accountId:'integration',deviceId:'d1',principalId:'user'};
const combinations=[{create:true,createProject:false},{create:false,createProject:true},{create:true,createProject:true},{create:false,createProject:false}];
let screen,cache,raw,greeting;
let progressScreens=[];
const query=selector=>document.querySelector(selector);
const click=selector=>query(selector).click();
const input=(selector,value)=>{query(selector).value=value;query(selector).dispatchEvent(new Event('input'));};
const open=async options=>{screen=openCreateWorkspaceWizard(options);await screen.ready;};
beforeEach(async()=>{
 globalThis.indexedDB=new IDBFactory();globalThis.IDBKeyRange=IDBKeyRange;sessionStorage.clear();await wipeCache();await wipeUiRecords();await wipePendingOperations();document.body.innerHTML='';vi.clearAllMocks();
 progressScreens=[];App.devices=[{id:'d1',name:'Laptop'}];mocks.progress.mockReturnValue({ready:Promise.resolve(),close:vi.fn()});
 mocks.legacy.mockResolvedValue({workspace_id:'legacy-workspace',project_id:'p1',status:'ready'});
 await writeCached({deviceId:'d1',entityId:'',kind:'projects'},[{project_id:'p1',name:'Board',environment_id:'e1',sources:[{path:'/code/api'}]}]);
 await writeCached({deviceId:'d1',entityId:'',kind:'environments'},[{environment_id:'e1',name:'Sources',sources:[{path:'/code/api'}],project_ids:['p1']}]);
});
afterEach(async()=>{const cancel=query('[data-create-cancel]');if(cancel&&!cancel.disabled){cancel.click();await vi.waitFor(()=>expect(query('[data-create-cancel]')).toBeNull());}await Promise.all(progressScreens.map(progress=>progress.close()));await screen?.close();screen=null;await mocks.service?.close();mocks.service=null;});
async function bind(flags,refusal){
 mocks.liveFlags=flags;
 await rememberEnvironmentSupport('d1',{environments:{list:true,...flags,addedSources:true}});
 greeting={bridge_epoch:'epoch',journal_incarnation:'journal',capabilities:['bridge.epoch',...(flags.create?['environments.create']:[]),...(flags.createProject?['environments']:[])],ack_confirm_methods:['environment.create','environment.create_project','workspace.create']};
 raw=vi.fn(async(method,_params,options)=>{
  if(method.startsWith('writes.'))return {journal_incarnation:'journal',operations:[]};
  if(refusal)throw refusal;
  return {ok:true,bridge_epoch:'epoch',write:{phase:'ack',state:'accepted',revision:1,operation_id:options.write.operation_id}};
 });
 await startService();
}
async function startService(){
 cache=createWriteOperationCache(identity);mocks.service=createWriteOperationService({...identity,cache});
 await mocks.service.bind({session:{callRaw:raw,onWriteFrame:()=>()=>{}},greeting,active:()=>true});
}
async function expectDurable(method){
 await vi.waitFor(()=>expect(raw.mock.calls.filter(([name])=>name===method)).toHaveLength(1));
 const operationId=raw.mock.calls.find(([name])=>name===method)[2].write.operation_id;
 const row=await cache.get(operationId);expect(row).toBeDefined();expect(row.request.method).toBe(method);
 const progress=[{step:method==='environment.create'?'environment':'workspace',state:'running'}];
 await mocks.service.handleFrame({type:'writes.progress',bridge_epoch:'epoch',operation_id:row.operationId,progress});
 expect((await cache.get(row.operationId)).progress).toEqual(progress);
 await mocks.service.handleFrame({type:'writes.confirmed',bridge_epoch:'epoch',operation_id:row.operationId,state:'succeeded',revision:2,result:{workspace:{id:'created-workspace',project_id:'p1'}}});
 expect((await cache.get(row.operationId)).state).toBe('succeeded');
}
it.each(combinations)('offers and submits a usable creation for partial support %j',async flags=>{
 await bind(flags);
 const snapshot=await readCreateWorkspaceTargets();const model=createWorkspaceWizardModel(snapshot);
 const environments=model.sections.filter(section=>section.kind==='environments').flatMap(section=>section.targets);
 expect(environments).toHaveLength(flags.createProject?1:0);
 expect(model.newEnvironmentDevices).toHaveLength(flags.create?1:0);
 if(flags.create){
  await open({deviceId:'d1',newEnvironment:true});click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');input('#create-work-input','Ship');click('[data-create-go]');await expectDurable('environment.create');
 }else if(flags.createProject){
  await open({deviceId:'d1',environmentId:'e1',fixedProject:false});input('#create-work-input','Ship');click('[data-create-go]');await expectDurable('environment.create_project');
 }else{
  await open({deviceId:'d1',projectId:'p1'});input('#create-work-input','Ship');click('[data-create-go]');await vi.waitFor(()=>expect(mocks.legacy).toHaveBeenCalledWith('workspace.create',expect.objectContaining({project_id:'p1'})));
 }
});
it.each(combinations)('submits a fixed-project workspace through its own supported path for %j',async flags=>{
 await bind(flags);await open({deviceId:'d1',projectId:'p1',fixedProject:true});input('#create-work-input','Ship');click('[data-create-go]');
 if(flags.createProject)await expectDurable('workspace.create');
 else {await vi.waitFor(()=>expect(mocks.legacy).toHaveBeenCalledWith('workspace.create',expect.objectContaining({project_id:'p1'})));expect(raw.mock.calls.filter(([name])=>name==='workspace.create')).toHaveLength(0);}
});
it.each(combinations.filter(flags=>flags.createProject))('submits the offered existing-environment target for %j',async flags=>{
 await bind(flags);await open({deviceId:'d1',fixedProject:false});click('[data-create-target][data-kind="environment"]');input('#create-work-input','Ship');click('[data-create-go]');await expectDurable('environment.create_project');
});
it('retains bounded admission source details in the rejected promise, durable cache and reloaded wizard',async()=>{
 const creation={step:'sources',source_scope:'project'};
 const refusal=Object.assign(new Error('managed source destination overlaps a project or workspace location'),{error_code:'internal',retryable:false,details:{creation:{...creation,private_payload:'discard me'},unrelated:'x'.repeat(100000)}});
 await bind({create:true,createProject:true},refusal);const submit=vi.spyOn(mocks.service,'submit');
 await open({deviceId:'d1',newEnvironment:true});click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');input('#create-work-input','Ship');
 click('[data-create-advanced] summary');click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/extra.git');click('[data-create-go]');
 await vi.waitFor(()=>expect(submit).toHaveBeenCalledTimes(1));
 await expect(submit.mock.results[0].value).rejects.toMatchObject({details:{creation}});
 await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalled());
 const oldId=mocks.progress.mock.calls.at(-1)[0].operationId;
 expect((await cache.get(oldId)).confirmation.error.details).toEqual({creation});
 await screen.close();screen=null;await mocks.service.close();await startService();
 const reloaded=await cache.get(oldId);expect(reloaded.confirmation.error.details).toEqual({creation});
 await open({deviceId:'d1',newEnvironment:true});
 await mocks.progress.mock.calls.at(-1)[0].onBack({record:reloaded,operationId:oldId});
 expect(query('[data-create-advanced]')?.open).toBe(true);expect(query('#create-work-input').value).toBe('Ship');expect(query('[data-folder-value]').value).toContain('extra.git');
});

async function failedCreation(flags, landed, initialMethod='environment.create') {
 await bind({create:true,createProject:true});
 const options=initialMethod==='environment.create' ? {deviceId:'d1',newEnvironment:true}
  : initialMethod==='workspace.create' ? {deviceId:'d1',projectId:'p1',fixedProject:true}
  : {deviceId:'d1',environmentId:'e1',fixedProject:false};
 await open(options);
 if(initialMethod==='environment.create'){click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');}
 input('#create-work-input','Ship');click('[data-create-go]');
 await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalled());
 const oldId=mocks.progress.mock.calls.at(-1)[0].operationId;
 const environment={environment_id:'landed-env',name:'API',sources:[{path:'/code/api'}],project_ids:[]};
 const project={project_id:'landed-project',name:'Ship',environment_id:'landed-env',environment_name:'API',sources:[{path:'/code/api'}]};
 const details=landed==='environment'?{environment}:landed==='project'?{environment,project}:{};
 await mocks.service.handleFrame({type:'writes.confirmed',bridge_epoch:'epoch',operation_id:oldId,revision:2,state:'failed',effect:landed==='none'?'unapplied':'partial',error:{code:'creation_failed',message:'recorded creation failure',details}});
 await screen.close();screen=null;await mocks.service.close();await bind(flags);
 const {openCreateWorkspaceProgress}=await vi.importActual('../src/core/createWorkspaceProgress.js');
 mocks.progress.mockImplementation(options=>{const progress=openCreateWorkspaceProgress({...options,autoOpenDelay:0});progressScreens.push(progress);return progress;});
 await open(options);await progressScreens.at(-1).ready;
 return oldId;
}
const recoveryCases=combinations.flatMap(flags=>['environment','project'].map(landed=>({flags,landed})));
it.each(recoveryCases)('offers and completes only a supported provision continuation for %j',async({flags,landed})=>{
 const oldId=await failedCreation(flags,landed);
 const action=landed==='environment'?'resume':'retry';
 const method=landed==='environment'?'environment.create_project':'workspace.create';
 expect(document.body.textContent).toContain('recorded creation failure');
 const button=query(`[data-progress-action="${action}"]`);
 if(flags.createProject){
  expect(button).not.toBeNull();button.click();await expectDurable(method);
  const next=(await cache.list()).find(row=>row.operationId!==oldId&&row.request.method===method);
  expect(next.request.params.continues).toBe(oldId);expect(next.operationId).not.toBe(oldId);
 }else{
  expect(button).toBeNull();expect(query('[data-progress-action="close"]')).not.toBeNull();
  expect(document.body.textContent).toContain('Update Build');
  click('[data-progress-action="close"]');await vi.waitFor(()=>expect(query('[role="dialog"]')).toBeNull());
  expect((await cache.get(oldId)).dismissed).not.toBe(true);expect((await cache.get(oldId)).state).toBe('failed');
  expect(raw.mock.calls.filter(([name])=>!name.startsWith('writes.'))).toHaveLength(0);
 }
});
it.each(recoveryCases)('offers and completes only a supported discard for %j',async({flags,landed})=>{
 const oldId=await failedCreation(flags,landed);
 const action=landed==='environment'?'discardEnvironment':'discard';
 const method=landed==='environment'?'environment.discard_unused':'project.discard_unused';
 const supported=landed==='environment'?flags.create:flags.createProject;
 const button=query(`[data-progress-action="${action}"]`);
 if(supported){
  expect(button).not.toBeNull();button.click();await vi.waitFor(()=>expect(mocks.legacy).toHaveBeenCalledWith(method,expect.objectContaining({creation_operation_id:oldId})));
  await vi.waitFor(()=>expect(query('[role="dialog"]')).toBeNull());
 }else{
  expect(button).toBeNull();expect(query('[data-progress-action="close"]')).not.toBeNull();
  expect((await cache.get(oldId)).dismissed).not.toBe(true);expect(mocks.legacy).not.toHaveBeenCalled();
 }
});
const backCases=combinations.flatMap(flags=>['environment.create','environment.create_project','workspace.create'].map(method=>({flags,method})));
it.each(backCases)('offers Back and resubmission only when its original method is supported: %j',async({flags,method})=>{
 const oldId=await failedCreation(flags,'none',method);
 const supported=method==='environment.create'?flags.create:flags.createProject;
 const back=query('[data-progress-action="back"]');
 if(supported){
  expect(back).not.toBeNull();back.click();await vi.waitFor(()=>expect(query('#create-work-input, [data-create-next]')).not.toBeNull());
  if(query('[data-create-next]'))click('[data-create-next]');
  click('[data-create-go]');await expectDurable(method);
  const next=(await cache.list()).find(row=>row.operationId!==oldId&&row.request.method===method);expect(next.request.params.continues).toBe(oldId);
 }else{
  expect(back).toBeNull();expect(query('[data-progress-action="close"]')).not.toBeNull();
  expect((await cache.get(oldId)).dismissed).not.toBe(true);expect(document.body.textContent).toContain('recorded creation failure');
 }
});
