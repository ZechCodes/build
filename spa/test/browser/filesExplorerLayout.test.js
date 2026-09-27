import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountFilesExplorer, openFile, row, stageDesktop } from "./filesExplorerSeed.mjs";

// What jsdom cannot see of the explorer (#151): the tabs stay one row however
// many are open, the preview head stands under them rather than over them, a
// row steps in one indent per directory above it, and the open file and the
// keyboard selection are drawn as two different things.

it("keeps the open tabs to one sideways-scrolling row over the preview head", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountFilesExplorer(page, basePath);
    await row(page, "spa").click();
    await row(page, "spa/src").click();
    await row(page, "spa/src/app.js").waitFor();
    for (const path of ["AGENTS.md", "ARCHITECTURE.md", "README.md", "CLAUDE.md", "spa/package.json", "spa/vite.config.js", "spa/src/app.js", "spa/src/main.js"])
      await openFile(page, path);
    const geometry = await page.evaluate(() => {
      const strip = document.querySelector(".ftabs");
      const tabs = [...strip.querySelectorAll(".ftab")].map((tab) => tab.getBoundingClientRect());
      return {
        count: tabs.length,
        rows: new Set(tabs.map((tab) => Math.round(tab.top))).size,
        overflows: strip.scrollWidth > strip.clientWidth,
        stripBottom: strip.getBoundingClientRect().bottom,
        headTop: document.querySelector(".fphead").getBoundingClientRect().top,
        activeVisible: (() => {
          const active = strip.querySelector(".ftab.active").getBoundingClientRect();
          const box = strip.getBoundingClientRect();
          return active.left >= box.left - 1 && active.right <= box.right + 1;
        })(),
      };
    });
    expect(geometry).toMatchObject({ count: 8, rows: 1, overflows: true });
    expect(geometry.headTop).toBeGreaterThanOrEqual(geometry.stripBottom - 1);
    expect(geometry.activeVisible).toBe(true);
  }, { width: 1000, height: 760 });
}, 60_000);

it("steps each row in one indent per directory and draws the open file apart from the selection", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountFilesExplorer(page, basePath);
    await stageDesktop(page);
    // The cursor fill animates for --motion-fast after the last click.
    await page.waitForFunction(() => {
      const cursor = document.querySelector('.frow[data-path="spa/src/styles.css"]');
      const plain = document.querySelector('.frow[data-path="spa/src/app.js"]');
      return getComputedStyle(cursor).backgroundColor !== getComputedStyle(plain).backgroundColor;
    });
    const drawn = await page.evaluate(() => {
      const rowOf = (path) => document.querySelector(`.frow[data-path="${path}"]`);
      const nameLeft = (path) => rowOf(path).querySelector(".fname").getBoundingClientRect().left;
      const fill = (path) => getComputedStyle(rowOf(path)).backgroundColor;
      return {
        steps: [nameLeft("spa/src") - nameLeft("spa"), nameLeft("spa/src/app.js") - nameLeft("spa/src")],
        open: fill("spa/src/main.js"),
        cursor: fill("spa/src/styles.css"),
        plain: fill("spa/src/app.js"),
      };
    });
    expect(drawn.steps).toEqual([14, 14]);
    expect(drawn.open).not.toBe(drawn.cursor);
    expect(drawn.cursor).not.toBe(drawn.plain);
  });
}, 60_000);
