import { afterAll, beforeAll, expect, it } from "vitest";
import { markdownHtml } from "../../src/core/markdown.js";
import { openLayoutSession, mountLayout } from "./layoutHarness.mjs";
import { MARKDOWN_POLISH_FIXTURE, FIXTURE_IMAGE, FIXTURE_STYLES, fixtureMarkup } from "./markdownPolishFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

for (const theme of ["light", "dark"]) {
  for (const width of [320, 390, 960]) {
    it(`keeps timing values whole and scrolls only the table at ${width}px in ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await page.route("https://markdown-fixture.test/**", (route) => route.fulfill({ contentType: "image/svg+xml", body: FIXTURE_IMAGE }));
        await mountLayout(page, fixtureMarkup(markdownHtml(MARKDOWN_POLISH_FIXTURE)), { basePath, styles: FIXTURE_STYLES });
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        await page.evaluate(async () => {
          for (const image of document.images) { image.loading = "eager"; await image.decode(); }
        });
        const metrics = await page.evaluate(() => {
          const table = document.querySelector(".mdtable");
          const cells = [...table.querySelectorAll("td")].map((cell) => {
            const range = document.createRange();
            range.selectNodeContents(cell);
            const style = getComputedStyle(cell);
            return { lines: range.getClientRects().length, align: style.textAlign, numbers: style.fontVariantNumeric, padding: parseFloat(style.paddingTop) };
          });
          return { cells, scrolls: table.scrollWidth > table.clientWidth, pageWidth: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth), imageWidth: document.querySelector("img").getBoundingClientRect().width, hostWidth: document.querySelector(".thread-body").getBoundingClientRect().width, width: innerWidth };
        });
        expect(metrics.cells.every((cell) => cell.lines === 1)).toBe(true);
        expect(metrics.cells.every((cell) => cell.numbers === "tabular-nums" && cell.padding >= 8)).toBe(true);
        expect(metrics.cells[1].align).toBe("right");
        expect(metrics.scrolls).toBe(width === 320);
        expect(metrics.imageWidth).toBeLessThanOrEqual(metrics.hostWidth);
        expect(metrics.pageWidth).toBeLessThanOrEqual(metrics.width);
      }, { width, height: 600 });
    });
  }
}

it("wraps long prose cells without turning the table into a page-wide line", async () => {
  await session.withPage(async ({ page, basePath }) => {
    const source = "| Description | Result |\n| --- | ---: |\n| " + "A useful explanation with several words. ".repeat(12) + " | 100–149 ms |";
    await mountLayout(page, fixtureMarkup(markdownHtml(source)), { basePath, styles: FIXTURE_STYLES });
    const metrics = await page.evaluate(() => {
      const cells = document.querySelectorAll("td");
      const range = document.createRange();
      range.selectNodeContents(cells[0]);
      return { lines: range.getClientRects().length, width: cells[0].getBoundingClientRect().width, pageWidth: document.documentElement.scrollWidth };
    });
    expect(metrics.lines).toBeGreaterThan(1);
    expect(metrics.width).toBeLessThan(700);
    expect(metrics.pageWidth).toBeLessThanOrEqual(390);
  }, { width: 390, height: 600 });
});
