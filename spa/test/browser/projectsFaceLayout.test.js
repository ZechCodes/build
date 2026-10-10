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
      }
    }, { width, height, hasTouch });
  }, 60_000);
}
