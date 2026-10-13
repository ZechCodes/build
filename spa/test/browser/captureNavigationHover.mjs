// Offline production toolbar evidence; no app boot, bridge or device connection.
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

const phase = process.argv[2] || "after";
const output = resolve(process.argv[3] || "/tmp/navigation-618-evidence");
await mkdir(output, { recursive: true });
const session = await openLayoutSession({ plugins: [navigationAppShim] });
const measurements = [];
const states = [["rest", null], ["chevron-hover", ".tb-switch"], ["project-hover", ".tb-project"],
  ["tasks-hover", ".tb-tasks"], ["leaf-hover", ".tb-leaf"], ["workspace-hover", ".tb-workspace"],
  ["keyboard-focus", ".tb-switch"]];
try {
  for (const width of [390, 1280]) for (const theme of ["light", "dark"]) {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "projectTask" });
      const spacing = await page.evaluate(() => {
        const switcher = document.querySelector('[data-select="project"]');
        const icon = switcher.querySelector("svg, span").getBoundingClientRect();
        const crumb = document.querySelector("[data-project-return]").getBoundingClientRect();
        const target = switcher.getBoundingClientRect();
        return { iconToCrumb: crumb.left - icon.right, targetWidth: target.width, targetHeight: target.height };
      });
      for (const [state, selector] of states) {
        await page.mouse.move(0, 0);
        if (state === "keyboard-focus") { await page.keyboard.press("Tab"); await page.locator(selector).focus(); }
        else if (selector) await page.locator(selector).hover();
        await page.waitForTimeout(200);
        const clip = await page.locator("#toolbar").boundingBox();
        await page.screenshot({ path: `${output}/${phase}-${width}px-${theme}-${state}.png`, clip, animations: "disabled" });
      }
      measurements.push({ phase, width, theme, ...spacing });
    }, { width, height: 844 });
  }
  await writeFile(`${output}/${phase}-measurements.json`, JSON.stringify(measurements, null, 2));
  if (phase === "after") await captureComparisons();
  console.log(JSON.stringify(measurements, null, 2));
} finally { await session.close(); }

async function captureComparisons() {
  for (const width of [390, 1280]) for (const theme of ["light", "dark"]) {
    const rows = [];
    let captureWidth = width;
    for (const [state] of states) {
      const images = await Promise.all(["before", "after"].map(async (phase) => {
        const bytes = await readFile(`${output}/${phase}-${width}px-${theme}-${state}.png`);
        captureWidth = bytes.readUInt32BE(16);
        return `<div><b>${phase}</b><img src="data:image/png;base64,${bytes.toString("base64")}"></div>`;
      }));
      rows.push(`<section><h2>${state}</h2><div class="pair">${images.join("")}</div></section>`);
    }
    await session.withPage(async ({ page }) => {
      await page.setContent(`<!doctype html><html><head><style>body{margin:16px;font:16px Arial;background:#eef0f2;color:#17232b}h1{font-size:24px}h2{font-size:16px;margin:16px 0 6px}.pair{display:grid;grid-template-columns:repeat(2,${captureWidth}px);gap:16px}img{display:block;margin-top:4px;max-width:100%}</style></head><body><h1>#618 · ${width}px viewport · ${theme}</h1><p>Original toolbar pixels · before / after · every crumb, switcher and keyboard focus</p>${rows.join("")}</body></html>`);
      await page.screenshot({ path: `${output}/comparison-${width}px-${theme}.png`, fullPage: true });
    }, { width: captureWidth * 2 + 48, height: 844 });
  }
}
