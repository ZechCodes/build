// @vitest-environment jsdom
import { expect, it } from 'vitest';
import { createWorkspaceWizardBodyHtml } from '../src/core/createWorkspaceWizard.js';
import { createWorkspaceWizardModel, optionalProjectName } from '../src/core/createWorkspaceWizardModel.js';
import { refusalDraftPage } from '../src/core/createWorkspaceEnvironmentDraft.js';
const target={kind:'environment',id:'e',key:'d/e',deviceId:'d',name:'Platform',sources:[]};
it('renders selection, optional name, Advanced and Create together on page one',()=>{
 document.body.innerHTML=createWorkspaceWizardBodyHtml({page:1,sections:[{title:'Environments',kind:'environments',targets:[target]}],target});
 expect(document.querySelector('#create-work-input')?.required).toBe(false);
 expect(document.querySelector('[data-create-go]')).not.toBeNull();
 expect(document.querySelector('[data-create-target]')?.getAttribute('aria-pressed')).toBe('true');
 expect(document.querySelector('[data-create-advanced]')?.open).toBe(false);
 expect(document.body.textContent).toContain('Name (optional)');
});
it('always renders Advanced with a prompt before selection',()=>{
 document.body.innerHTML=createWorkspaceWizardBodyHtml({page:1});
 expect(document.querySelector('[data-create-advanced]')?.textContent).toContain('Choose a project or environment first.');
});
it('derives optional project names and the lowest free environment-local suffix',()=>{
 const projects=[{deviceId:'d',environment_id:'e',name:'Platform'},{deviceId:'d',environment_id:'e',name:'Platform 3'},{deviceId:'other',environment_id:'e',name:'Platform 2'}];
 expect(optionalProjectName({target,name:''},{})).toBe('Platform');
 expect(optionalProjectName({target,name:'  '},{projects})).toBe('Platform 2');
 expect(optionalProjectName({target:{...target,kind:'new-environment'},name:''},{projects})).toBe('Platform');
 expect(optionalProjectName({target:{...target,kind:'project'},name:''},{projects})).toBe('');
 expect(optionalProjectName({target,name:'My work'},{projects})).toBe('My work');
});
it('presets page one and uses singular project, workspace and task counts',()=>{
 const model=createWorkspaceWizardModel({environmentId:'e',deviceId:'d',support:{d:true},environments:[target],projects:[{id:'p',deviceId:'d',environment_id:'e',name:'Board',total_tasks:1}],workspaces:[{id:'w',deviceId:'d',project_id:'p'}]});
 expect(model.page).toBe(1);
 expect(model.sections.find(s=>s.kind==='environments').targets[0].description).toBe('1 project · 1 workspace');
 expect(model.sections.find(s=>s.kind==='projects').targets[0].description).toBe('1 workspace · 1 task');
});
it('routes refusals to configuration only for new-environment failures',()=>{
 expect(refusalDraftPage(target,{})).toBe(1);
 expect(refusalDraftPage({...target,kind:'new-environment'},{details:{creation:{step:'project'}}})).toBe(1);
 expect(refusalDraftPage({...target,kind:'new-environment'},{details:{creation:{step:'environment'}}})).toBe(2);
});
