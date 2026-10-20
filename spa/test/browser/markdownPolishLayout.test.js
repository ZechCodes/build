import { afterAll, beforeAll, expect, it } from "vitest";
import { markdownHtml } from "../../src/core/markdown.js";
import { openLayoutSession, mountLayout } from "./layoutHarness.mjs";
import { MARKDOWN_POLISH_FIXTURE, FIXTURE_IMAGE, FIXTURE_STYLES, fixtureMarkup } from "./markdownPolishFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

for (const theme of ["light", "dark"]) {
  for (const width of [320, 390, 960]) {
    it(`uses borderless rows, keeps timing values whole and scrolls only the table at ${width}px in ${theme}`, async () => {
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
          const borders = [...table.querySelectorAll("table, thead, tbody, tr, th, td"), table].flatMap((element) => {
            const style = getComputedStyle(element);
            return ["Top", "Right", "Bottom", "Left"].flatMap((side) =>
              parseFloat(style[`border${side}Width`]) ? [`${element.tagName} ${side}: ${style[`border${side}Width`]}`] : []);
          });
          const rows = [...table.querySelectorAll("tbody tr")].map((row) => getComputedStyle(row).backgroundColor);
          const context = document.createElement("canvas").getContext("2d");
          context.fillStyle = rows[1];
          context.fillRect(0, 0, 1, 1);
          const shadeAlpha = context.getImageData(0, 0, 1, 1).data[3] / 255;
          const columnPadding = [...table.querySelectorAll("th:not(:last-child), td:not(:last-child)")]
            .map((cell) => parseFloat(getComputedStyle(cell).paddingRight));
          return { cells, borders, rows, shadeAlpha, columnPadding, scrolls: table.scrollWidth > table.clientWidth, pageWidth: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth), imageWidth: document.querySelector("img").getBoundingClientRect().width, hostWidth: document.querySelector(".thread-body").getBoundingClientRect().width, width: innerWidth };
        });
        expect(metrics.borders).toEqual([]);
        expect(metrics.rows[0]).toBe(metrics.rows[2]);
        expect(metrics.rows[1]).not.toBe(metrics.rows[0]);
        expect(metrics.shadeAlpha).toBeGreaterThan(0);
        expect(metrics.shadeAlpha).toBeLessThanOrEqual(.06);
        expect(metrics.columnPadding.every((padding) => padding >= 16)).toBe(true);
        expect(metrics.cells.every((cell) => cell.lines === 1)).toBe(true);
        expect(metrics.cells.every((cell) => cell.numbers === "tabular-nums" && cell.padding >= 8)).toBe(true);
        expect(metrics.cells[1].align).toBe("right");
        expect(metrics.scrolls).toBe(width === 320);
        expect(metrics.imageWidth).toBeLessThanOrEqual(metrics.hostWidth);
        expect(metrics.pageWidth).toBeLessThanOrEqual(metrics.width);
        await page.keyboard.press("Tab");
        const focus = await page.evaluate(() => {
          const table = document.querySelector(".mdtable");
          const style = getComputedStyle(table);
          return { active: document.activeElement === table, width: style.outlineWidth, style: style.outlineStyle };
        });
        expect(focus).toEqual({ active: true, width: "2px", style: "solid" });
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
