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
const collapsedStates = [["rest", null], ["inbox-hover", "#inbox-open"], ["inbox-focus", "#inbox-open"], ["chevron-hover", ".tb-switch"], ["project-hover", ".tb-project"],
  ["tasks-hover", ".tb-tasks"], ["leaf-hover", ".tb-leaf"], ["workspace-hover", ".tb-workspace"],
  ["keyboard-focus", ".tb-switch"]];
function captureStates(width) {
  const header = [["inbox-view", '[data-inbox-view="inbox"]'], ["projects-view", '[data-inbox-view="projects"]'], ["new-project", "#inbox-new-project"]];
  if (width > 900) header.push(["pin", "#inbox-collapse"]);
  const openStates = [["rest", null], ...header.flatMap(([name, selector]) => [[`${name}-hover`, selector], [`${name}-focus`, selector]])];
  return [
    ...collapsedStates.map(([name, selector]) => ["collapsed", name, selector]),
    ...openStates.map(([name, selector]) => ["open", name, selector]),
    ["popover", "new-project-hover", "#inbox-new-project"],
  ];
}
try {
  for (const width of [390, 1280]) for (const theme of ["light", "dark"]) {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "projectTask", inboxHeader: true });
      await page.evaluate(() => window.__layoutModules.inbox.setInboxCollapsed(true, { animate: false, persist: false, reveal: false }));
      await page.waitForTimeout(300);
      const spacing = await page.evaluate(() => {
        const switcher = document.querySelector('[data-select="project"]');
        const icon = switcher.querySelector("svg, span").getBoundingClientRect();
        const crumb = document.querySelector("[data-project-return]").getBoundingClientRect();
        const target = switcher.getBoundingClientRect();
        return { iconToCrumb: crumb.left - icon.right, targetWidth: target.width, targetHeight: target.height };
      });
      for (const [layout, state, selector] of captureStates(width)) {
        await page.evaluate((layout) => {
          document.activeElement?.blur();
          return window.__layoutModules.inbox.setInboxCollapsed(layout !== "open", { animate: false, persist: false, reveal: layout === "popover" });
        }, layout);
        await page.mouse.move(0, 0);
        if (state.endsWith("focus")) { await page.keyboard.press("Tab"); await page.locator(selector).focus(); }
        else if (selector) await page.locator(selector).hover();
        await page.waitForTimeout(200);
        const clip = await page.locator("#toolbar").boundingBox();
        // #inbox-open is in the bar visually, outside the toolbar's DOM and box.
        clip.width += clip.x;
        clip.x = 0;
        if (layout !== "collapsed") {
          const header = await page.locator(".inbox-head").boundingBox();
          clip.height = Math.max(clip.height, header.y + header.height - clip.y + 10);
        }
        await page.screenshot({ path: `${output}/${phase}-${width}px-${theme}-${layout}-${state}.png`, clip, animations: "disabled" });
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
    for (const [layout, state] of captureStates(width)) {
      const images = await Promise.all(["before", "after"].map(async (phase) => {
        const bytes = await readFile(`${output}/${phase}-${width}px-${theme}-${layout}-${state}.png`);
        captureWidth = bytes.readUInt32BE(16);
        return `<div><b>${phase}</b><img src="data:image/png;base64,${bytes.toString("base64")}"></div>`;
      }));
      const label = layout === "open" ? (width > 900 ? "docked inbox" : "phone overlay") : layout;
      rows.push(`<section><h2>${label} · ${state}</h2><div class="pair">${images.join("")}</div></section>`);
    }
    await session.withPage(async ({ page }) => {
      await page.setContent(`<!doctype html><html><head><style>body{margin:16px;font:16px Arial;background:#eef0f2;color:#17232b}h1{font-size:24px}h2{font-size:16px;margin:16px 0 6px}.pair{display:grid;grid-template-columns:repeat(2,${captureWidth}px);gap:16px}img{display:block;margin-top:4px;max-width:100%}</style></head><body><h1>#618 · round 3 · ${width}px viewport · ${theme}</h1><p>Before: 08eb2d33 · after: docked-row fix · production header, hover and keyboard focus</p>${rows.join("")}</body></html>`);
      await page.screenshot({ path: `${output}/comparison-${width}px-${theme}.png`, fullPage: true });
    }, { width: captureWidth * 2 + 48, height: 844 });
  }
}
