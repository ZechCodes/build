// Offline production toolbar evidence; no app boot, bridge or device connection.
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

const phase = process.argv[2] || "after";
const output = resolve(process.argv[3] || "/tmp/navigation-618-evidence");
await mkdir(output, { recursive: true });
const session = await openLayoutSession({ plugins: [navigationAppShim] });
const measurements = [];
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
      const states = [["rest", null], ["chevron-hover", ".tb-switch"], ["project-hover", ".tb-project"],
        ["tasks-hover", ".tb-tasks"], ["leaf-hover", ".tb-leaf"], ["workspace-hover", ".tb-workspace"],
        ["keyboard-focus", ".tb-switch"]];
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
  console.log(JSON.stringify(measurements, null, 2));
} finally { await session.close(); }
