// @vitest-environment jsdom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { setImmediate as nextTask } from "node:timers/promises";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const mocks = vi.hoisted(() => ({ read: vi.fn(), submit: vi.fn(), legacy: vi.fn(), progress: vi.fn(), navigate: vi.fn(), consume:vi.fn(), cache:vi.fn(), route:vi.fn(), follow:null, service:null }));
vi.mock("../src/core/writeOperationRuntime.js",async original=>({...await original(),deviceWriteOperations:()=>mocks.service}));
vi.mock("../src/app.js", () => ({ go: mocks.navigate }));
vi.mock("../src/core/createWorkspaceTargets.js", () => ({
  readCreateWorkspaceTargets: mocks.read, followCreateWorkspaceTargets: listener => {mocks.follow=listener;return () => {};},
  submitCreateWorkspace: mocks.submit, createLegacyWorkspace: mocks.legacy,
  taskCountText: (open, total) => open > 0 ? `${open} open tasks` : `${total || 0} tasks`,
  cacheCreatedWorkspace: vi.fn(), cacheCreatedResources:mocks.cache, createdWorkspaceRoute:mocks.route, discardCreatedProject: vi.fn(), consumeCreateWorkspaceAttempt:mocks.consume,
}));
vi.mock("../src/core/createWorkspaceProgress.js", () => ({ openCreateWorkspaceProgress: mocks.progress }));
vi.mock("../src/core/motion.js", () => ({
  reveal: async el => { el.hidden = false; }, hide: async el => { el.hidden = true; }, settleHidden: el => { el.hidden = true; },
}));
const { openCreateWorkspaceWizard, createWorkspaceWizardHtml } = await import("../src/core/createWorkspaceWizard.js");
const { readUiRecord, writeUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");
const { uiAddress } = await import("../src/core/localUiState.js");
const snapshot = () => ({
  projects: [{ id:"p1", name:"Payments", deviceId:"d1", environment_id:"e1", sources:[{name:"api",path:"/code/api"}], open_tasks:2,total_tasks:9 }],
  environments:[{id:"e1",name:"Platform",deviceId:"d1",sources:[{name:"api",path:"/code/api"}]}], workspaces:[], visits:[],
  devices:[{id:"d1",name:"Laptop",status:"offline"},{id:"d2",name:"Desktop",status:"online"}], support:{d1:true,d2:true},
});
let opened;
beforeEach(async () => {
  globalThis.indexedDB ||= new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  await wipeUiRecords(); document.body.innerHTML = '<button id="opener">New workspace</button>'; document.querySelector('#opener').focus();
  vi.clearAllMocks(); mocks.read.mockResolvedValue(snapshot()); mocks.progress.mockReturnValue({close:vi.fn(),ready:Promise.resolve()});
  mocks.submit.mockReturnValue({ operationId:'op-one', completion:Promise.resolve(), get:async()=>({state:'accepted'}) });
});
afterEach(async()=>{ await opened?.close(); opened=null; });
const open = async options => { opened=openCreateWorkspaceWizard(options); await opened.ready; return opened; };
const dialog = () => document.querySelector('[role="dialog"]');
const clickTarget = kind => dialog().querySelector(`[data-create-target][data-kind="${kind}"]`).click();
const name = value => { const input=dialog().querySelector('#create-work-input'); input.value=value;input.dispatchEvent(new Event('input')); };

describe('workspace wizard pages',()=>{
  it('lists projects before environments with helpers and accessible device labels',async()=>{
    await open({fixedProject:false});
    expect(dialog().getAttribute('aria-labelledby')).toBe('create-workspace-title');
    expect([...dialog().querySelectorAll('[data-create-section]')].map(el=>el.dataset.createSection)).toEqual(['projects','environments']);
    expect(dialog().textContent).toContain('Uses the existing task board'); expect(dialog().textContent).toContain('Creates a new task board');
    const row=dialog().querySelector('[data-create-target]');expect(row.textContent).toContain('Laptop');expect(row.textContent).toContain('Offline');
    expect(row.querySelector('[aria-hidden="true"]')).not.toBeNull(); expect(mocks.submit).not.toHaveBeenCalled();
  });
  it('selects in place without replacing the focused row or resetting scroll and keeps the selection through search',async()=>{
    await open({fixedProject:false});
    const row=dialog().querySelector('[data-create-target][data-kind="environment"]');row.focus();
    const scroll=dialog().querySelector('[data-create-scroll]');scroll.scrollTop=75;row.click();
    expect(document.activeElement).toBe(row);expect(scroll.scrollTop).toBe(75);
    expect(row.getAttribute('aria-pressed')).toBe('true');expect(row.textContent).toContain('✓');
    expect(dialog().dataset.createPage).toBe('1');expect(dialog().querySelector('[data-create-back]')).toBeNull();
    expect(dialog().querySelector('#create-work-input').required).toBe(false);
    name('Ship payments');expect(dialog().querySelector('[data-branch-preview]').textContent).toContain('build/ship-payments');
    const search=dialog().querySelector('#create-workspace-search');search.value='zzzzz';search.dispatchEvent(new Event('input'));
    expect(dialog().querySelector('[data-create-target]')).toBeNull();
    expect(dialog().querySelector('.create-target-summary').textContent).toContain('Platform');
    expect(dialog().querySelector('.create-target-summary').textContent).toContain('Laptop');
    expect(dialog().querySelector('.create-target-summary').textContent).toContain('Offline');
    expect(dialog().querySelector('#create-work-input').value).toBe('Ship payments');
    search.value='';dialog().querySelector('#create-workspace-search').value='';dialog().querySelector('#create-workspace-search').dispatchEvent(new Event('input'));
    expect(dialog().querySelector('[data-kind="environment"]').getAttribute('aria-pressed')).toBe('true');
    clickTarget('project');expect(dialog().textContent).toContain("Uses Payments's task board");
    expect(mocks.submit).not.toHaveBeenCalled();
  });
  it('supports search focus, arrow navigation, Enter selection, slash search, Escape and opener focus restoration',async()=>{
    await open({fixedProject:false,fromShortcut:true});
    expect(document.activeElement.id).toBe('create-workspace-search');
    dialog().querySelector('[data-create-target]').focus();
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true}));
    expect(document.activeElement.dataset.kind).toBe('environment');
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'/',bubbles:true}));
    expect(document.activeElement.id).toBe('create-workspace-search');
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));expect(mocks.submit).not.toHaveBeenCalled();
    dialog().querySelector('[data-create-target][data-kind="environment"]').focus();document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
    expect(document.activeElement.getAttribute('aria-pressed')).toBe('true');
    name('New board');dialog().querySelector('#create-work-input').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
    await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(1));
    expect(mocks.submit.mock.calls[0][0]).toMatchObject({target:{kind:'environment',id:'e1'},name:'New board'});
    await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledWith(expect.objectContaining({operationId:'op-one',deviceId:'d1'})));
  });
  it('requires a selection, permits blank names for every target and traps Tab including Advanced',async()=>{
    await open({fixedProject:false});dialog().querySelector('[data-create-go]').click();
    expect(dialog().querySelector('.create-error').textContent).toBe('Choose a project or environment.');
    expect(document.activeElement.id).toBe('create-workspace-search');expect(mocks.submit).not.toHaveBeenCalled();
    const summary=dialog().querySelector('summary');expect(summary).not.toBeNull();
    summary.focus();summary.dispatchEvent(new KeyboardEvent('keydown',{key:'/',bubbles:true}));expect(document.activeElement.id).toBe('create-workspace-search');
    const create=dialog().querySelector('[data-create-go]');create.focus();create.dispatchEvent(new KeyboardEvent('keydown',{key:'Tab',bubbles:true}));
    expect(document.activeElement.id).toBe('create-workspace-search');
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Tab',shiftKey:true,bubbles:true}));expect(document.activeElement).toBe(create);
    clickTarget('environment');dialog().querySelector('[data-create-go]').click();await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(1));
    expect(mocks.submit.mock.calls[0][0].name).toBe('Platform');
    await opened.close();await open({projectId:'p1',deviceId:'d1'});
    expect(document.activeElement.id).toBe('create-work-input');
    expect(dialog().querySelector('#create-workspace-search,[data-create-target],[data-create-new-environment],[data-create-back]')).toBeNull();
    dialog().querySelector('[data-create-go]').click();await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(2));
    expect(mocks.submit.mock.calls[1][0].name).toBe('');
  });
  it('restores page-three drafts as page one with target, name, isolation and Advanced intact',async()=>{
    const target={...snapshot().environments[0],kind:'environment',key:'d1/e1'};
    await writeUiRecord(uiAddress({view:'create-workspace',kind:'draft'}),{page:3,target,name:'Retained',isolation:'rift',advancedExpanded:true});
    await open({fixedProject:false});
    expect(dialog().dataset.createPage).toBe('1');expect(dialog().querySelector('#create-work-input').value).toBe('Retained');
    expect(dialog().querySelector('#create-work-isolation').value).toBe('rift');expect(dialog().querySelector('[data-create-advanced]').open).toBe(true);
    expect(dialog().querySelector('[data-kind="environment"]').getAttribute('aria-pressed')).toBe('true');
  });
  it('keeps the old bridge flow current-project scoped and exposes transport refusal',async()=>{
    mocks.read.mockResolvedValue({...snapshot(),support:{}});mocks.legacy.mockRejectedValue(new Error('Device refused the checkout'));
    await open({projectId:'p1',deviceId:'d1'});name('Legacy');dialog().querySelector('[data-create-go]').click();
    await vi.waitFor(()=>expect(dialog().querySelector('.create-error').textContent).toContain('Device refused the checkout'));
    expect(mocks.submit).not.toHaveBeenCalled(); expect(dialog().querySelector('#create-work-input').value).toBe('Legacy');
  });
  it('uses a full-height mobile sheet class and escapes target names',()=>{
    const html=createWorkspaceWizardHtml({page:3,target:{kind:'project',id:'p',label:'<Payments>',name:'<Payments>',sources:[]},name:'',isolation:''});
    expect(html).toContain('create-workspace-sheet');expect(html).toContain('&lt;Payments&gt;');expect(html).not.toContain('<Payments>');
  });
  it('reopens a retained attempt without a second Create and Close keeps its link',async()=>{
    const options={projectId:'p1',deviceId:'d1'};
    await open(options);name('Retained');dialog().querySelector('[data-create-go]').click();
    await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledTimes(1));
    await open(options);expect(dialog().textContent).toContain('Still creating ‹Retained›');
    await opened.close();opened=null;await open(options);
    expect(dialog().querySelector('[data-create-resume]')).not.toBeNull();expect(mocks.submit).toHaveBeenCalledTimes(1);
  });
  it('creates a real workspace continuation under a new operation id instead of reopening the failed record',async()=>{
    const {createWriteOperationCache}=await import('../src/core/writeOperationCache.js');
    const {createWriteOperationService}=await import('../src/core/writeOperations.js');
    const {submitCreateWorkspace}=await vi.importActual('../src/core/createWorkspaceTargets.js');
    const identity={accountId:'retry-account',deviceId:'d1',principalId:'user'};
    const cache=createWriteOperationCache(identity);
    const service=createWriteOperationService({...identity,cache});mocks.service=service;
    const callRaw=vi.fn(async(method,params,options)=>method.startsWith('writes.') ? {journal_incarnation:'journal',operations:[]}
      : {ok:true,bridge_epoch:'epoch',write:{phase:'ack',state:'accepted',revision:1,operation_id:options.write.operation_id}});
    await service.bind({session:{callRaw,onWriteFrame:()=>()=>{}},greeting:{bridge_epoch:'epoch',journal_incarnation:'journal',capabilities:['bridge.epoch','environments'],ack_confirm_methods:['environment.create_project','workspace.create']},active:()=>true});
    mocks.submit.mockImplementation(submitCreateWorkspace);
    try {
      await open({fixedProject:false});clickTarget('environment');name('New board');dialog().querySelector('[data-create-go]').click();
      await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledTimes(1));
      const oldId=mocks.progress.mock.calls[0][0].operationId;
      const project={project_id:'landed',name:'New board',environment_id:'e1'};
      await service.handleFrame({type:'writes.confirmed',bridge_epoch:'epoch',operation_id:oldId,revision:2,state:'failed',effect:'partial',error:{code:'creation_failed',message:'workspace failed',details:{project}}});
      await mocks.progress.mock.calls[0][0].onRetryWorkspace({project,operationId:oldId});
      await vi.waitFor(()=>expect(callRaw.mock.calls.filter(([method])=>method==='workspace.create')).toHaveLength(1));
      const [method,params,options]=callRaw.mock.calls.find(([method])=>method==='workspace.create');
      const nextId=options.write.operation_id;
      expect(nextId).not.toBe(oldId);expect(method).toBe('workspace.create');
      expect(params).toMatchObject({project_id:'landed',continues:oldId});
      expect((await cache.get(oldId)).request.method).toBe('environment.create_project');
      expect((await cache.get(nextId)).request.method).toBe('workspace.create');
      expect(mocks.progress.mock.calls.at(-1)[0].operationId).toBe(nextId);
      expect(mocks.progress.mock.calls.filter(([options])=>options.operationId===oldId)).toHaveLength(1);
      await service.handleFrame({type:'writes.confirmed',bridge_epoch:'epoch',operation_id:nextId,revision:2,state:'succeeded',result:{workspace:{id:'retried-workspace',project_id:'landed'}}});
      expect((await cache.get(nextId)).state).toBe('succeeded');expect((await cache.get(oldId)).state).toBe('failed');
    } finally {await service.close();mocks.service=null;}
  });
  it('returns to the retained draft if durable admission refuses before persisting',async()=>{
    let refuse;const completion=new Promise((resolve,reject)=>{refuse=reject;});
    mocks.submit.mockReturnValue({operationId:'refused',completion,get:async()=>null});
    await open({projectId:'p1',deviceId:'d1'});name('Keep my name');dialog().querySelector('[data-create-go]').click();
    await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledTimes(1));refuse(new Error('Update the bridge first'));
    await vi.waitFor(()=>expect(dialog()?.querySelector('.create-error').textContent).toContain('Update the bridge first'));
    expect(dialog().querySelector('#create-work-input').value).toBe('Keep my name');
    expect(dialog().querySelector('[data-create-go]').disabled).toBe(false);
    dialog().querySelector('[data-create-cancel]').click();await vi.waitFor(()=>expect(dialog()).toBeNull());
  });
  it('derives an environment name for whitespace-only input',async()=>{
    await open({fixedProject:false});clickTarget('environment');name('   ');dialog().querySelector('[data-create-go]').click();
    await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(1));expect(mocks.submit.mock.calls[0][0].name).toBe('Platform');
  });

  it('finds a durable attempt even when no modal draft survived the reload',async()=>{
    mocks.read.mockResolvedValue({...snapshot(),attempts:[{operationId:'saved-attempt',createdAt:123,state:'accepted',request:{method:'workspace.create',params:{project_id:'p1'}},draft:{target:{id:'p1',deviceId:'d1',kind:'project',name:'Payments',key:'d1/p1'},name:'Recovered'}}]});
    await open({projectId:'p1',deviceId:'d1'});expect(dialog().textContent).toContain('Still creating ‹Recovered›');
    dialog().querySelector('[data-create-resume]').click();await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledWith(expect.objectContaining({operationId:'saved-attempt'})));
    expect(mocks.submit).not.toHaveBeenCalled();
  });

  it.each(['succeeded','failed'])('recovers cache-only terminal %s results and consumes them only on an explicit action',async state=>{
    const record={operationId:'finished',createdAt:200,state,effect:state==='failed'?'partial':undefined,request:{method:'environment.create_project'},draft:{target:{id:'e1',deviceId:'d1',kind:'environment',name:'Platform',key:'d1/e1'},name:'Finished'}};
    mocks.read.mockResolvedValue({...snapshot(),attempts:[record]});
    await open({fixedProject:false});expect(mocks.consume).not.toHaveBeenCalled();
    expect(mocks.progress).toHaveBeenCalledTimes(1);expect(dialog()).toBeNull();expect(mocks.submit).not.toHaveBeenCalled();
    const progress=mocks.progress.mock.calls[0][0];expect(progress.operationId).toBe('finished');
    if(state==='failed')await progress.onKeepProject({operationId:'finished',record});
    else await progress.onOpenWorkspace({operationId:'finished',record:{...record,confirmation:{result:{workspace:{id:'w1',project_id:'p1'}}}},workspace:{id:'w1',project_id:'p1'}});
    expect(mocks.consume).toHaveBeenCalledWith('d1','finished');
  });

  it('persists every name, isolation and target action without advancing timers',async()=>{
    await open({fixedProject:false});vi.useFakeTimers({toFake:['setTimeout','clearTimeout']});
    try {
      clickTarget('environment');
      const address=uiAddress({view:'create-workspace',kind:'draft'});
      const persisted=async predicate=>{for(let i=0;i<30;i++){await nextTask();const value=(await readUiRecord(address))?.value;if(predicate(value))return value;}return (await readUiRecord(address))?.value;};
      expect((await persisted(value=>value?.target?.id==='e1'))?.target.id).toBe('e1');
      name('First edit');expect((await persisted(value=>value?.name==='First edit'))?.name).toBe('First edit');
      name('Second edit');expect((await persisted(value=>value?.name==='Second edit'))?.name).toBe('Second edit');
      const isolation=dialog().querySelector('#create-work-isolation');isolation.value='rift';isolation.dispatchEvent(new Event('change'));
      expect((await persisted(value=>value?.isolation==='rift'))?.isolation).toBe('rift');
    } finally {vi.useRealTimers();}
  });

  it('waits for cached capability before mounting any creation markup',async()=>{
    let answer;mocks.read.mockReturnValue(new Promise(resolve=>{answer=resolve;}));
    opened=openCreateWorkspaceWizard({projectId:'p1',deviceId:'d1',projectName:'Original project name'});
    expect(dialog()).toBeNull();
    answer({...snapshot(),support:{}});await opened.ready;
    expect(dialog().querySelector('h3').textContent).toBe('New workspace in Original project name');
    expect(dialog().querySelector('label[for="create-work-input"]').textContent).toBe('Workspace label');
    expect(dialog().querySelector('.create-hint').textContent).toBe('Creates a separate checkout of the project sources.');
    expect(dialog().querySelector('#create-workspace-search,[data-create-target]')).toBeNull();
    expect(dialog().textContent).not.toMatch(/task board|What are you working on|Directories/);
    expect(mocks.read).toHaveBeenCalledTimes(1);
  });

  it('keeps an unpreset old-bridge entry on the original form and refusal',async()=>{
    mocks.read.mockResolvedValue({...snapshot(),support:{}});
    await open({fixedProject:false,projectName:'Original project name'});
    expect(dialog().querySelector('h3').textContent).toBe('New workspace in Original project name');
    expect(dialog().querySelector('#create-workspace-search,[data-create-target]')).toBeNull();
    dialog().querySelector('[data-create-go]').click();
    expect(dialog().querySelector('.create-error').textContent).toBe('No connected project to create in.');
    expect(mocks.submit).not.toHaveBeenCalled();expect(mocks.legacy).not.toHaveBeenCalled();
  });

  it('does not expose the shortcut on a bridge without environments',async()=>{
    mocks.read.mockResolvedValue({...snapshot(),support:{}});
    await open({projectId:'p1',deviceId:'d1',fromShortcut:true,fixedProject:false});
    expect(dialog()).toBeNull();expect(mocks.submit).not.toHaveBeenCalled();expect(mocks.legacy).not.toHaveBeenCalled();
  });

  it('preserves a scoped old bridge when another account device supports environments',async()=>{
    mocks.read.mockResolvedValue({...snapshot(),support:{d1:false,d2:true}});
    await open({projectId:'p1',deviceId:'d1',projectName:'Old machine project'});
    expect(dialog().querySelector('label[for="create-work-input"]').textContent).toBe('Workspace label');
    expect(dialog().querySelector('h3').textContent).toBe('New workspace in Old machine project');
  });

  it.each(['accepted','failed'])('reopens retained %s attempts directly in cached progress after a downgrade',async state=>{
    const record={operationId:'downgraded',createdAt:300,state,effect:state==='failed'?'partial':undefined,request:{method:'environment.create_project'},draft:{target:{id:'e1',deviceId:'d1',kind:'environment',name:'Platform',key:'d1/e1'},name:'Retained work'}};
    mocks.read.mockResolvedValue({...snapshot(),support:{},attempts:[record]});
    await open({fixedProject:false});
    expect(dialog()).toBeNull();expect(mocks.progress).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({operationId:'downgraded',deviceId:'d1'}));
    expect(mocks.submit).not.toHaveBeenCalled();expect(mocks.legacy).not.toHaveBeenCalled();expect(mocks.consume).not.toHaveBeenCalled();
    await mocks.progress.mock.calls[0][0].onKeepProject();expect(mocks.consume).toHaveBeenCalledWith('d1','downgraded');
  });

  it('does not mount after being closed while the initial cached read is pending',async()=>{
    let answer;mocks.read.mockReturnValue(new Promise(resolve=>{answer=resolve;}));
    opened=openCreateWorkspaceWizard({fixedProject:false});await opened.close();
    answer(snapshot());await opened.ready;expect(dialog()).toBeNull();expect(mocks.progress).not.toHaveBeenCalled();
  });

  it('hands an incapable project from a mixed-device picker to the exact legacy form',async()=>{
    const held=snapshot();
    mocks.read.mockResolvedValue({...held,support:{d1:true,d2:false},projects:[...held.projects,{id:'old-p',deviceId:'d2',name:'Old device project',sources:[]}]});
    await open({fixedProject:false});
    const old=[...dialog().querySelectorAll('[data-create-target]')].find(row=>row.textContent.includes('Old device project'));
    old.click();
    await vi.waitFor(()=>expect(dialog()?.querySelector('label[for="create-work-input"]')?.textContent).toBe('Workspace label'));
    expect(dialog().querySelector('h3').textContent).toBe('New workspace in Old device project');
    expect(dialog().textContent).not.toMatch(/task board|What are you working on|Directories/);
    expect(await readUiRecord(uiAddress({view:'create-workspace',kind:'draft'}))).toBeUndefined();
    expect(mocks.submit).not.toHaveBeenCalled();expect(mocks.legacy).not.toHaveBeenCalled();
  });

  it('retains a downgraded cache-only draft before Back restores the legacy form',async()=>{
    const target={id:'p1',deviceId:'d1',kind:'project',name:'Payments',key:'d1/p1'};
    const record={operationId:'downgraded-back',createdAt:301,state:'failed',effect:'unapplied',request:{method:'workspace.create'},draft:{target,name:'Retained work',isolation:'rift'}};
    mocks.read.mockResolvedValue({...snapshot(),support:{},attempts:[record]});
    await open({projectId:'p1',deviceId:'d1',projectName:'Payments'});
    const address=uiAddress({deviceId:'d1',entityId:'p1',view:'create-workspace',kind:'draft'});
    expect((await readUiRecord(address))?.value.name).toBe('Retained work');
    mocks.read.mockResolvedValue({...snapshot(),support:{},attempts:[]});
    await mocks.progress.mock.calls[0][0].onBack();
    expect(dialog().querySelector('label[for="create-work-input"]').textContent).toBe('Workspace label');
    expect(dialog().querySelector('#create-work-input').value).toBe('Retained work');
    expect(dialog().querySelector('#create-work-isolation').value).toBe('rift');
    expect(mocks.consume).toHaveBeenCalledWith('d1','downgraded-back');
    dialog().querySelector('[data-create-cancel]').click();await vi.waitFor(()=>expect(dialog()).toBeNull());
    await vi.waitFor(async()=>expect(await readUiRecord(address)).toBeUndefined());
  });

  it('does not adopt an unsubmitted wizard draft for an unpreset old-bridge entry',async()=>{
    const address=uiAddress({view:'create-workspace',kind:'draft'});
    await writeUiRecord(address,{target:{id:'p1',deviceId:'d1',kind:'project',name:'Payments'},name:'Unsubmitted work',isolation:'rift',page:3});
    mocks.read.mockResolvedValue({...snapshot(),support:{}});
    await open({fixedProject:false});
    expect(dialog().querySelector('h3').textContent).toBe('New workspace in project');
    expect(dialog().querySelector('#create-work-input').value).toBe('');
    expect(dialog().querySelector('#create-work-isolation').value).toBe('');
    dialog().querySelector('[data-create-go]').click();
    expect(dialog().querySelector('.create-error').textContent).toBe('No connected project to create in.');
    expect(mocks.legacy).not.toHaveBeenCalled();expect(mocks.submit).not.toHaveBeenCalled();
  });

});

