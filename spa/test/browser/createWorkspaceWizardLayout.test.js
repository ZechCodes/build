import { afterAll, beforeAll, expect, it } from 'vitest';
import { openLayoutSession, mountLayout, loadBrowserModules } from './layoutHarness.mjs';
import { navigationAppShim } from './navigationBreadcrumbSeed.mjs';
let session;
beforeAll(async()=>{session=await openLayoutSession({plugins:[navigationAppShim]});},30000);
afterAll(async()=>{await session?.close();});
async function mount(page,basePath,pageNumber=1,variant=''){
 await mountLayout(page,'<div class="modal-scrim" id="creation"></div>',{basePath});
 await loadBrowserModules(page,{wizard:'src/core/createWorkspaceWizard.js'},basePath);
 await page.evaluate(({pageNumber,variant})=>{
  const target={kind:'environment',id:'e1',key:'d1/e1',deviceId:'d1',name:'Platform',label:'Platform',summaryLabel:'Platform',deviceLabel:'Laptop',offline:true,sources:Array.from({length:8},(_,i)=>({path:`/code/api-${i}`})),description:'1 project · 1 workspace'};
  const projects=Array.from({length:16},(_,i)=>({...target,kind:'project',id:`p${i}`,key:`d1/p${i}`,label:`Payments ${i}`}));
  const sections=[{kind:'projects',title:'Projects',helper:'Uses the existing task board',targets:projects},{kind:'environments',title:'Environments',helper:'Creates a new task board',targets:[target]}];
  const sources=Array.from({length:4},(_,i)=>({kind:'remote',remote:`https://github.com/org/api-${i}.git`,name:`api-${i}`,base_branch:'main'}));
  const state={page:pageNumber,target,sections,name:'Release',isolation:'',error:'',showAdvanced:true,addedSources:sources,newEnvironmentDevices:[{id:'d1',name:'Laptop'}],environmentDraft:{deviceId:'d1',name:'Platform',sources,isolation:''}};
  if(variant.includes('long'))Object.assign(state.target,{name:'An exceptionally long selected environment for cross-team release planning and deployment verification',summaryLabel:'An exceptionally long selected environment for cross-team release planning and deployment verification',deviceLabel:'A long device label for the development laptop'});
  if(variant.includes('project')){
   const name='A project for release coordination across multiple products with comprehensive deployment verification and staged migrations for every supported device and shared workspace'.slice(0,170);
   Object.assign(state.target,{kind:'project',name,label:name,summaryLabel:name,taskText:'1 open task',sources:Array.from({length:16},(_,i)=>({path:`/code/project-${i}`}))});
   state.showAdvanced=false;state.addedSources=[];state.sections=sections.filter(section=>section.kind==='projects');
   projects.unshift({...state.target});
  }
  if(variant.includes('error'))state.error='Choose a project or environment. Please check the draft and try again.';
  if(variant.includes('progress'))state.operationId='pending';
  if(variant.includes('lengthy')){state.error='The environment refused this workspace because its configuration needs attention. '.repeat(40)+'END OF REFUSAL';state.name='A very long retained workspace name '.repeat(30)+'END OF RETAINED NAME';}
  document.querySelector('#creation').innerHTML=window.__layoutModules.wizard.createWorkspaceWizardHtml(state);
  window.__layoutModules.wizard.wireWizardAdvanced?.(document.querySelector('[role="dialog"]'));
 },{pageNumber,variant});
}
async function pinned(page,selectors){
 const dimensions=await page.evaluate(selectors=>{
  const dialog=document.querySelector('[role="dialog"]');
  return {scrolls:dialog.scrollHeight>dialog.clientHeight,wide:document.documentElement.scrollWidth>innerWidth,controls:selectors.map(selector=>{
   const el=document.querySelector(selector);const r=el?.getBoundingClientRect();
   return {selector,inside:!!r&&r.width>0&&r.height>0&&r.top>=0&&r.left>=0&&r.bottom<=innerHeight&&r.right<=innerWidth};
  })};
 },selectors);
 expect(dimensions).toEqual({scrolls:false,wide:false,controls:selectors.map(selector=>({selector,inside:true}))});
}
async function advancedReachable(page,controls=pageOne){
 const body=page.locator('#create-advanced-body');
 const structure=await body.evaluate(el=>{
  const summary=el.previousElementSibling;const r=el.getBoundingClientRect();const s=summary.getBoundingClientRect();
  return {native:el.parentElement.matches('details[open]')&&summary.tagName==='SUMMARY',below:r.top>=s.bottom-1&&r.top<=s.bottom+1,overflow:el.scrollHeight>el.clientHeight,capped:r.height<=Math.min(innerHeight*.4,304)+1};
 });
 expect(structure).toEqual({native:true,below:true,overflow:true,capped:true});
 for(const control of await body.locator('input,select,textarea,button').all()){
  await control.evaluate(el=>{
   const body=el.closest('#create-advanced-body'),r=el.getBoundingClientRect(),b=body.getBoundingClientRect();
   body.scrollTop+=(r.top+r.bottom-b.top-b.bottom)/2;
  });
  const geometry=await control.evaluate(el=>{
   const c=el.getBoundingClientRect(),b=el.closest('#create-advanced-body').getBoundingClientRect();
   return {inside:c.top>=b.top-1&&c.bottom<=b.bottom+1&&c.left>=b.left-1&&c.right<=b.right+1&&c.top>=0&&c.bottom<=innerHeight,control:{top:c.top,bottom:c.bottom,left:c.left,right:c.right},body:{top:b.top,bottom:b.bottom,left:b.left,right:b.right}};
  });
  expect(geometry.inside,JSON.stringify(geometry)).toBe(true);
  await pinned(page,controls);
 }
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
   await page.locator('[data-create-advanced] summary').click();await pinned(page,pageOne);
   await advancedReachable(page);

  },{width,height});
 },30000);
 it(`pins page two actions while its form scrolls at ${width}x${height}`,async()=>{
  await session.withPage(async({page,basePath})=>{
   await mount(page,basePath,2);
   const controls=['[data-create-back]','[data-create-cancel]','[data-create-next]'];await pinned(page,controls);
   expect(await page.locator('[data-create-next]').textContent()).toBe('Use environment');
   const scroll=page.locator('[data-create-scroll]');expect(await scroll.evaluate(el=>el.scrollHeight>el.clientHeight)).toBe(true);
   await scroll.evaluate(el=>{el.scrollTop=el.scrollHeight;});await pinned(page,controls);
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
  await page.locator('[data-create-advanced] summary').focus();await page.keyboard.press('Tab');
  expect(await page.evaluate(()=>document.activeElement.id)).toBe('create-work-isolation');
  await page.keyboard.press('Tab');expect(await page.evaluate(()=>document.activeElement.hasAttribute('data-create-new-environment'))).toBe(true);
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

for(const [width,height] of [[390,844],[320,568]])for(const variant of ['error','progress','long','long-error-progress']){
 it(`keeps pinned controls visible with ${variant} at ${width}x${height}`,async()=>{
  await session.withPage(async({page,basePath})=>{
   await mount(page,basePath,1,variant);
   const extras=[];
   if(variant.includes('error'))extras.push('.create-error');
   if(variant.includes('progress'))extras.push('.create-resume','[data-create-resume]');
   if(variant.includes('long'))extras.push('.create-target-summary');
   const controls=[...pageOne,...extras];await pinned(page,controls);
   await page.locator('[data-create-advanced] summary').click();await pinned(page,controls);
   await advancedReachable(page,controls);
  },{width,height});
 },30000);
}

async function singleLineSummaries(page){
 const names=['.create-summary-name','.create-summary-device-name'];
 for(const selector of names){
  expect(await page.locator(selector).count()).toBe(1);
  const details=await page.locator(selector).evaluate(el=>{
   const style=getComputedStyle(el);
   return {noWrap:style.whiteSpace==='nowrap',ellipsis:style.textOverflow==='ellipsis',fullText:el.title===el.textContent.trim(),height:el.getBoundingClientRect().height,lineHeight:parseFloat(style.lineHeight)};
  });
  expect(details.noWrap).toBe(true);expect(details.ellipsis).toBe(true);expect(details.fullText).toBe(true);
  expect(details.height).toBeLessThanOrEqual(details.lineHeight+1);
 }
}
async function summaryFactsVisible(page){
 const facts=await page.evaluate(()=>{
  const outcome=document.querySelector('.create-outcome');
  const boxes=['.create-summary-offline','.create-outcome'].map(selector=>{
   const el=document.querySelector(selector);
   if(!el)return {selector,inside:false,unclipped:false};
   const r=el.getBoundingClientRect(),box=el.closest('.create-target-summary,.create-name').getBoundingClientRect();
   return {selector,inside:r.top>=box.top&&r.bottom<=box.bottom&&r.left>=box.left&&r.right<=box.right,
    unclipped:el.scrollWidth<=el.clientWidth};
  });
  return {outcome:outcome.textContent,boxes};
 });
 expect(facts).toEqual({outcome:"Uses this project's task board",boxes:[
  {selector:'.create-summary-offline',inside:true,unclipped:true},
  {selector:'.create-outcome',inside:true,unclipped:true},
 ]});
}
for(const [width,height] of [[390,844],[320,568]])for(const status of ['error','progress','error-progress']){
 it(`pins controls for a 170-character project with ${status} and Advanced at ${width}x${height}`,async()=>{
  await session.withPage(async({page,basePath})=>{
   await mount(page,basePath,1,`long-project-${status}`);
   const targetName=await page.locator('.create-target-summary').textContent();
   expect(targetName).toContain('A project for release coordination');
   expect(await page.locator('.create-summary-name').getAttribute('title')).toHaveLength(170);
   expect(await page.locator('[data-create-target]').first().textContent()).toContain('staged migrations');
   const controls=[...pageOne,'.create-target-summary','.create-outcome'];
   if(status.includes('error'))controls.push('.create-error');
   if(status.includes('progress'))controls.push('.create-resume','[data-create-resume]');
   await page.locator('[data-create-advanced] summary').click();await pinned(page,controls);
   expect(await page.locator('[role="dialog"]').evaluate(el=>el.scrollHeight<=el.clientHeight)).toBe(true);
   await summaryFactsVisible(page);await singleLineSummaries(page);await advancedReachable(page,controls);
  },{width,height});
 },30000);
 if(status!=='error-progress')continue;
 it(`scrolls lengthy refusal and progress independently for a long project at ${width}x${height} with ${status}`,async()=>{
  await session.withPage(async({page,basePath})=>{
   await mount(page,basePath,1,`long-project-lengthy-${status}`);await page.locator('[data-create-advanced] summary').click();
   await pinned(page,pageOne);
   const error=page.locator('.create-error');
   expect(await error.evaluate(el=>el.scrollHeight>el.clientHeight&&el.clientHeight<=3*parseFloat(getComputedStyle(el).lineHeight)+1)).toBe(true);
   await error.evaluate(el=>{
    const text=el.firstChild;const range=document.createRange();range.setStart(text,text.length-'END OF REFUSAL'.length);range.setEnd(text,text.length);
    el.scrollTop=el.scrollHeight;const r=range.getBoundingClientRect(),b=el.getBoundingClientRect();
    window.__errorEndInside=r.top>=b.top-1&&r.bottom<=b.bottom+1;
   });
   expect(await page.evaluate(()=>window.__errorEndInside)).toBe(true);await pinned(page,pageOne);
   if(status.includes('progress')){
    const progress=page.locator('.create-resume');
    expect(await progress.evaluate(el=>el.scrollHeight>el.clientHeight&&el.clientHeight<=3*parseFloat(getComputedStyle(el).lineHeight)+1)).toBe(true);
    await progress.evaluate(el=>{el.scrollTop=el.scrollHeight;});await pinned(page,[...pageOne,'[data-create-resume]']);
   }
  },{width,height});
 },30000);
}
