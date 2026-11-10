import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({plugins:[navigationAppShim]}); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const width of [1280,390,320]) for (const theme of ["light","dark"]) {
  it(`keeps the progress badge visual and its label clean at ${width}px in ${theme}`, async () => {
    await session.withPage(async ({page,basePath}) => {
      await page.emulateMedia({reducedMotion:'reduce'});
      await mountLayout(page, '<button id="opener">New workspace</button>', {basePath});
      await loadBrowserModules(page, {progress:"src/core/createWorkspaceProgress.js",theme:"src/core/theme.js"}, basePath);
      await page.evaluate(async theme => {
        const {progress,theme:appearance} = window.__layoutModules;
        appearance.applyTheme(theme);
        const row = {operationId:"attempt",state:"executing",request:{method:"environment.create_project"}};
        window.__progressDialog = progress.openCreateWorkspaceProgress({
          operationId:"attempt",deviceId:"badge-device",environmentSupport:{create:true,createProject:true},
          target:{kind:"project",id:"project",environment_id:"env",deviceId:"badge-device",name:"Payments",summaryLabel:"Sources › Payments",deviceLabel:"Laptop"},
          writeOperations:{readOperations:()=>[row],get:async()=>row,subscribe:()=>()=>{}},
        });
        await window.__progressDialog.ready;
      }, theme);
      await page.locator('.create-target-summary').waitFor({state:'visible'});
      const facts = await page.locator('.create-target-summary').evaluate(summary => {
        const badge = summary.querySelector('.identity-badge');
        const letters = badge.querySelector('.identity-letters');
        const rect = node => {const {x,y,width,height}=node.getBoundingClientRect(); return [x,y,width,height];};
        const before = [rect(badge),rect(summary),rect(badge.parentElement.nextElementSibling)];
        const content = badge.innerHTML;
        badge.innerHTML = '<circle cx="8" cy="8" r="4" fill="currentColor"/>';
        const old = [rect(badge),rect(summary),rect(badge.parentElement.nextElementSibling)];
        badge.innerHTML = content;
        return {text:summary.textContent.replace(/\s+/g,' ').trim(),badgeText:badge.textContent,
          hidden:badge.getAttribute('aria-hidden'),before,old,
          glyph:getComputedStyle(badge.querySelector('.identity-letters'),'::before').content,
          size:rect(badge).slice(2),expectedGlyph:letters.dataset.glyph};
      });
      expect(facts.text).toBe("Sources › Payments Laptop");
      expect(facts.badgeText).toBe("");
      expect(facts.hidden).toBe("true");
      expect(facts.size).toEqual([16,16]);
      expect(facts.glyph).toBe('"PA"');
      expect(facts.expectedGlyph).toBe("PA");
      expect(facts.before).toEqual(facts.old);
      expect(await page.getByRole('dialog',{name:'Creating…',exact:true}).count()).toBe(1);
      await captureLayout(page,`creation-progress-${width}-${theme}.png`);
      await page.evaluate(() => window.__progressDialog.close());
    }, {width,height:844});
  }, 30_000);
}
