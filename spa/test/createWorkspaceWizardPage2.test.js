// @vitest-environment jsdom
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { IDBFactory, IDBKeyRange } from 'fake-indexeddb';
const mocks=vi.hoisted(()=>({read:vi.fn(),submit:vi.fn(),progress:vi.fn(),call:vi.fn(),consume:vi.fn()}));
vi.mock('../src/app.js',()=>({go:vi.fn()}));
vi.mock('../src/core/inboxDevices.js',()=>({deviceCall:()=>mocks.call}));
vi.mock('../src/core/createWorkspaceTargets.js',()=>({readCreateWorkspaceTargets:mocks.read,followCreateWorkspaceTargets:()=>()=>{},submitCreateWorkspace:mocks.submit,taskCountText:()=> '0 tasks',createLegacyWorkspace:vi.fn(),cacheCreatedResources:vi.fn(),createdWorkspaceRoute:vi.fn(),discardCreatedProject:vi.fn(),discardCreatedEnvironment:vi.fn(),consumeCreateWorkspaceAttempt:mocks.consume}));
vi.mock('../src/core/createWorkspaceProgress.js',()=>({openCreateWorkspaceProgress:mocks.progress}));
vi.mock('../src/core/motion.js',()=>({reveal:async el=>{el.hidden=false;},hide:async el=>{el.hidden=true;},settleHidden:()=>{}}));
const {openCreateWorkspaceWizard}=await import('../src/core/createWorkspaceWizard.js');
const {wipeUiRecords,readUiRecord}=await import('../src/core/localUiStore.js');
const {wipeCache}=await import('../src/core/localCache.js');
const {uiAddress}=await import('../src/core/localUiState.js');
let screen;
const snapshot=()=>({devices:[{id:'d1',name:'Laptop'}],projects:[],environments:[],workspaces:[],support:{d1:true},features:{d1:{create:true,createProject:true,addedSources:true}}});
const query=s=>document.querySelector(s);
const click=s=>query(s).click();
const input=(s,value)=>{query(s).value=value;query(s).dispatchEvent(new Event('input'));};
const open=async(options={})=>{screen=openCreateWorkspaceWizard(options);await screen.ready;};
beforeEach(async()=>{
 globalThis.indexedDB=new IDBFactory();globalThis.IDBKeyRange=IDBKeyRange;await wipeUiRecords();await wipeCache();document.body.innerHTML='';vi.clearAllMocks();
 mocks.read.mockResolvedValue(snapshot());mocks.call.mockResolvedValue({path:'/code',parent:'/',entries:[{path:'/code/api',name:'api',is_git:true}]});
 mocks.progress.mockReturnValue({ready:Promise.resolve(),close:vi.fn()});mocks.submit.mockReturnValue({operationId:'one',ready:Promise.resolve(),completion:Promise.resolve(),get:async()=>({})});
});
afterEach(async()=>{await screen?.close();screen=null;});
it('browses with fs.list only, keeps Back drafts and submits once only on Create',async()=>{
 await open();click('[data-create-new-environment]');expect(query('h3').textContent).toBe('New environment');
 click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');
 click('[data-folder-add="path"]');await vi.waitFor(()=>expect(query('.use')).not.toBeNull());click('.use');
 expect(mocks.call.mock.calls.map(([method])=>method)).toEqual(['fs.list']);expect(mocks.submit).not.toHaveBeenCalled();
 click('[data-create-next]');expect(query('h3').textContent).toContain('api');expect(document.body.textContent).toContain('(new environment)');
 input('#create-work-input','Ship');click('[data-create-back]');expect(query('[data-folder-value]').value).toContain('github.com');
 click('[data-create-next]');click('[data-create-go]');click('[data-create-go]');await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(1));
 expect(mocks.submit.mock.calls[0][0]).toMatchObject({target:{kind:'new-environment',deviceId:'d1'},name:'Ship',environmentDraft:{sources:[{remote:'https://github.com/org/api.git'},{path:'/code/api'}]}});
});
it('saves page 2 in build-ui and validates URL shape locally without requests',async()=>{
 await open({newEnvironment:true,deviceId:'d1'});click('[data-folder-add="remote"]');input('[data-folder-value]','not a url');click('[data-create-next]');
 expect(query('h3').textContent).toBe('New environment');expect(document.body.textContent).toContain('valid Git remote URL');expect(mocks.call).not.toHaveBeenCalled();
 input('[data-folder-value]','git@github.com:org/api.git');await screen.flushDraft();await screen.detach();screen=null;
 await open({newEnvironment:true,deviceId:'d1'});expect(query('[data-folder-value]').value).toBe('git@github.com:org/api.git');
 expect((await readUiRecord(uiAddress({deviceId:'d1',view:'create-workspace',kind:'draft'}))).value.page).toBe(2);
});
it('retains additional folders in the preselected expanded Advanced section',async()=>{
 const held=snapshot();held.environments=[{id:'e1',deviceId:'d1',name:'Sources',sources:[{path:'/code/api'}]}];mocks.read.mockResolvedValue(held);
 await open({environmentId:'e1',deviceId:'d1',advancedExpanded:true,fixedProject:false});expect(query('[data-create-advanced]').open).toBe(true);
 click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/extra.git');input('#create-work-input','Board');click('[data-create-go]');
 await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalled());expect(mocks.submit.mock.calls[0][0].addedSources).toMatchObject([{remote:'https://github.com/org/extra.git'}]);
});
it('returns source admission refusals to page 2 with all drafts intact',async()=>{
 mocks.submit.mockImplementation(()=>{throw new Error('source overlaps a registered folder');});
 await open({newEnvironment:true,deviceId:'d1'});click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');input('#create-work-input','Ship');click('[data-create-go]');
 await vi.waitFor(()=>expect(query('h3').textContent).toBe('New environment'));expect(query('[data-folder-value]').value).toContain('api.git');expect(document.body.textContent).toContain('source overlaps');
});
it('tracks new-environment writes as durable creation and requires their own capability',async()=>{
 const {isCreationWrite,creationWritesSupported}=await import('../src/core/writeOperationProgress.js');
 expect(isCreationWrite('environment.create')).toBe(true);
 expect(creationWritesSupported({capabilities:['environments']},'environment.create')).toBe(false);
 expect(creationWritesSupported({capabilities:['environments.create']},'environment.create')).toBe(true);
});
it('resumes only the landed environment with retained project drafts and a fresh predecessor link',async()=>{
 await open({newEnvironment:true,deviceId:'d1'});click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');input('#create-work-input','Ship');
 click('[data-create-advanced] summary');click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/extra.git');click('[data-create-go]');
 await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalled());
 mocks.submit.mockReturnValue({operationId:'two',ready:Promise.resolve(),completion:Promise.resolve(),get:async()=>({})});
 await mocks.progress.mock.calls[0][0].onResumeEnvironment({environment:{environment_id:'landed',name:'API',sources:[{path:'/code/api'}]},operationId:'one'});
 expect(mocks.submit.mock.calls[1][0]).toMatchObject({target:{id:'landed',kind:'environment'},name:'Ship',continues:'one',addedSources:[{remote:'https://github.com/org/extra.git'}]});
 expect(mocks.progress.mock.calls[1][0].operationId).toBe('two');expect(mocks.consume).toHaveBeenCalledWith('d1','one');
});
it('hides Advanced in fixed-project mode',async()=>{
 const held=snapshot();held.projects=[{id:'p1',deviceId:'d1',name:'Board',sources:[{path:'/existing'}]}];mocks.read.mockResolvedValue(held);
 await open({projectId:'p1',deviceId:'d1'});expect(query('[data-create-advanced]')).toBeNull();expect(document.body.textContent).toContain('/existing');
});
it('includes New environment in picker arrow navigation',async()=>{
 const held=snapshot();held.environments=[{id:'e1',deviceId:'d1',name:'Sources',sources:[]}];mocks.read.mockResolvedValue(held);
 await open();query('[data-create-target]').focus();document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true}));
 expect(document.activeElement).toBe(query('[data-create-new-environment]'));
});

