import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

async function mountEnvironments(page, basePath) {
  await mountNavigationBreadcrumbs(page, basePath, { duplicateNames: true });
  await page.evaluate(async () => {
    const { cache } = window.__layoutModules;
    const address = kind => ({ deviceId: "nav-device", entityId: "", kind });
    const held = (await cache.readCached(address("projects"))).value;
    const projects = held.map(project => ({ ...project, environment_id: "shared", last_activity_ms: Date.now() }));
    await cache.writeCached(address("projects"), projects);
    await cache.writeCached(address("environments"), [{ environment_id: "shared", name: "Shared environment with a very long name", project_ids: ["build", "docs"] }]);
    await cache.writeCached(address("environment-support"), { list: true, createProject: true });
  });
  await page.locator(".tb-environment").waitFor({ state: "attached" });
}

it.each([1280, 761, 760, 390, 320])("environment crumb and picker fit at %ipx without changing control geometry", async width => {
  await session.withPage(async ({ page, basePath }) => {
    await mountEnvironments(page, basePath);
    const measured = await page.evaluate(() => {
      const rect = selector => document.querySelector(selector).getBoundingClientRect();
      const env = document.querySelector(".tb-environment");
      const glyph = rect('[data-project-return] [data-kind="project"]');
      return { envVisible: getComputedStyle(env).display !== "none", envOverflow: env.scrollWidth > env.clientWidth,
        glyph: [glyph.width, glyph.height], arrow: rect(".tb-switch").height,
        workspace: rect('[data-select="workspace"]').height, bar: rect("#toolbar").height, scrollWidth: document.documentElement.scrollWidth };
    });
    expect(measured.envVisible).toBe(width > 760);
    if (width > 760) expect(measured.envOverflow).toBe(true);
    expect(measured.glyph).toEqual([16, 16]);
    expect(measured.arrow).toBe(44);
    expect(measured.workspace).toBe(44);
    expect(measured.bar).toBe(44);
    expect(measured.scrollWidth).toBeLessThanOrEqual(width);
    await page.locator('[data-select="project"]').click();
    await expect.poll(() => page.locator('[data-environment-group="nav-device/shared"]').count()).toBe(1);
    expect(await page.locator('[data-environment-group="nav-device/shared"]').textContent()).toContain("Workshop");
    const accessible = await page.locator('[data-environment-group="nav-device/shared"] [data-kind]').getAttribute("aria-hidden");
    expect(accessible).toBe("true");
    await captureLayout(page, `environment-switcher-${width}.png`);
  }, { width, height: 844, hasTouch: width < 900 });
}, 45_000);
