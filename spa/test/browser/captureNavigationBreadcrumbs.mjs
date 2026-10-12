// Offline evidence: production toolbar/controller/cache/CSS, without app boot.
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
const phase = process.argv[2] || "after";
const output = resolve(process.argv[3] || "/tmp/navigation-542-evidence");
await mkdir(output, { recursive: true });
const session = await openLayoutSession({ plugins: [navigationAppShim] });
const manifest = [];
try {
  for (const width of [1280, 390, 320]) for (const theme of ["light", "dark"]) {
    await session.withPage(async ({ page, basePath }) => {
      for (const longNames of [false, true]) {
        await mountNavigationBreadcrumbs(page, basePath, { theme, longNames });
        const stem = `${phase}-${width}px-${theme}-${longNames ? "long-names" : "short-names"}`;
        await page.screenshot({ path: `${output}/${stem}-workspace.png` });
        for (const select of ["workspace", "project"]) {
          const control = page.locator(`[data-select="${select}"]`);
          if (phase === "before" && select === "project" && await control.count() === 0) {
            await page.locator('[data-select="workspace"]').click(); await page.locator("[data-projects]").click();
          } else if (width < 900) await control.tap(); else await control.click();
          await page.locator(".tb-filter").waitFor();
          await page.waitForFunction(() => document.activeElement?.matches(".tb-filter"));
          await page.screenshot({ path: `${output}/${stem}-${select}-switcher-search-focused.png` });
          manifest.push({ phase, width, theme, longNames, switcher: select, focused: true, synchronousFocus: await page.evaluate(() => window.__navigationHarness.focusInGesture) });
          await page.keyboard.press("Escape"); await page.locator(".tbmenu").waitFor({ state: "detached" });
        }
      }
      for (const kind of ["workspaceTask", "projectTask"]) {
        await mountNavigationBreadcrumbs(page, basePath, { theme, kind });
        await page.screenshot({ path: `${output}/${phase}-${width}px-${theme}-${kind}-route.png` });
      }
    }, { width, height: width === 1280 ? 800 : width === 390 ? 844 : 740, hasTouch: width < 900 });
  }
  await writeFile(`${output}/${phase}-manifest.json`, JSON.stringify(manifest, null, 2));
} finally { await session.close(); }
console.log(output);