it.each(['project','environment','new-environment'])('uses the shared identity SVG for wizard %s targets',async kind=>{
 const {kindGlyphHtml}=await import('../src/core/environmentIdentity.js');
 const html=createWorkspaceWizardHtml({page:3,target:{kind,id:'x',name:'Target',sources:[]}});
 expect(html).toContain(kindGlyphHtml(kind==='project'?'project':'environment'));
});

it('Escape cancels the wizard and restores the opener',async()=>{
 await open({fixedProject:false});document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
 await vi.waitFor(()=>expect(dialog()).toBeNull());expect(document.activeElement.id).toBe('opener');
});
it('disables the picker, name, disclosure and actions during durable admission',async()=>{
 let admitted;const ready=new Promise(resolve=>{admitted=resolve;});
 mocks.submit.mockReturnValue({operationId:'busy',ready,completion:Promise.resolve(),get:async()=>({})});
 await open({fixedProject:false});clickTarget('environment');dialog().querySelector('[data-create-go]').click();
 expect(dialog().querySelector('#create-work-input').disabled).toBe(true);
 expect(dialog().querySelector('#create-workspace-search').disabled).toBe(true);
 expect([...dialog().querySelectorAll('[data-create-target],[data-create-go],[data-create-cancel]')].every(el=>el.disabled)).toBe(true);
 dialog().querySelector('summary').click();expect(dialog().querySelector('[data-create-advanced]').open).toBe(false);
 document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));expect(dialog()).not.toBeNull();
 admitted();await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledTimes(1));
});
it('keeps environment update refusals on the main page at selection time',async()=>{
 const held={...snapshot(),features:{d1:{create:true,createProject:true}}};mocks.read.mockResolvedValue(held);
 await open({fixedProject:false});held.features.d1.createProject=false;clickTarget('environment');
 expect(dialog().dataset.createPage).toBe('1');expect(dialog().querySelector('.create-error').textContent).toContain('needs an update');
 expect(dialog().querySelector('[data-create-target][aria-pressed="true"]')).toBeNull();expect(mocks.submit).not.toHaveBeenCalled();
});
it('uses the lowest free derived name across cached siblings when creating a project',async()=>{
 const held=snapshot();held.projects.push({id:'collision',deviceId:'d1',environment_id:'e1',name:'Platform'},{id:'third',deviceId:'d1',environment_id:'e1',name:'Platform 3'});mocks.read.mockResolvedValue(held);
 await open({fixedProject:false});clickTarget('environment');dialog().querySelector('[data-create-go]').click();
 await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(1));expect(mocks.submit.mock.calls[0][0].name).toBe('Platform 2');
});

