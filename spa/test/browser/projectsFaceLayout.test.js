import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { inspectProjectsFace, inspectActiveBlock, mountProjectsFace, openRecent } from "./projectsFaceSeed.mjs";

// The project name's left edge, as #514 measured it: every child of a block
// stands on it (#516).
const CONTENT_X = { desktop: 51, phone: 64 };

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
              for (const quietTitle of await block.locator(".inbox-recent .stitle").all()) {
                expect(caret.x).toBe((await quietTitle.boundingBox()).x);
              }
            }
          }
        }
        if (!expanded) for (const toggle of await toggles.all()) await toggle.click();
      }
    }, { width, height, hasTouch });
  }, 60_000);

  it(`stands a block's rows and Recent on one content column on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const block = await inspectActiveBlock(page);
      expect(block.name.x).toBe(CONTENT_X[label]);
      expect(block.caret.x).toBe(CONTENT_X[label]);
      expect(block.rows.map((row) => row.key)).toEqual(["workspace:projects-review/build-live",
        "tracker_task:land-tracker", "tracker_task:release-check", "workspace:projects-review/build-quiet"]);
      for (const row of block.rows) expect(row.title.x, row.key).toBe(CONTENT_X[label]);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`keeps a quiet row's name readable beside its facts on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const quiet = (await inspectActiveBlock(page)).rows.find((row) => row.facts);
      expect(quiet.text).toBe("Older workspace");
      expect(quiet.clipped).toBe(false);
      expect(quiet.facts.x).toBeGreaterThanOrEqual(quiet.title.right);
      expect(quiet.facts.x).toBeGreaterThan(quiet.row.x);
      expect(quiet.at).toBe(true);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`keeps every right-edge control on one column on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const block = await inspectActiveBlock(page);
      const column = block.create.right;
      const [live, ...quiet] = block.rows;
      expect(live.done.right).toBe(column);
      for (const row of quiet) {
        if (row.watch) expect(row.watch.right, row.key).toBe(column);
        // The eye's glyph stands where the head's + glyph does, whatever its touch target.
        if (row.watch) expect(Math.abs(row.watchGlyph.right - block.createGlyph.right), row.key).toBeLessThanOrEqual(1);
        if (row.facts) expect(row.facts.right, row.key).toBe(column);
      }
      expect(block.count.right).toBe(column);
      expect(quiet.filter((row) => row.watch)).toHaveLength(2);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`fills the active block's Recent heading like its head on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      for (const theme of ["light", "dark"]) {
        await page.evaluate((theme) => {
          document.documentElement.dataset.theme = theme;
          for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
        }, theme);
        const block = await inspectActiveBlock(page);
        expect(block.headFill, theme).toBe(block.accentSoft);
        expect(block.toggleFill, theme).toBe(block.accentSoft);
        expect(block.toggle.x, theme).toBe(block.head.x);
        expect(block.toggle.right, theme).toBe(block.head.right);
        // The rows keep their own fills: the standing row's accent and hover read as before.
        for (const row of block.rows) expect(row.fill, `${theme} ${row.key}`).toBe(block.panel);
        const inactive = await page.locator('[data-recent-toggle="projects-review/alpha"]')
          .evaluate((toggle) => getComputedStyle(toggle).backgroundColor);
        expect(inactive, theme).toBe("rgba(0, 0, 0, 0)");
      }
    }, { width, height, hasTouch });
  }, 60_000);
}
