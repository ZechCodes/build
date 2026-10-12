import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim, mountNavigationBreadcrumbs } from "./navigationBreadcrumbSeed.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });
async function activate(page, selector, method) {
  const control = page.locator(selector);
  if (method === "touch") await control.tap();
  else if (method === "mouse") await control.click();
  else { await control.focus(); await page.keyboard.press("Enter"); }
}
it("B1 project_crumb_goes_up", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath);
    expect(await page.locator('[data-project-return]').count()).toBe(1);
    await page.waitForFunction(() => document.querySelector("[data-project-return]")?.getAttribute("href")?.includes("README.md"));
    const target = await page.locator("[data-project-return]").evaluate((link) => window.__layoutModules.router.routeFromHash(link.getAttribute("href")));
    expect(target).toMatchObject({ name: "project", projectId: "build", tab: "files", file: "README.md" });
    await page.locator('[data-project-return]').click();
    await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
    expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ name: "project", projectId: "build", tab: "files", file: "README.md" });
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);
it.each(["touch", "mouse", "keyboard"])("B2 siblings_two_activations_and_search_focus %s", async (method) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath);
    for (const selector of ['[data-select="project"]', '[data-select="workspace"]']) {
      await activate(page, selector, method);
      expect(await page.evaluate(() => window.__navigationHarness.focusInGesture)).toBe(true);
      expect(await page.locator(".tb-filter").evaluate((field) => document.activeElement === field)).toBe(true);
      await page.keyboard.press("Escape");
      await page.waitForSelector(".tbmenu", { state: "detached" });
      expect(await page.locator(selector).evaluate((button) => document.activeElement === button)).toBe(true);
    }
    await activate(page, '[data-select="workspace"]', method);
    await page.locator(".tb-filter").fill("Sibling");
    await page.locator('[data-workspace="nav-device/sibling"]').click();
    await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
    expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ name: "workspace", workspaceId: "sibling" });
  }, { width: method === "touch" ? 390 : 1280, height: 844, hasTouch: method === "touch" });
}, 30_000);
it("B3 task_route_host", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath, { kind: "workspaceTask" });
    expect(await page.locator(".tb-breadcrumbs ol li").count()).toBe(4);
    await page.waitForFunction(() => document.querySelector(".tb-leaf")?.textContent.includes("542"));
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

it.each(["projectTask", "branch"])("B3 task_route_host preserves %s ancestry and current leaf", async (kind) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath, { kind });
    const leaf = page.locator(".tb-leaf");
    await leaf.waitFor();
    const current = await page.evaluate(() => location.hash);
    expect(await leaf.getAttribute("href")).toBe(current);
    expect(await leaf.getAttribute("aria-current")).toBe("page");
    await leaf.click();
    expect(await page.evaluate(() => window.__navigationHarness.navigations)).toEqual([]);
    if (kind === "projectTask") {
      expect(await page.locator(".tb-breadcrumbs ol li").count()).toBe(3);
      await page.locator("[data-tasks-parent]").click();
      await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
      expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ name: "project", projectId: "build", tab: "tasks" });
    } else expect(await leaf.textContent()).toBe("build/navigation-evidence");
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);

it.each(["workspace", "workspaceTask"])("B4 overlay offsets follow measured %s toolbar height", async (kind) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath, { kind });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const agent = await page.evaluate(() => ({ toolbarBottom: document.querySelector("#toolbar").getBoundingClientRect().bottom, overlayTop: document.querySelector(".rail-panel").getBoundingClientRect().top }));
    expect(Math.abs(agent.overlayTop - agent.toolbarBottom), JSON.stringify(agent)).toBeLessThanOrEqual(1);
    await page.evaluate(() => {
      const consoleRegion = document.querySelector("#console-region");
      consoleRegion.innerHTML = '<div class="console">Offline console geometry probe</div>';
      consoleRegion.dataset.size = "full";
    });
    const consoleBounds = await page.evaluate(() => ({ toolbarBottom: document.querySelector("#toolbar").getBoundingClientRect().bottom, overlayTop: document.querySelector("#console-region").getBoundingClientRect().top }));
    expect(Math.abs(consoleBounds.overlayTop - consoleBounds.toolbarBottom), JSON.stringify(consoleBounds)).toBeLessThanOrEqual(1);
  }, { width: 320, height: 740, hasTouch: true });
}, 30_000);

it("B4 duplicate project names expose their cached device qualifier", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath, { duplicateNames: true });
    const project = page.locator("[data-project-return]");
    expect(await project.getAttribute("aria-label")).toContain("Workshop");
    expect(await project.textContent()).toContain("Workshop");
    await page.locator('[data-select="project"]').tap();
    expect(await page.locator('[data-project="second-device/build"]').textContent()).toContain("Laptop");
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);

it("B2 search and Close stay reachable after the touch keyboard reduces the viewport", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath);
    await page.locator('[data-select="workspace"]').tap();
    await page.setViewportSize({ width: 390, height: 360 });
    await page.locator(".tb-filter").fill("Sibling");
    const bounds = await page.evaluate(() => {
      const box = (selector) => { const { top, bottom, height } = document.querySelector(selector).getBoundingClientRect(); return { top, bottom, height }; };
      return { search: box(".tb-filter"), close: box("[data-menu-close]"), menu: box(".tbmenu"), list: box(".tbmenu-list"), viewport: innerHeight };
    });
    expect(bounds.search.top).toBeGreaterThanOrEqual(0);
    expect(bounds.close.bottom).toBeLessThanOrEqual(bounds.viewport);
    expect(bounds.menu.bottom).toBeLessThanOrEqual(bounds.viewport);
    expect(bounds.list.height).toBeGreaterThan(30);
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
    expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ workspaceId: "sibling" });
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);

it("B3 branch verb remains reachable beside the current branch breadcrumb", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath, { kind: "branch" });
    await page.evaluate(() => {
      window.__branchVerb = 0;
      window.__layoutModules.toolbar.setToolbarVerb((host) => {
        host.innerHTML = '<button class="btn" id="branch-verb">Done</button>';
        host.querySelector("button").onclick = () => { window.__branchVerb++; };
      });
    });
    await page.locator("#branch-verb").tap();
    expect(await page.evaluate(() => window.__branchVerb)).toBe(1);
    expect(await page.locator(".tb-leaf").getAttribute("aria-current")).toBe("page");
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320);
  }, { width: 320, height: 740, hasTouch: true });
}, 30_000);
