import { afterAll, beforeAll, expect, it } from 'vitest';
import { openLayoutSession, mountLayout, loadBrowserModules } from './layoutHarness.mjs';
import { navigationAppShim } from './navigationBreadcrumbSeed.mjs';
const scratch='/home/zech/Projects/.agent-scratch/env-wizard-polish';
let session;
beforeAll(async()=>{session=await openLayoutSession({plugins:[navigationAppShim]});},30000);
afterAll(async()=>{await session?.close();});
async function mount(page,basePath,pageNumber=1){
 await mountLayout(page,'<div class="modal-scrim" id="creation"></div>',{basePath});
 await loadBrowserModules(page,{wizard:'src/core/createWorkspaceWizard.js'},basePath);
 await page.evaluate(pageNumber=>{
  const target={kind:'environment',id:'e1',key:'d1/e1',deviceId:'d1',name:'Platform',label:'Platform',summaryLabel:'Platform',deviceLabel:'Laptop',offline:true,sources:Array.from({length:8},(_,i)=>({path:`/code/api-${i}`})),description:'1 project · 1 workspace'};
  const projects=Array.from({length:16},(_,i)=>({...target,kind:'project',id:`p${i}`,key:`d1/p${i}`,label:`Payments ${i}`}));
  const sections=[{kind:'projects',title:'Projects',helper:'Uses the existing task board',targets:projects},{kind:'environments',title:'Environments',helper:'Creates a new task board',targets:[target]}];
  const sources=Array.from({length:4},(_,i)=>({kind:'remote',remote:`https://github.com/org/api-${i}.git`,name:`api-${i}`,base_branch:'main'}));
  const state={page:pageNumber,target,sections,name:'Release',isolation:'',error:'',showAdvanced:true,addedSources:sources,newEnvironmentDevices:[{id:'d1',name:'Laptop'}],environmentDraft:{deviceId:'d1',name:'Platform',sources,isolation:''}};
  document.querySelector('#creation').innerHTML=window.__layoutModules.wizard.createWorkspaceWizardHtml(state);
  window.__layoutModules.wizard.placeWizardAdvanced?.(document.querySelector('[role="dialog"]'));
 },pageNumber);
}
async function pinned(page,selectors){
 const dimensions=await page.evaluate(selectors=>{
  const dialog=document.querySelector('[role="dialog"]');
  return {scrolls:dialog.scrollHeight>dialog.clientHeight+1,wide:document.documentElement.scrollWidth>innerWidth,controls:selectors.map(selector=>{
   const el=document.querySelector(selector);const r=el?.getBoundingClientRect();
   return {selector,inside:!!r&&r.width>0&&r.height>0&&r.top>=0&&r.left>=0&&r.bottom<=innerHeight&&r.right<=innerWidth};
  })};
 },selectors);
 expect(dimensions).toEqual({scrolls:false,wide:false,controls:selectors.map(selector=>({selector,inside:true}))});
}
const pageOne=['#create-workspace-search','#create-work-input','[data-create-advanced] summary','[data-create-new-environment]','[data-create-cancel]','[data-create-go]'];
for(const [width,height] of [[390,844],[440,956],[320,568],[1280,800]]){
 it(`pins page one controls above and below a scrolling list at ${width}x${height}`,async()=>{
  await session.withPage(async({page,basePath})=>{
   await mount(page,basePath);await pinned(page,pageOne);
   expect(await page.locator('[data-create-target]').count()).toBeGreaterThanOrEqual(16);
   const rows=await page.locator('[data-create-target]').evaluateAll(rows=>rows.map(row=>row.getBoundingClientRect().height));
   expect(Math.min(...rows)).toBeGreaterThanOrEqual(44);
   const scroll=page.locator('[data-create-scroll]');
   expect(await scroll.evaluate(el=>el.scrollHeight>el.clientHeight)).toBe(true);
   await scroll.evaluate(el=>{el.scrollTop=el.scrollHeight;});await pinned(page,pageOne);
   if(width===440)await page.screenshot({path:`${scratch}/page-1-440.png`});
   await page.locator('[data-create-advanced] summary').click();await pinned(page,pageOne);
   expect(await page.getByLabel('Isolation',{exact:true}).isVisible()).toBe(true);
   await scroll.evaluate(el=>{el.scrollTop=el.scrollHeight;});await pinned(page,pageOne);
   await page.locator('[data-folder-add="remote"]').scrollIntoViewIfNeeded();
   await pinned(page,pageOne);
   if(width===440)await page.screenshot({path:`${scratch}/page-1-advanced-440.png`});
  },{width,height});
 },30000);
 it(`pins page two actions while its form scrolls at ${width}x${height}`,async()=>{
  await session.withPage(async({page,basePath})=>{
   await mount(page,basePath,2);
   const controls=['[data-create-back]','[data-create-cancel]','[data-create-next]'];await pinned(page,controls);
   expect(await page.locator('[data-create-next]').textContent()).toBe('Use environment');
   const scroll=page.locator('[data-create-scroll]');expect(await scroll.evaluate(el=>el.scrollHeight>el.clientHeight)).toBe(true);
   await scroll.evaluate(el=>{el.scrollTop=el.scrollHeight;});await pinned(page,controls);
   if(width===440)await page.screenshot({path:`${scratch}/page-2-440.png`});
  },{width,height});
 },30000);
}
it('preserves real list scroll and row focus, search selection, disclosure focus and draft editing',async()=>{
 await session.withPage(async({page,basePath})=>{
  await mountLayout(page,'<button id="opener">New workspace</button>',{basePath});
  await loadBrowserModules(page,{wizard:'src/core/createWorkspaceWizard.js',app:'src/appState.js',cache:'src/core/localCache.js',support:'src/core/environmentSupport.js'},basePath);
  await page.evaluate(async()=>{
   const {wizard,app,cache,support}=window.__layoutModules;
   app.App.devices=[{id:'layout',name:'Laptop',status:'offline'}];
   await support.rememberEnvironmentSupport('layout',{environments:{list:true,create:true,createProject:true,addedSources:true}});
   await cache.writeCached({deviceId:'layout',entityId:'',kind:'projects'},Array.from({length:20},(_,i)=>({project_id:`p${i}`,name:`Project ${String(i).padStart(2,'0')}`,sources:[{path:'/code/api'}]})));
   await cache.writeCached({deviceId:'layout',entityId:'',kind:'environments'},[{environment_id:'e',name:'Platform',sources:[{path:'/code/api'}]}]);
   window.__wizardScreen=wizard.openCreateWorkspaceWizard({fixedProject:false});await window.__wizardScreen.ready;
  });
  await expect.poll(()=>page.evaluate(()=>document.activeElement.id)).toBe('create-workspace-search');
  const row=page.locator('[data-create-target]').nth(15);await row.scrollIntoViewIfNeeded();await row.focus();
  const before=await page.locator('[data-create-scroll]').evaluate(el=>el.scrollTop);
  await page.keyboard.press('Enter');
  expect(await page.locator('[data-create-scroll]').evaluate(el=>el.scrollTop)).toBe(before);
  expect(await row.evaluate(el=>document.activeElement===el&&el.getAttribute('aria-pressed')==='true')).toBe(true);
  await page.keyboard.press('/');await page.locator('#create-workspace-search').fill('no-match');
  expect(await page.locator('[data-create-target]').count()).toBe(0);
  expect(await page.locator('.create-target-summary').textContent()).toContain('Project 15');
  await page.locator('[data-create-advanced] summary').click();await pinned(page,pageOne);
  await page.getByLabel('Isolation',{exact:true}).focus();await page.keyboard.press('Tab');
  expect(await page.evaluate(()=>document.activeElement.id)).toBe('create-work-input');
  await page.locator('[data-create-advanced] summary').focus();await page.keyboard.press('Space');
  expect(await page.locator('[data-create-advanced]').evaluate(el=>el.open)).toBe(false);
  await page.locator('[data-create-new-environment]').click();await page.locator('[data-folder-add="remote"]').click();
  await page.locator('[data-folder-value]').fill('https://github.com/org/api.git');await page.locator('[data-create-next]').click();
  expect(await page.evaluate(()=>document.activeElement.id)).toBe('create-work-input');
  await page.locator('#create-workspace-search').fill('');
  await page.locator('[data-create-edit]').focus();await page.keyboard.press('Enter');
  expect(await page.locator('[data-folder-value]').inputValue()).toContain('api.git');
  await page.evaluate(()=>window.__wizardScreen.close());
 },{width:390,height:844});
},30000);
