// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from 'vitest';
import { IDBFactory, IDBKeyRange } from 'fake-indexeddb';
vi.mock('../src/core/changeEvents.js',()=>({bridgeCapabilities:()=>({environments:{}})}));
const mocked=vi.hoisted(()=>({service:{submit:vi.fn(),get:vi.fn(),list:vi.fn(),dismiss:vi.fn(),subscribe:vi.fn(()=>()=>{})},call:vi.fn()}));
vi.mock('../src/core/writeOperationRuntime.js',()=>({deviceWriteOperations:()=>mocked.service}));
vi.mock('../src/core/inboxDevices.js',()=>({deviceCall:()=>mocked.call}));
const {submitCreateWorkspace,createLegacyWorkspace,cacheCreatedWorkspace,readCreateWorkspaceTargets,consumeCreateWorkspaceAttempt}=await import('../src/core/createWorkspaceTargets.js');
const {readCached,writeCached,wipeCache}=await import('../src/core/localCache.js');
const {App}=await import('../src/appState.js');
beforeEach(async()=>{globalThis.indexedDB ||= new IDBFactory();globalThis.IDBKeyRange=IDBKeyRange;await wipeCache();vi.clearAllMocks();App.devices=[{id:'d1',name:'Laptop'}];mocked.service.list.mockResolvedValue([]);mocked.service.submit.mockResolvedValue({});});
it('sends one environment command with a frame operation id, and same-id retries retain it',()=>{
 const input={target:{kind:'environment',id:'e1',deviceId:'d1'},name:'Release',isolation:'rift',operationId:'one'};
 const first=submitCreateWorkspace(input);submitCreateWorkspace(input);
 expect(first.operationId).toBe('one');expect(mocked.service.submit).toHaveBeenCalledTimes(2);
 for(const [request] of mocked.service.submit.mock.calls){expect(request).toMatchObject({operationId:'one',method:'environment.create_project',params:{environment_id:'e1',name:'Release',workspace:{name:'Release',isolation:'rift'}}});expect(request.params).not.toHaveProperty('operation_id');}
});
it('terminal workspace retry stays bound to landed project and names the predecessor',()=>{
 submitCreateWorkspace({target:{kind:'project',id:'p1',deviceId:'d1'},name:'',continues:'prior',operationId:'next'});
 expect(mocked.service.submit).toHaveBeenCalledWith(expect.objectContaining({operationId:'next',method:'workspace.create',params:{project_id:'p1',name:'',continues:'prior'}}));
});
it.each(['  ','../feature: *?🦊','a'.repeat(500)])('preserves exact legacy names %j',async name=>{
 mocked.call.mockResolvedValue({id:'w1',project_id:'p1',status:'ready'});
 await createLegacyWorkspace({target:{id:'p1',deviceId:'d1'},name,isolation:''});expect(mocked.call).toHaveBeenCalledWith('workspace.create',{project_id:'p1',name});
 expect((await readCached({deviceId:'d1',entityId:'',kind:'workspaces'})).value[0].id).toBe('w1');
});
it('refuses a failed workspace result with its directory error',async()=>{
 mocked.call.mockResolvedValue({id:'w1',status:'failed',directories:[{error:'could not copy files'}]});
 await expect(createLegacyWorkspace({target:{id:'p1',deviceId:'d1'}})).rejects.toThrow('could not copy files');
});
it('upserts only the created workspace and retains other cached rows',async()=>{
 await writeCached({deviceId:'d1',entityId:'',kind:'workspaces'},[{id:'other',project_id:'p1'}]);await cacheCreatedWorkspace('d1',{workspace:{workspace_id:'w1',project_id:'p1'}});
 expect((await readCached({deviceId:'d1',entityId:'',kind:'workspaces'})).value.map(x=>x.id)).toEqual(['other','w1']);
});
it('reads the persisted project list for the account devices',async()=>{
 await writeCached({deviceId:'d1',entityId:'',kind:'projects'},[{project_id:'p1',name:'Payments'}]);
 expect((await readCreateWorkspaceTargets()).projects).toMatchObject([{id:'p1',deviceId:'d1',name:'Payments'}]);
});

it('waits for persisted intent before publishing its progress identity',async()=>{
 let announce;mocked.service.subscribe.mockImplementation(listener=>{announce=listener;return ()=>{};});
 mocked.service.get.mockResolvedValue(undefined);mocked.service.submit.mockReturnValue(new Promise(()=>{}));
 const attempt=submitCreateWorkspace({target:{kind:'project',id:'p1',deviceId:'d1'},operationId:'durable'});
 let ready=false;attempt.ready.then(()=>{ready=true;});await Promise.resolve();expect(ready).toBe(false);
 announce([{operationId:'durable',state:'queued'}]);await expect(attempt.ready).resolves.toMatchObject({operationId:'durable',state:'queued'});
});
it('recovers active creation attempts from the durable cache for a fresh wizard',async()=>{
 const pending={operationId:'active',state:'accepted',request:{method:'workspace.create',params:{project_id:'p1'}},draft:{target:{id:'p1',kind:'project',deviceId:'d1'},name:'Retained'}};
 mocked.service.list.mockResolvedValue([pending,{...pending,operationId:'old',state:'succeeded'}]);
 expect((await readCreateWorkspaceTargets()).attempts).toEqual([pending,{...pending,operationId:'old',state:'succeeded'}]);
});

it.each(['succeeded','failed'])('retains unconsumed terminal %s attempts until explicit dismissal',async state=>{
 const attempt={operationId:'finished',state,effect:'partial',request:{method:'environment.create_project'},draft:{target:{id:'e',kind:'environment',deviceId:'d1'},name:'Work'}};
 mocked.service.list.mockResolvedValue([attempt,{...attempt,operationId:'consumed',dismissed:true}]);
 expect((await readCreateWorkspaceTargets()).attempts).toEqual([attempt]);
 mocked.service.get.mockResolvedValue(attempt);await consumeCreateWorkspaceAttempt('d1','finished');expect(mocked.service.dismiss).toHaveBeenCalledWith('finished');
});
it('sends a new environment, project, workspace and added sources as one retained command',()=>{
 const source={kind:'remote',remote:'https://github.com/org/api.git',name:'api',base_branch:'main'};
 submitCreateWorkspace({target:{kind:'new-environment',deviceId:'d1'},environmentDraft:{name:'Sources',sources:[source],isolation:'worktree'},name:'Ship',addedSources:[{...source,remote:'https://github.com/org/extra.git',name:'extra'}],isolation:'rift',operationId:'one'});
 expect(mocked.service.submit).toHaveBeenCalledTimes(1);
 expect(mocked.service.submit).toHaveBeenCalledWith(expect.objectContaining({operationId:'one',method:'environment.create',params:{name:'Sources',sources:[{remote:source.remote,name:'api',base_branch:'main'}],isolation:'worktree',project:{name:'Ship',workspace:{name:'Ship',isolation:'rift'},added_sources:[{remote:'https://github.com/org/extra.git',name:'extra',base_branch:'main'}]}}}));
});
