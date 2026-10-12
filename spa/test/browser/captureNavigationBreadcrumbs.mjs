// Offline evidence: production toolbar/controller/cache/CSS, without app boot.
import { mkdir, readFile, writeFile } from "node:fs/promises";
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
        await page.screenshot({ animations: "disabled", caret: "initial", path: `${output}/${stem}-workspace.png` });
        for (const select of ["workspace", "project"]) {
          const control = page.locator(`[data-select="${select}"]`);
          if (phase === "before" && select === "project" && await control.count() === 0) {
            await page.locator('[data-select="workspace"]').click(); await page.locator("[data-projects]").click();
          } else if (width < 900) await control.tap(); else await control.click();
          await page.locator(".tb-filter").waitFor();
          await page.waitForFunction(() => document.activeElement?.matches(".tb-filter"));
          await page.screenshot({ animations: "disabled", caret: "initial", path: `${output}/${stem}-${select}-switcher-search-focused.png` });
          manifest.push({ phase, width, theme, longNames, switcher: select, focused: true, synchronousFocus: await page.evaluate(() => window.__navigationHarness.focusInGesture) });
          await page.keyboard.press("Escape"); await page.locator(".tbmenu").waitFor({ state: "detached" });
        }
      }
      for (const kind of ["workspaceTask", "projectTask"]) {
        await mountNavigationBreadcrumbs(page, basePath, { theme, kind });
        await page.screenshot({ animations: "disabled", caret: "initial", path: `${output}/${phase}-${width}px-${theme}-${kind}-route.png` });
      }
    }, { width, height: width === 1280 ? 800 : width === 390 ? 844 : 740, hasTouch: width < 900 });
  }
  await writeFile(`${output}/${phase}-manifest.json`, JSON.stringify(manifest, null, 2));
  if (phase === "after") await captureComparisonSheets(session);
} finally { await session.close(); }
console.log(output);


/** Review sheets show original screenshot pixels side by side. The complete
 * viewport captures remain separate; crops here concentrate on bar and menu. */
async function captureComparisonSheets(session) {
  for (const width of [1280, 390, 320]) for (const theme of ["light", "dark"]) {
    const cases = [
      ["short-names-workspace", "Short names", 140],
      ["long-names-workspace", "Long names", 140],
      ["short-names-workspace-switcher-search-focused", "Sibling switcher · short names · search focused", 440],
      ["long-names-workspace-switcher-search-focused", "Sibling switcher · long names · search focused", 440],
      ["short-names-project-switcher-search-focused", "Global switcher · short names · search focused", 440],
      ["long-names-project-switcher-search-focused", "Global switcher · long names · search focused", 440],
      ["workspaceTask-route", "Workspace-hosted task", 160],
      ["projectTask-route", "Project-hosted task", 160],
    ];
    const sections = [];
    for (const [suffix, label, height] of cases) {
      const images = await Promise.all(["before", "after"].map(async (phase) => {
        const bytes = await readFile(`${output}/${phase}-${width}px-${theme}-${suffix}.png`);
        return `<div><b>${phase}</b><div class="capture" style="height:${height}px"><img width="${width}" src="data:image/png;base64,${bytes.toString("base64")}" alt="${phase} ${label}"></div></div>`;
      }));
      sections.push(`<section><h2>${label}</h2><div class="pair">${images.join("")}</div></section>`);
    }
    const html = `<!doctype html><html><head><meta charset="utf-8"><title>Navigation #542 · ${width}px · ${theme}</title><style>body{margin:16px;font:16px Arial,sans-serif;background:#eef0f2;color:#17232b}h1{font-size:24px}h2{font-size:18px;margin:16px 0 6px}.pair{display:grid;grid-template-columns:repeat(2,${width}px);gap:16px}.capture{overflow:hidden;margin-top:4px}img{display:block}b{text-transform:capitalize}</style></head><body><h1>Navigation #542 · ${width} CSS px · ${theme}</h1><p>Before / after: original screenshot pixels. Complete viewport captures attached separately.</p>${sections.join("")}</body></html>`;
    await writeFile(`${output}/comparison-${width}px-${theme}.html`, html);
    await session.withPage(async ({ page }) => {
      await page.setContent(html);
      await page.screenshot({ animations: "disabled", caret: "initial", path: `${output}/comparison-${width}px-${theme}.png`, fullPage: true });
    }, { width: width * 2 + 48, height: 800 });
  }
}
