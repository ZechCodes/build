// @vitest-environment jsdom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const mocks = vi.hoisted(() => ({ read: vi.fn(), submit: vi.fn(), legacy: vi.fn(), progress: vi.fn(), navigate: vi.fn(), consume:vi.fn(), cache:vi.fn(), route:vi.fn() }));
vi.mock("../src/app.js", () => ({ go: mocks.navigate }));
vi.mock("../src/core/createWorkspaceTargets.js", () => ({
  readCreateWorkspaceTargets: mocks.read, followCreateWorkspaceTargets: () => () => {},
  submitCreateWorkspace: mocks.submit, createLegacyWorkspace: mocks.legacy,
  taskCountText: (open, total) => open > 0 ? `${open} open tasks` : `${total || 0} tasks`,
  cacheCreatedWorkspace: vi.fn(), cacheCreatedResources:mocks.cache, createdWorkspaceRoute:mocks.route, discardCreatedProject: vi.fn(), consumeCreateWorkspaceAttempt:mocks.consume,
}));
vi.mock("../src/core/createWorkspaceProgress.js", () => ({ openCreateWorkspaceProgress: mocks.progress }));
vi.mock("../src/core/motion.js", () => ({
  reveal: async el => { el.hidden = false; }, hide: async el => { el.hidden = true; }, settleHidden: el => { el.hidden = true; },
}));
const { openCreateWorkspaceWizard, createWorkspaceWizardHtml } = await import("../src/core/createWorkspaceWizard.js");
const { readUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");
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
  it('advances, preserves the name on Back, previews the branch and lists sources',async()=>{
    await open({fixedProject:false});clickTarget('environment');
    expect(dialog().querySelector('.create-workspace-page-enter')).not.toBeNull();
    expect(dialog().textContent).toContain('New workspace in Platform'); expect(dialog().querySelector('#create-work-input').required).toBe(true);
    name('Ship payments');expect(dialog().querySelector('[data-branch-preview]').textContent).toContain('build/ship-payments');
    expect(dialog().textContent).toContain('/code/api');dialog().querySelector('[data-create-back]').click();clickTarget('project');
    expect(dialog().querySelector('#create-work-input').value).toBe('Ship payments');expect(dialog().querySelector('#create-work-input').required).toBe(false);
    expect(dialog().textContent).toContain("Uses Payments's task board"); expect(mocks.submit).not.toHaveBeenCalled();
  });
  it('supports arrow navigation, Enter selection, slash search, Escape and opener focus restoration',async()=>{
    await open({fixedProject:false,fromShortcut:true});
    expect(document.activeElement.hasAttribute('data-create-target')).toBe(true);
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true}));
    expect(document.activeElement.dataset.kind).toBe('environment');
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'/',bubbles:true}));
    expect(document.activeElement.id).toBe('create-workspace-search');
    dialog().querySelector('[data-create-target]').focus();document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
    expect(dialog().querySelector('#create-work-input')).not.toBeNull();
    document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));await vi.waitFor(()=>expect(dialog()).toBeNull());
    expect(document.activeElement.id).toBe('opener');
  });
  it('stores a draft in build-ui, restores it after detach and deletes it on Cancel',async()=>{
    const options={projectId:'p1',deviceId:'d1',projectName:'Payments'};
    await open(options);name('Draft name');await opened.flushDraft();await opened.detach();opened=null;
    await open(options);expect(dialog().querySelector('#create-work-input').value).toBe('Draft name');
    dialog().querySelector('[data-create-cancel]').click();await vi.waitFor(()=>expect(dialog()).toBeNull());
    const address=uiAddress({deviceId:'d1',entityId:'p1',view:'create-workspace',kind:'draft'});
    expect(await readUiRecord(address)).toBeUndefined();
  });
  it('submits once, opens cached progress and retains the operation for reopening',async()=>{
    await open({fixedProject:false});clickTarget('environment');name('New board');
    const create=dialog().querySelector('[data-create-go]');create.click();create.click();
    await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(1));
    expect(mocks.submit.mock.calls[0][0]).toMatchObject({target:{kind:'environment',id:'e1'},name:'New board'});
    await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledWith(expect.objectContaining({operationId:'op-one',deviceId:'d1'})));
  });
  it('requires a name for new projects, permits blank fixed names and traps Tab within the sheet',async()=>{
    await open({fixedProject:false});clickTarget('environment');dialog().querySelector('[data-create-go]').click();expect(mocks.submit).not.toHaveBeenCalled();
    const cancel=dialog().querySelector('[data-create-cancel]');const create=dialog().querySelector('[data-create-go]');
    create.focus();create.dispatchEvent(new KeyboardEvent('keydown',{key:'Tab',bubbles:true}));expect(dialog().contains(document.activeElement)).toBe(true);
    cancel.click();await vi.waitFor(()=>expect(dialog()).toBeNull());
    await open({projectId:'p1',deviceId:'d1'});dialog().querySelector('[data-create-go]').click();await vi.waitFor(()=>expect(mocks.submit).toHaveBeenCalledTimes(1));
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
  it('retries only the landed project with its own summary and predecessor',async()=>{
    await open({fixedProject:false});clickTarget('environment');name('New board');dialog().querySelector('[data-create-go]').click();
    await vi.waitFor(()=>expect(mocks.progress).toHaveBeenCalledTimes(1));
    await mocks.progress.mock.calls[0][0].onRetryWorkspace({project:{project_id:'landed',name:'New board',environment_id:'e1'},operationId:'failed-attempt'});
    expect(mocks.submit.mock.calls[1][0]).toMatchObject({target:{kind:'project',id:'landed',key:'d1/landed',summaryLabel:'Platform › New board'},continues:'failed-attempt'});
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
  it('does not accept a whitespace-only name for a new task board',async()=>{
    await open({fixedProject:false});clickTarget('environment');name('   ');dialog().querySelector('[data-create-go]').click();
    await new Promise(resolve=>setTimeout(resolve,10));expect(mocks.submit).not.toHaveBeenCalled();
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

});
