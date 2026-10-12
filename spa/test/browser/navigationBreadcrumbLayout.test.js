import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim, mountNavigationBreadcrumbs } from "./navigationBreadcrumbSeed.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });
it("B1 project_crumb_goes_up", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath);
    expect(await page.locator('[data-project-return]').count()).toBe(1);
    expect(await page.locator('[data-project-return]').getAttribute("href")).toContain("/project/build/files");
    await page.locator('[data-project-return]').click();
    await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
    expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ name: "project", projectId: "build", tab: "files", file: "README.md" });
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);
it("B2 siblings_two_activations_and_search_focus", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath);
    for (const selector of ['[data-select="project"]', '[data-select="workspace"]']) {
      await page.locator(selector).tap();
      expect(await page.evaluate(() => window.__navigationHarness.focusInGesture)).toBe(true);
      expect(await page.locator(".tb-filter").evaluate((field) => document.activeElement === field)).toBe(true);
      await page.keyboard.press("Escape");
      await page.waitForSelector(".tbmenu", { state: "detached" });
      expect(await page.locator(selector).evaluate((button) => document.activeElement === button)).toBe(true);
    }
    await page.locator('[data-select="workspace"]').tap();
    await page.locator(".tb-filter").fill("Sibling");
    await page.locator('[data-workspace="nav-device/sibling"]').tap();
    await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
    expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ name: "workspace", workspaceId: "sibling" });
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);
it("B3 task_route_host", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath, { kind: "workspaceTask" });
    expect(await page.locator(".tb-breadcrumbs ol li").count()).toBe(4);
    expect(await page.locator(".tb-leaf").textContent()).toContain("542");
    const exact = await page.evaluate(() => location.hash);
    await page.locator(".tb-leaf").click();
    expect(await page.evaluate(() => location.hash)).toBe(exact);
    expect(await page.evaluate(() => window.__navigationHarness.navigations)).toEqual([]);
    await page.locator("[data-tasks-parent]").click();
    await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
    expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ name: "workspace", workspaceId: "current", tab: "tasks", view: "board" });
  }, { width: 320, height: 740, hasTouch: true });
}, 30_000);
for (const width of [1280, 390, 320]) for (const theme of ["light", "dark"]) {
  it(`B4 intrinsic_project_width_and_long_names ${width}px ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      for (const longNames of [false, true]) {
        await mountNavigationBreadcrumbs(page, basePath, { longNames, theme });
        const dimensions = await page.evaluate(() => {
          const rect = (selector) => { const element = document.querySelector(selector); if (!element) return null; const { x, y, width, height, right, bottom } = element.getBoundingClientRect(); return { x, y, width, height, right, bottom }; };
          const project = document.querySelector("[data-project-return]");
          const range = document.createRange(); range.selectNodeContents(project);
          return { toolbar: rect("#toolbar"), project: rect("[data-project-return]"), textWidth: range.getBoundingClientRect().width, arrow: rect('[data-select="project"]'), workspace: rect('[data-select="workspace"]'), caret: rect(".tb-workspace .tb-caret"), lock: rect("[data-workspace-lock]"), viewport: innerWidth, scrollWidth: document.documentElement.scrollWidth, padding: getComputedStyle(project).paddingLeft };
        });
        expect(dimensions.toolbar.height, JSON.stringify(dimensions)).toBe(44);
        expect(dimensions.project.height).toBe(44);
        expect(dimensions.padding).toBe("0px");
        if (!longNames) expect(Math.abs(dimensions.project.width - dimensions.textWidth)).toBeLessThanOrEqual(1);
        expect(dimensions.arrow.height).toBe(44); expect(dimensions.workspace.height).toBe(44); expect(dimensions.lock.height).toBe(44);
        expect(dimensions.caret.width).toBeGreaterThan(0);
        expect(dimensions.arrow.right).toBeLessThanOrEqual(dimensions.project.x);
        expect(dimensions.workspace.right).toBeLessThanOrEqual(dimensions.lock.x);
        expect(dimensions.lock.right).toBeLessThanOrEqual(dimensions.toolbar.right);
        expect(dimensions.scrollWidth).toBeLessThanOrEqual(width);
      }
    }, { width, height: width === 1280 ? 800 : 844, hasTouch: width < 900 });
  }, 45_000);
}
