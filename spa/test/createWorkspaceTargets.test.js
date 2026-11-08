// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from 'vitest';
import { IDBFactory, IDBKeyRange } from 'fake-indexeddb';
const mocked=vi.hoisted(()=>({service:{submit:vi.fn(),get:vi.fn(),list:vi.fn()},call:vi.fn()}));
vi.mock('../src/core/writeOperationRuntime.js',()=>({deviceWriteOperations:()=>mocked.service}));
vi.mock('../src/core/inboxDevices.js',()=>({deviceCall:()=>mocked.call}));
const {submitCreateWorkspace,createLegacyWorkspace,cacheCreatedWorkspace,readCreateWorkspaceTargets}=await import('../src/core/createWorkspaceTargets.js');
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
