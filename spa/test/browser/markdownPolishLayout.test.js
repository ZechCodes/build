import { afterAll, beforeAll, expect, it } from "vitest";
import { markdownHtml } from "../../src/core/markdown.js";
import { openLayoutSession, mountLayout, loadBrowserModules } from "./layoutHarness.mjs";
import { MARKDOWN_POLISH_FIXTURE, FIXTURE_IMAGE, FIXTURE_STYLES, fixtureMarkup } from "./markdownPolishFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

for (const theme of ["light", "dark"]) {
  for (const width of [320, 390, 960]) {
    it(`uses borderless rows, keeps timing values whole and fits all three timing columns at ${width}px in ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        let imageRequests = 0;
        await page.route("https://markdown-fixture.test/**", (route) => {
          imageRequests += 1;
          return route.fulfill({ contentType: "image/svg+xml", body: FIXTURE_IMAGE });
        });
        await mountLayout(page, fixtureMarkup(markdownHtml(MARKDOWN_POLISH_FIXTURE)), { basePath, styles: FIXTURE_STYLES });
        expect(await page.locator("img").count()).toBe(0);
        expect(imageRequests).toBe(0);
        await loadBrowserModules(page, { tableFocus: "src/core/markdownTableFocus.js", images: "src/core/markdownImages.js" }, basePath);
        await page.evaluate(() => {
          window.__tableFocusDispose = window.__layoutModules.tableFocus.installMarkdownTableFocus(document);
          window.__imagesDispose = window.__layoutModules.images.installMarkdownImages(document);
        });
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        await page.locator(".md-image-load").click();
        await page.evaluate(async () => {
          for (const image of document.images) { image.loading = "eager"; await image.decode(); }
        });
        expect(imageRequests).toBe(1);
        const metrics = await page.evaluate(() => {
          const table = document.querySelector(".mdtable");
          const cells = [...table.querySelectorAll("td")].map((cell) => {
            const range = document.createRange();
            range.selectNodeContents(cell);
            const style = getComputedStyle(cell);
            return { lines: range.getClientRects().length, align: style.textAlign, numbers: style.fontVariantNumeric, padding: parseFloat(style.paddingTop), whitespace: style.whiteSpace, width: cell.getBoundingClientRect().width, left: cell.getBoundingClientRect().left, right: cell.getBoundingClientRect().right };
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
        expect(metrics.cells.filter((_, index) => index % 3 !== 0).every((cell) => cell.lines === 1)).toBe(true);
        expect(metrics.cells.every((cell) => cell.numbers === "tabular-nums" && cell.padding >= 8)).toBe(true);
        expect(metrics.cells[1].align).toBe("right");
        expect(metrics.cells.filter((_, index) => index % 3 === 0).every((cell) => cell.whitespace === "normal")).toBe(true);
        expect(metrics.scrolls).toBe(false);
        expect(metrics.cells.slice(0, 3).every((cell) => cell.left >= 0 && cell.right <= width)).toBe(true);
        expect(metrics.imageWidth).toBeLessThanOrEqual(metrics.hostWidth);
        expect(metrics.pageWidth).toBeLessThanOrEqual(metrics.width);
        expect(await page.locator(".mdtable").getAttribute("tabindex")).toBeNull();
        await page.evaluate(() => { window.__tableFocusDispose(); window.__imagesDispose(); });
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
      return { lines: range.getClientRects().length, width: cells[0].getBoundingClientRect().width, maxWidth: getComputedStyle(cells[0]).maxWidth, pageWidth: document.documentElement.scrollWidth };
    });
    expect(metrics.maxWidth).toBe("none");
    expect(metrics.lines).toBeGreaterThan(1);
    expect(metrics.width).toBeLessThan(700);
    expect(metrics.pageWidth).toBeLessThanOrEqual(390);
  }, { width: 390, height: 600 });
});

it("adds a keyboard stop only while a measured table overflows and updates on resize", async () => {
  await session.withPage(async ({ page, basePath }) => {
    const source = "| First | Second | Third | Fourth |\n| ---: | ---: | ---: | ---: |\n| 100–149 ms | 105–178 ms | 100–149 ms | 105–178 ms |";
    await mountLayout(page, fixtureMarkup(markdownHtml(source)), { basePath, styles: FIXTURE_STYLES });
    await loadBrowserModules(page, { tableFocus: "src/core/markdownTableFocus.js" }, basePath);
    await page.evaluate(() => { window.__tableFocusDispose = window.__layoutModules.tableFocus.installMarkdownTableFocus(document); });
    await expect.poll(() => page.locator(".mdtable").getAttribute("tabindex")).toBe("0");
    await page.keyboard.press("Tab");
    expect(await page.evaluate(() => {
      const table = document.querySelector(".mdtable");
      return { active: document.activeElement === table, outline: getComputedStyle(table).outlineWidth };
    })).toEqual({ active: true, outline: "2px" });
    await page.setViewportSize({ width: 960, height: 600 });
    await expect.poll(() => page.locator(".mdtable").getAttribute("tabindex")).toBeNull();
    await page.setViewportSize({ width: 320, height: 600 });
    await expect.poll(() => page.locator(".mdtable").getAttribute("tabindex")).toBe("0");
    await page.evaluate(() => window.__tableFocusDispose());
  }, { width: 320, height: 600 });
});
