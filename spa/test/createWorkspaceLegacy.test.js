// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks=vi.hoisted(()=>({create:vi.fn(),navigate:vi.fn()}));
vi.mock('../src/app.js',()=>({go:mocks.navigate}));
vi.mock('../src/core/createWorkspaceTargets.js',()=>({createLegacyWorkspace:mocks.create}));
vi.mock('../src/core/motion.js',()=>({
  reveal:async element=>{element.hidden=false;},hide:async element=>{element.hidden=true;},settleHidden:element=>{element.hidden=true;},
}));
const {openCreateWorkspaceLegacy,createWorkspaceLegacyHtml}=await import('../src/core/createWorkspaceLegacy.js');
const route={name:'workspace',deviceId:'old-device',projectId:'p1',workspaceId:'ws1',tab:'changes'};
const modal=()=>document.querySelector('#create-scrim .modal');
let opened;
beforeEach(()=>{
  document.body.innerHTML='<button id="opener">New workspace</button>';
  vi.clearAllMocks();mocks.create.mockResolvedValue(route);
});
afterEach(async()=>{await opened?.close();opened=null;});
const open=options=>opened=openCreateWorkspaceLegacy({projectId:'p1',deviceId:'old-device',projectName:'Payments',...options});
const input=value=>{const field=modal().querySelector('#create-work-input');field.value=value;field.dispatchEvent(new Event('input'));};

describe('old bridge workspace creation',()=>{
  it('keeps the original copy, fields and modal without wizard content',()=>{
    open();
    expect(modal().querySelector('h3').textContent).toBe('New workspace in Payments');
    expect(modal().querySelector('.create-hint').textContent).toBe('Creates a separate checkout of the project sources.');
    expect(modal().querySelector('label[for="create-work-input"]').textContent).toBe('Workspace label');
    expect(modal().querySelector('#create-work-input').placeholder).toBe('Workspace');
    expect(modal().querySelector('#create-work-isolation option').textContent).toBe('Inherit project setting');
    expect(modal().querySelector('[data-create-go]').textContent).toBe('Create workspace');
    expect(modal().classList.contains('create-workspace-sheet')).toBe(false);
    expect(modal().textContent).not.toMatch(/What are you working on|task board|Directories|Where\?|Back/);
    expect(modal().querySelector('[data-create-target],#create-workspace-search')).toBeNull();
  });

  it.each(['','  Bridge interface  ','../feature: *?🦊','a'.repeat(500)])('submits the exact legacy workspace label %j',async name=>{
    open();input(name);
    const isolation=modal().querySelector('#create-work-isolation');isolation.value='rift';isolation.dispatchEvent(new Event('change'));
    modal().querySelector('[data-create-go]').click();
    expect(mocks.create).toHaveBeenCalledExactlyOnceWith({target:{kind:'project',id:'p1',deviceId:'old-device'},name,isolation:'rift'});
    await vi.waitFor(()=>expect(mocks.navigate).toHaveBeenCalledExactlyOnceWith(route));
    expect(modal()).toBeNull();
  });

  it('freezes the old fields and cancellation while a single create is pending',async()=>{
    let fail;mocks.create.mockImplementation(()=>new Promise((resolve,reject)=>{fail=reject;}));
    open();input('My draft');
    modal().querySelector('[data-create-go]').click();modal().querySelector('[data-create-go]').click();
    expect(mocks.create).toHaveBeenCalledTimes(1);
    for(const selector of ['#create-work-input','#create-work-isolation','[data-create-cancel]','[data-create-go]'])expect(modal().querySelector(selector).disabled).toBe(true);
    await opened.close();document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
    expect(modal()).not.toBeNull();
    fail(new Error('Checkout refused'));
    await vi.waitFor(()=>expect(modal().querySelector('.create-error').textContent).toBe('Checkout refused'));
    expect(modal().querySelector('#create-work-input').value).toBe('My draft');
    expect(modal().querySelector('[data-create-cancel]').disabled).toBe(false);
    mocks.create.mockResolvedValue(route);modal().querySelector('[data-create-go]').click();
    await vi.waitFor(()=>expect(mocks.navigate).toHaveBeenCalledTimes(1));
  });

  it('keeps an unpreset entry on the original no-project surface',()=>{
    open({projectId:null,projectName:'Project'});
    modal().querySelector('[data-create-go]').click();
    expect(modal().querySelector('.create-error').textContent).toBe('No connected project to create in.');
    expect(mocks.create).not.toHaveBeenCalled();
    expect(modal().querySelector('[data-create-target],#create-workspace-search')).toBeNull();
  });

  it('closes the original modal on Escape without creation',async()=>{
    open();document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
    await vi.waitFor(()=>expect(modal()).toBeNull());expect(mocks.create).not.toHaveBeenCalled();
  });

  it('escapes project labels in the original renderer',()=>{
    expect(createWorkspaceLegacyHtml({projectName:'<b>unsafe</b>',name:'',isolation:'',busy:false,error:''})).toContain('&lt;b&gt;unsafe&lt;/b&gt;');
  });
});