it.each([{create:true,createProject:false},{create:false,createProject:true}])('opens a usable wizard from partial cached support %j',async features=>{
 const held=snapshot();held.features.d1=features;held.support.d1=features.createProject;held.environments=[{id:'e1',deviceId:'d1',name:'Sources',sources:[]}];mocks.read.mockResolvedValue(held);
 await open({deviceId:'d1',...(features.create?{newEnvironment:true}:{environmentId:'e1',fixedProject:false})});
 expect(query('h3').textContent).toBe(features.create?'New environment':'New workspace in Sources');
});
it.each([
 'managed source destination overlaps a project or workspace location',
 'source overlaps a registered project source: /canonical/actual/extra'
])('routes structured added-source refusals without matching literal input: %s',async message=>{
 mocks.submit.mockImplementation(()=>{throw Object.assign(new Error(message),{details:{creation:{step:'sources',source_scope:'project'}}});});
 await open({newEnvironment:true,deviceId:'d1'});click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');input('#create-work-input','Ship');
 mocks.call.mockResolvedValueOnce({path:'/links',parent:'/',entries:[{path:'/links/extra',name:'extra',is_git:true}]});
 click('[data-create-advanced] summary');click('[data-folder-add="path"]');await vi.waitFor(()=>expect(query('.use')).not.toBeNull());click('.use');click('[data-create-go]');
 await vi.waitFor(()=>expect(document.body.textContent).toContain(message));expect(query('[data-create-advanced]')?.open).toBe(true);expect(document.body.textContent).toContain('/links/extra');
 expect(query('#create-work-input').value).toBe('Ship');expect(document.body.textContent).toContain(message);
});
it('retains structured source ownership when returning from a terminal refusal',async()=>{
 await open({newEnvironment:true,deviceId:'d1'});click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');input('#create-work-input','Ship');
 click('[data-create-advanced] summary');click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/extra.git');click('[data-create-go]');
 await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalled());
 await mocks.progress.mock.calls[0][0].onBack({record:{confirmation:{error:{message:'could not clone repository: authentication failed',details:{creation:{step:'sources',source_scope:'project'}}}}}});
 expect(query('[data-create-advanced]')?.open).toBe(true);expect(query('[data-folder-value]').value).toContain('extra.git');
});
it('routes an environment-source refusal to page 2 even when its message mentions a workspace',async()=>{
 mocks.submit.mockImplementation(()=>{throw Object.assign(new Error('managed source destination overlaps a project or workspace location'),{details:{creation:{step:'sources',source_scope:'environment'}}});});
 await open({newEnvironment:true,deviceId:'d1'});click('[data-folder-add="remote"]');input('[data-folder-value]','https://github.com/org/api.git');click('[data-create-next]');input('#create-work-input','Ship');click('[data-create-go]');
 await vi.waitFor(()=>expect(query('h3').textContent).toBe('New environment'));expect(query('[data-folder-value]').value).toContain('api.git');
});
