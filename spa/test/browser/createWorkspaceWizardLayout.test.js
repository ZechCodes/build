import { afterAll, beforeAll, expect, it } from 'vitest';
import { openLayoutSession, mountLayout, loadBrowserModules, captureLayout } from './layoutHarness.mjs';
import { navigationAppShim } from './navigationBreadcrumbSeed.mjs';
let session;
beforeAll(async()=>{session=await openLayoutSession({plugins:[navigationAppShim]});},30000);
afterAll(async()=>{await session?.close();});
async function mount(page,basePath,pageNumber=1){
 await mountLayout(page,'<div class="modal-scrim" id="creation"></div>',{basePath});
 await loadBrowserModules(page,{wizard:'src/core/createWorkspaceWizard.js'},basePath);
 await page.evaluate(pageNumber=>{
  const target={kind:'project',id:'p1',deviceId:'d1',name:'Payments',label:'Payments',summaryLabel:'Platform › Payments',deviceLabel:'Laptop',sources:[{path:'/code/api'}],taskText:'12 open tasks',description:'2 workspaces · 12 open tasks'};
  const sections=[{kind:'projects',title:'Projects',helper:'Uses the existing task board',targets:Array.from({length:16},(_,i)=>({...target,id:`p${i}`,label:`Payments ${i}`}))},{kind:'environments',title:'Environments',helper:'Creates a new task board',targets:[{...target,kind:'environment',name:'Platform',label:'Platform',description:'2 projects · 3 workspaces'}]}];
  document.querySelector('#creation').innerHTML=window.__layoutModules.wizard.createWorkspaceWizardHtml({page:pageNumber,target,sections,name:'Release',isolation:'',error:''});
 },pageNumber);
}
for(const width of [390,760])it(`fills the mobile viewport and keeps 44px target rows at ${width}px`,async()=>{
 await session.withPage(async({page,basePath})=>{
  await mount(page,basePath);const geometry=await page.evaluate(()=>{const el=document.querySelector('[role="dialog"]');const r=el.getBoundingClientRect();return {top:r.top,left:r.left,width:r.width,height:r.height,rows:[...el.querySelectorAll('[data-create-target]')].map(row=>row.getBoundingClientRect().height),scrolls:el.scrollHeight>el.clientHeight};});
  expect(geometry.top).toBe(0);expect(geometry.left).toBe(0);expect(geometry.width).toBe(width);expect(geometry.height).toBe(844);expect(geometry.scrolls).toBe(true);expect(Math.min(...geometry.rows)).toBeGreaterThanOrEqual(44);
  await captureLayout(page,`workspace-wizard-${width}.png`);
 },{width,height:844});
},30000);
it('keeps the name page and directory list within desktop and phone viewports',async()=>{
 for(const width of [390,1280])await session.withPage(async({page,basePath})=>{
  await mount(page,basePath,3);expect(await page.getByLabel('Name (optional)').inputValue()).toBe('Release');
  const geometry=await page.locator('[role="dialog"]').boundingBox();expect(geometry.x).toBeGreaterThanOrEqual(0);expect(geometry.x+geometry.width).toBeLessThanOrEqual(width);
  expect(await page.getByRole('button',{name:'Create workspace'}).isVisible()).toBe(true);await captureLayout(page,`workspace-wizard-name-${width}.png`);
 },{width,height:844});
},30000);

for(const width of [390,1280])it(`keeps new-environment drafts and Advanced folders usable at ${width}px`,async()=>{
 await session.withPage(async({page,basePath})=>{
  await mount(page,basePath);
  await page.evaluate(()=>{
   const source={kind:'remote',remote:'https://github.com/org/api.git',name:'api',base_branch:'main'};
   const draft={deviceId:'d1',name:'Platform',sources:[source],isolation:''};
   window.__m2Draft={page:2,environmentDraft:draft,newEnvironmentDevices:[{id:'d1',name:'Laptop'}]};
   document.querySelector('#creation').innerHTML=window.__layoutModules.wizard.createWorkspaceWizardHtml(window.__m2Draft);
  });
  expect(await page.getByLabel('Git remote URL').inputValue()).toContain('api.git');
  await captureLayout(page,`workspace-wizard-environment-${width}.png`);
  await page.evaluate(()=>{
   const target={kind:'new-environment',id:'draft',deviceId:'d1',name:'Platform',summaryLabel:'Platform (new environment)',sources:[{path:'/code/api'}]};
   document.querySelector('#creation').innerHTML=window.__layoutModules.wizard.createWorkspaceWizardHtml({page:3,target,name:'Ship wizard',showAdvanced:true,advancedExpanded:true,addedSources:[{kind:'path',path:'/code/shared',name:'shared',base_branch:''}],folderReferences:{environments:[{deviceId:'d1',name:'Tools',sources:[{path:'/code/shared'}]}]}});
  });
  expect(await page.getByText('Folder 1 · shared with Tools').isVisible()).toBe(true);
  const dimensions=await page.evaluate(()=>({width:document.documentElement.scrollWidth,viewport:innerWidth}));
  expect(dimensions.width).toBe(dimensions.viewport);
  await captureLayout(page,`workspace-wizard-advanced-${width}.png`);
 },{width,height:844});
},30000);
