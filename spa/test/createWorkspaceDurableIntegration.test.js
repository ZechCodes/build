// @vitest-environment jsdom
import {beforeEach,afterEach,it,expect,vi} from 'vitest';
import {IDBFactory,IDBKeyRange} from 'fake-indexeddb';
const mocks=vi.hoisted(()=>({service:null,progress:vi.fn(),legacy:vi.fn(),navigate:vi.fn()}));
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
const query=selector=>document.querySelector(selector);
const click=selector=>query(selector).click();
const input=(selector,value)=>{query(selector).value=value;query(selector).dispatchEvent(new Event('input'));};
const open=async options=>{screen=openCreateWorkspaceWizard(options);await screen.ready;};
beforeEach(async()=>{
 globalThis.indexedDB=new IDBFactory();globalThis.IDBKeyRange=IDBKeyRange;await wipeCache();await wipeUiRecords();await wipePendingOperations();document.body.innerHTML='';vi.clearAllMocks();
 App.devices=[{id:'d1',name:'Laptop'}];mocks.progress.mockReturnValue({ready:Promise.resolve(),close:vi.fn()});
 mocks.legacy.mockResolvedValue({workspace_id:'legacy-workspace',project_id:'p1',status:'ready'});
 await writeCached({deviceId:'d1',entityId:'',kind:'projects'},[{project_id:'p1',name:'Board',environment_id:'e1',sources:[{path:'/code/api'}]}]);
 await writeCached({deviceId:'d1',entityId:'',kind:'environments'},[{environment_id:'e1',name:'Sources',sources:[{path:'/code/api'}],project_ids:['p1']}]);
});
afterEach(async()=>{await screen?.close();screen=null;await mocks.service?.close();mocks.service=null;});
async function bind(flags,refusal){
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
 const row=(await cache.list()).find(row=>row.request.method===method);expect(row).toBeDefined();
 expect(raw.mock.calls.find(([name])=>name===method)[2].write.operation_id).toBe(row.operationId);
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
