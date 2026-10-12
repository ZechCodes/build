import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { inspectProjectsFace, mountProjectsFace } from "./projectsFaceSeed.mjs";

for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 800, hasTouch: false },
  { label: "phone", width: 390, height: 844, hasTouch: true },
]) {
  it(`lists quiet projects alphabetically on transparent blocks on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      for (const theme of ["light", "dark"]) {
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        const blocks = await inspectProjectsFace(page);
        expect(blocks.some((block) => block.active)).toBe(true);
        for (const block of blocks) {
          expect(block.background).toBe("rgba(0, 0, 0, 0)");
          expect(block.border).toBe("0px");
          expect(block.shadow).toBe("none");
        }
        expect(blocks.map((block) => block.name)).toEqual(["Alpha", "Bravo", "Build", "Zulu"]);
        expect(await page.locator("#inbox-list > .inbox-recent").count()).toBe(0);
        expect(await page.locator('[data-recent-toggle="projects-review/build"]').count()).toBe(1);
        expect(await page.locator('[data-project-fold][aria-expanded="true"]').count()).toBe(4);
        expect(await page.locator('.inbox-project .inbox-recent-toggle[aria-expanded="false"]').count()).toBe(2);
        expect(await page.locator('.inbox-project[data-project="projects-review/alpha"] .inbox-entry').count()).toBe(0);
        expect(await page.locator('[data-recent-toggle="projects-review/alpha"] .inbox-recent-count').textContent()).toBe("1");
      }
      await page.locator('[data-recent-toggle="projects-review/alpha"]').click();
      await page.locator('.inbox-recent [data-key="workspace:projects-review/alpha-quiet"]').waitFor();
      expect(await page.locator('.inbox-project[data-project="projects-review/alpha"] .inbox-quiet').count()).toBe(1);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`indents each project's Recent into its content column on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await page.evaluate(() => window.__layoutModules.view.setInboxView("inbox"));
      const ungrouped = page.locator("#inbox-list > .inbox-recent > .inbox-recent-toggle");
      await ungrouped.waitFor();
      const ungroupedBox = await ungrouped.boundingBox();
      const ungroupedCaret = await ungrouped.locator(".inbox-recent-caret").boundingBox();
      expect(ungroupedBox.x).toBe(label === "phone" ? 33 : 20);
      expect(ungroupedCaret.x).toBe(ungroupedBox.x + 6);

      await page.evaluate(() => window.__layoutModules.view.setInboxView("projects"));
      const toggles = page.locator(".inbox-project .inbox-recent-toggle");
      expect(await toggles.count()).toBe(2);
      for (const expanded of [false, true]) {
        for (const theme of ["light", "dark"]) {
          await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
          for (const toggle of await toggles.all()) {
            const block = toggle.locator("..").locator("..");
            const caret = await toggle.locator(".inbox-recent-caret").boundingBox();
            const name = await block.locator(".inbox-project-name").boundingBox();
            const fold = await block.locator(".inbox-fold").boundingBox();
            expect(await toggle.getAttribute("aria-expanded")).toBe(String(expanded));
            expect(caret.x).toBeGreaterThanOrEqual(name.x);
            expect(caret.x).toBeGreaterThan(fold.x);
            if (expanded) {
              const quietTitle = await block.locator(".inbox-recent .stitle").boundingBox();
              expect(caret.x).toBe(quietTitle.x);
            }
          }
        }
        if (!expanded) for (const toggle of await toggles.all()) await toggle.click();
      }
    }, { width, height, hasTouch });
  }, 60_000);
}