it.each(['insert','remove','reorder'])('restores focused row identity on cache %s rather than reusing its index',async change=>{
 const held=snapshot();held.projects.push({id:'p2',deviceId:'d1',name:'Second',sources:[]});mocks.read.mockResolvedValue(held);
 await open({fixedProject:false});
 const focused=dialog().querySelector('[data-create-target]');focused.focus();
 if(change==='insert')held.projects.unshift({id:'p0',deviceId:'d1',name:'Inserted',sources:[]});
 if(change==='remove')held.projects.shift();
 if(change==='reorder')held.projects.reverse();
 mocks.follow();
 await vi.waitFor(()=>expect(focused.isConnected).toBe(false));
 await vi.waitFor(()=>{
  if(change==='remove')expect(document.activeElement.id).toBe('create-workspace-search');
  else expect(document.activeElement.textContent).toContain('Payments');
 });
 if(change!=='remove'){
  document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
  expect(dialog().querySelector('.create-target-summary').textContent).toContain('Payments');
 }
});
it('leaves arrow keys available to scroll a focused refusal',async()=>{
 await open({fixedProject:false,error:'A refusal that is read inside its own scroll region.'});
 const error=dialog().querySelector('.create-error');error.focus();
 const down=new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true,cancelable:true});error.dispatchEvent(down);
 expect(down.defaultPrevented).toBe(false);expect(document.activeElement).toBe(error);
});
