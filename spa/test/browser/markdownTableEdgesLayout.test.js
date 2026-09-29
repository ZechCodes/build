import { expect, it } from "vitest";
import { markdownHtml } from "../../src/core/markdown.js";
import { captureLayout, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// #253: a markdown table's edge shading is the sign that it scrolls, so a table
// that fits shows none, on whatever surface holds it. It used to be drawn
// always and hidden under covers painted in --panel; on any host that is not
// --panel — the addressed comment's tinted card, the reader's green bubble, a
// plan doc — the covers themselves showed as dark bands down both edges. Now
// the shading is driven by the table's own scroll position, so a table that
// fits has none, and one that overflows shades only the side with more to see.

const FITS = "| Kind | Form |\n|---|---|\n| task | #42 |\n| agent | @name |";
const OVERFLOWS = `| ${Array.from({ length: 14 }, (_, index) => `column ${index}`).join(" | ")} |\n|${"---|".repeat(14)}\n| ${Array.from({ length: 14 }, (_, index) => `value ${index}`).join(" | ")} |`;

/** The surfaces a table is read on, each with the background that is not
 *  --panel, since that is where covers in --panel showed. */
const HOSTS = {
  // Tinted the way the addressed comment's card was before #253 lightened it.
  "tinted task comment": (html) => `<ol class="task-timeline"><li class="task-entry task-comment task-comment-mentioned">
    <div class="task-comment-card" style="--task-comment-background:var(--accent-soft)"><div class="task-comment-body">${html}</div></div></li></ol>`,
  "reader's bubble": (html) => `<div class="thread-message user"><div class="thread-comment-card"><div class="thread-body markdown">${html}</div></div></div>`,
  "plan doc": (html) => `<div class="plan">${html}</div>`,
};

/** The shading each edge of the table's box shows, read from its own
 *  properties, and the pixels at the edges beside the pixel mid-row, with the
 *  cell text hidden so only backgrounds are sampled. */
async function edges(page) {
  const shading = await page.evaluate(() => {
    const table = document.querySelector(".mdtable");
    const style = getComputedStyle(table);
    const box = table.getBoundingClientRect();
    const row = table.querySelector("th").getBoundingClientRect();
    return {
      left: style.getPropertyValue("--mdtable-shade-left").trim(),
      right: style.getPropertyValue("--mdtable-shade-right").trim(),
      scrolls: table.scrollWidth > table.clientWidth,
      clip: { x: Math.round(box.left), y: Math.round(row.top + row.height / 2), width: Math.round(box.width), height: 1 },
    };
  });
  const png = await page.screenshot({ clip: shading.clip, type: "png" });
  return { ...shading, png };
}

/** Pixels of a one-row PNG screenshot, decoded in the page. */
async function rowPixels(page, png) {
  return page.evaluate(async (base64) => {
    const image = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const canvas = new OffscreenCanvas(image.width, image.height);
    const context = canvas.getContext("2d");
    context.drawImage(image, 0, 0);
    const data = context.getImageData(0, 0, image.width, 1).data;
    return Array.from({ length: image.width }, (_, x) => `${data[x * 4]},${data[x * 4 + 1]},${data[x * 4 + 2]}`);
  }, png.toString("base64"));
}

/** Runs in the page: the sheets as a browser without scroll-driven
 *  animations reads them (Firefox today, Safari before 26). It drops every
 *  animation-timeline declaration and every @supports block that asks for
 *  one, and keeps the rest — `animation` included. */
function withoutScrollTimelines() {
  const asksForTimelines = (rule) => rule instanceof CSSSupportsRule && rule.conditionText.includes("animation-timeline");
  const strip = (container) => {
    for (let index = container.cssRules.length - 1; index >= 0; index -= 1) {
      const rule = container.cssRules[index];
      if (asksForTimelines(rule)) container.deleteRule(index);
      else if (rule instanceof CSSImportRule) strip(rule.styleSheet);
      else if (rule instanceof CSSStyleRule) rule.style.removeProperty("animation-timeline");
      else if (rule.cssRules) strip(rule);
    }
  };
  for (const sheet of document.styleSheets) strip(sheet);
}

const HIDE_TEXT = ".mdtable th, .mdtable td { color:transparent !important; } body { padding:24px; width:560px; }";

for (const theme of ["dark", "light"]) {
  for (const [host, wrap] of Object.entries(HOSTS)) {
    it(`shades no edge of a table that fits, ${host}, ${theme}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await mountLayout(page, `<link rel="stylesheet" href="${basePath}src/styles/tasks.css">${wrap(markdownHtml(FITS))}`, { basePath, styles: HIDE_TEXT });
        await page.evaluate((name) => { document.documentElement.dataset.theme = name; }, theme);
        const found = await edges(page);
        expect(found.scrolls).toBe(false);
        expect([found.left, found.right]).toEqual(["0px", "0px"]);
        const pixels = await rowPixels(page, found.png);
        const middle = pixels[Math.floor(pixels.length / 2)];
        // Every pixel of the row is the host's own background: no band at either end.
        expect(pixels.slice(0, 24)).toEqual(Array(24).fill(middle));
        expect(pixels.slice(-24)).toEqual(Array(24).fill(middle));
        await captureLayout(page, `mdtable-fits-${host.replace(/\W+/g, "-")}-${theme}.png`);
      });
    });
  }

  it(`shades no edge of a table that fits where scroll timelines are unsupported, ${theme}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, `<link rel="stylesheet" href="${basePath}src/styles/tasks.css">${HOSTS["tinted task comment"](markdownHtml(FITS))}`, { basePath, styles: HIDE_TEXT });
      await page.evaluate((name) => { document.documentElement.dataset.theme = name; }, theme);
      await page.evaluate(withoutScrollTimelines);
      const found = await edges(page);
      expect([found.left, found.right]).toEqual(["0px", "0px"]);
      const pixels = await rowPixels(page, found.png);
      const middle = pixels[Math.floor(pixels.length / 2)];
      expect(pixels.slice(0, 24)).toEqual(Array(24).fill(middle));
      expect(pixels.slice(-24)).toEqual(Array(24).fill(middle));
    });
  });

  it(`shades only the side there is more table to reach, ${theme}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, `<link rel="stylesheet" href="${basePath}src/styles/tasks.css">${HOSTS["tinted task comment"](markdownHtml(OVERFLOWS))}`, { basePath, styles: HIDE_TEXT });
      await page.evaluate((name) => { document.documentElement.dataset.theme = name; }, theme);
      const atStart = await edges(page);
      expect(atStart.scrolls).toBe(true);
      expect([atStart.left, atStart.right]).toEqual(["0px", "14px"]);
      const startPixels = await rowPixels(page, atStart.png);
      const middle = startPixels[Math.floor(startPixels.length / 2)];
      expect(startPixels.slice(0, 24)).toEqual(Array(24).fill(middle));
      expect(startPixels.at(-2)).not.toBe(middle);

      await page.evaluate(() => { const table = document.querySelector(".mdtable"); table.scrollLeft = table.scrollWidth; });
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      const atEnd = await edges(page);
      expect([atEnd.left, atEnd.right]).toEqual(["14px", "0px"]);
      const endPixels = await rowPixels(page, atEnd.png);
      expect(endPixels[1]).not.toBe(middle);
      expect(endPixels.slice(-24)).toEqual(Array(24).fill(middle));
      await captureLayout(page, `mdtable-overflows-end-${theme}.png`);
    });
  });
}
