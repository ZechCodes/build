import { mkdir } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountNavigationPicker } from "./navigationPickerSeed.mjs";

const phase = process.argv[2] || "after";
const directory = process.argv[3] || "/tmp/navigation-picker-captures";
await mkdir(directory, { recursive: true });
const baselineToolbar = phase === "before" ? execFileSync("git", ["show", "main:spa/src/core/toolbar.js"], { encoding: "utf8" }) : null;
const baseline = { name: "navigation-picker-baseline", enforce: "pre", load(id) {
  return baselineToolbar && id.endsWith("/src/core/toolbar.js") ? baselineToolbar : null;
} };
const session = await openLayoutSession({ plugins: [navigationAppShim, baseline] });
try {
  for (const width of [1280, 390]) for (const theme of ["light", "dark"]) {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationPicker(page, basePath, { theme });
      await page.locator('[data-select="project"]').click();
      await page.locator(".tbmenu-list .mi").first().waitFor();
      await page.screenshot({ path: `${directory}/${phase}-${width}-${theme}.png` });
    }, { width, height: width === 1280 ? 800 : 844, hasTouch: width === 390 });
  }
} finally { await session.close(); }
