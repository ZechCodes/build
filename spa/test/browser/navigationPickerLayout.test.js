import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession, loadBrowserModules } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountNavigationPicker, tickNavigationPicker } from "./navigationPickerSeed.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });
const grouped = '.tbmenu-list a[data-destination]:not([data-visit])';

it.each(["global", "local project return"])("P1 production_router_adds_one_entry_and_restores_exact_route_on_Back_and_Forward %s", async (scope) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath, { observeHash: false });
    await page.evaluate(() => { window.__realRouterBaseModules = window.__layoutModules; });
    // The query bypasses the fixture's app.js resolver. Actual go/initRouter
    // execute; gating prevents mounting a live surface or creating a session.
    await loadBrowserModules(page, { realApp: "src/app.js?navigation-real-router" }, basePath);
    await page.evaluate(() => {
      Object.assign(window.__layoutModules, window.__realRouterBaseModules);
      const { realApp, app } = window.__layoutModules;
      app.App.gated = true;
      realApp.initRouter();
      window.__navigationHarness.navigate = function(next, options) { this.navigations.push(next); return realApp.go(next, options); };
    });
    const before = await page.evaluate(() => ({ hash: location.hash, entries: history.length }));
    await page.locator(`[data-select="${scope === "global" ? "project" : "workspace"}"]`).click();
    const destination = page.locator(scope === "global" ? `${grouped}[data-workspace="away-device/release"]` : "[data-project-page]");
    if (scope !== "global") await page.waitForFunction(() => document.querySelector("[data-project-page]")?.getAttribute("href")?.includes("README.md"));
    const hash = await destination.getAttribute("href");
    await destination.click();
    await page.waitForFunction((hash) => location.hash === hash, hash);
    expect(await page.evaluate(() => history.length)).toBe(before.entries + 1);
    await page.goBack();
    await page.waitForFunction(() => window.__layoutModules.app.App.route.workspaceId === "current");
    expect(await page.evaluate(() => location.hash)).toBe(before.hash);
    await page.goForward();
    await page.waitForFunction((hash) => location.hash === hash, hash);
    expect(await page.evaluate(() => window.__navigationHarness.navigations.length)).toBe(1);
  }, { width: 390, height: 844, hasTouch: true });
}, 40_000);

for (const width of [1280, 390]) for (const theme of ["light", "dark"]) {
  it(`P1 across_projects_two_activations ${width} ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationPicker(page, basePath, { theme });
      const history = await page.evaluate(() => window.history.length);
      await page.locator('[data-select="project"]').click();
      expect(await page.locator(".tb-scope").textContent()).toContain("All projects · All devices");
      await page.locator(`${grouped}[data-workspace="away-device/release"]`).click();
      await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
      expect(await page.evaluate(() => window.__navigationHarness.navigations[0])).toMatchObject({ name: "workspace", deviceId: "away-device", projectId: "docs", workspaceId: "release", tab: "changes" });
      expect(await page.evaluate(() => window.history.length)).toBe(history + 1);
      expect(await page.locator(".tbmenu").count()).toBe(0);
      await page.goBack();
      await page.waitForFunction(() => window.__layoutModules.app.App.route.workspaceId === "current");
      expect(await page.evaluate(() => window.__layoutModules.app.App.route)).toMatchObject({ file: "README.md", line: 12 });
      await page.goForward();
      await page.waitForFunction(() => window.__layoutModules.app.App.route.workspaceId === "release");
    }, { width, height: width === 1280 ? 800 : 844, hasTouch: width < 900 });
  }, 40_000);
}
it("P2 colliding_devices_and_names", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath);
    await page.locator('[data-select="project"]').click();
    await page.locator(".tb-filter").fill("Build navigation-plan Laptop");
    expect(await page.locator(grouped).count()).toBe(1);
    expect(await page.locator(grouped).getAttribute("data-workspace")).toBe("away-device/current");
    expect(await page.locator('[data-project-group="away-device/build"]').textContent()).toContain("Laptop");
    await page.locator(".tb-filter").fill("Café 文");
    expect(await page.locator(grouped).getAttribute("data-workspace")).toBe("away-device/release");
    expect(await page.locator("[data-visit]").count()).toBe(0);
    await page.locator(".tb-filter").fill("no such xyz123");
    expect(await page.locator(".tbmenu-list").textContent()).toContain("No matching projects or workspaces");
  });
}, 30_000);
it("P3 cache_tick_preserves_input", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath);
    await page.locator('[data-select="project"]').click();
    await page.locator(".tb-filter").fill("Docs");
    const before = await page.evaluate(() => {
      const field = document.querySelector(".tb-filter"); field.setSelectionRange(1, 3);
      const list = document.querySelector(".tbmenu-list"); list.scrollTop = 70;
      window.__pickerField = field;
      return { order: [...list.children].map((row) => row.dataset.key), scroll: list.scrollTop };
    });
    await tickNavigationPicker(page);
    await page.waitForFunction(() => document.querySelector('[data-workspace="away-device/new"]'));
    const after = await page.evaluate(() => ({ fieldSame: document.querySelector(".tb-filter") === window.__pickerField,
      focused: document.activeElement === window.__pickerField, value: window.__pickerField.value,
      selection: [window.__pickerField.selectionStart, window.__pickerField.selectionEnd],
      scroll: document.querySelector(".tbmenu-list").scrollTop,
      order: [...document.querySelector(".tbmenu-list").children].map((row) => row.dataset.key) }));
    expect(after).toMatchObject({ fieldSame: true, focused: true, value: "Docs", selection: [1, 3], scroll: before.scroll });
    expect(after.order.filter((key) => before.order.includes(key))).toEqual(before.order);
    await page.evaluate(async () => window.__layoutModules.ui.writeUiRecord({ deviceId: "", entityId: "", kind: "ui-menu", sub: "toolbar:jump" }, { open: false }));
    expect(await page.locator(".tbmenu").count()).toBe(1);
    await page.locator('[data-workspace="away-device/release"]').focus();
    await tickNavigationPicker(page, { remove: true });
    await page.waitForFunction(() => !document.querySelector('[data-workspace="away-device/release"]'));
    expect(await page.evaluate(() => document.activeElement.matches("a[data-destination]"))).toBe(true);
    expect(await page.evaluate(() => window.__navigationHarness.navigations)).toEqual([]);
  }, { width: 390, height: 844, hasTouch: true });
}, 40_000);
for (const method of ["touch", "mouse", "keyboard"]) for (const width of [1280, 390, 320]) {
  it(`P4 both_switchers_focus_search_on_touch_mouse_keyboard ${method} ${width}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationPicker(page, basePath);
      await page.evaluate(() => document.querySelectorAll("[data-select]").forEach((control) => control.addEventListener("click", () => {
        window.__pickerGestureFocus = document.activeElement.matches(".tb-filter");
      })));
      for (const selector of ['[data-select="project"]', '[data-select="workspace"]']) {
        const trigger = page.locator(selector);
        if (method === "touch") await trigger.tap();
        else if (method === "mouse") await trigger.click();
        else { await trigger.focus(); await page.keyboard.press("Enter"); }
        expect(await page.evaluate(() => window.__pickerGestureFocus)).toBe(true);
        await page.keyboard.press("Tab");
        expect(await page.evaluate(() => document.querySelector(".tbmenu").contains(document.activeElement))).toBe(true);
        await page.keyboard.press("Escape");
        expect(await trigger.evaluate((button) => document.activeElement === button)).toBe(true);
      }
      await page.keyboard.press("Control+k");
      expect(await page.locator(".tb-filter").evaluate((field) => document.activeElement === field)).toBe(true);
      await page.locator(".tb-filter").fill("Café 文");
      await page.keyboard.press("ArrowDown");
      expect(await page.evaluate(() => document.activeElement.dataset.workspace)).toBe("away-device/release");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__navigationHarness.navigations.length === 1);
      expect(await page.evaluate(() => document.activeElement.closest("#root") !== null)).toBe(true);
    }, { width, height: width === 1280 ? 800 : width === 320 ? 740 : 844, hasTouch: true });
  }, 40_000);
}
it("P4 shortcut leaves editable terminal composition and other modal keys alone; modified links remain native", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath);
    const stolen = await page.evaluate(() => {
      const probes = ['<input>', '<textarea>', '<div contenteditable="true" tabindex="0"></div>', '<div class="xterm" tabindex="0"></div>', '<div class="cm-editor" tabindex="0"></div>', '<div class="term-screen" tabindex="0"></div>', '<div class="console-pane" tabindex="0"></div>'];
      return probes.map((html) => {
        const host = document.createElement("div"); host.innerHTML = html; document.body.append(host); const node = host.firstChild; node.focus();
        const event = new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true }); node.dispatchEvent(event); host.remove(); return event.defaultPrevented;
      });
    });
    expect(stolen).toEqual([false, false, false, false, false, false, false]);
    expect(await page.locator(".tbmenu").count()).toBe(0);
    await page.evaluate(() => {
      const modal = document.createElement("div"); modal.setAttribute("role", "dialog"); document.body.append(modal);
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true })); modal.remove();
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, isComposing: true, bubbles: true }));
    });
    expect(await page.locator(".tbmenu").count()).toBe(0);
    await page.locator('[data-select="project"]').click();
    const intercepted = await page.locator(`${grouped}[data-workspace="away-device/release"]`).evaluate((link) => {
      const event = new MouseEvent("click", { ctrlKey: true, button: 0, bubbles: true, cancelable: true }); link.dispatchEvent(event); return event.defaultPrevented;
    });
    expect(intercepted).toBe(false);
    expect(await page.locator(".tbmenu").count()).toBe(1);
  });
}, 30_000);
for (const width of [390, 320]) for (const theme of ["light", "dark"]) {
  it(`P4 keyboard_reduced_visual_viewport ${width} ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationPicker(page, basePath, { theme });
      await page.locator('[data-select="project"]').tap();
      await page.evaluate(() => {
        Object.defineProperty(visualViewport, "height", { configurable: true, value: 300 });
        Object.defineProperty(visualViewport, "offsetTop", { configurable: true, value: 40 });
        visualViewport.dispatchEvent(new Event("resize"));
      });
      for (const selector of [".tb-filter", "[data-menu-close]", ".tbmenu-list"]) {
        const box = await page.locator(selector).boundingBox();
        expect(box.y).toBeGreaterThanOrEqual(40); expect(box.y + box.height).toBeLessThanOrEqual(340);
        expect(box.height).toBeGreaterThanOrEqual(selector === ".tb-filter" ? 28 : 32);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    }, { width, height: 844, hasTouch: true });
  }, 30_000);
}
it("P5 visit_history_not_quiet_recent survives toolbar remount and excludes face/cache/Current changes", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath);
    const read = () => page.evaluate(async () => (await window.__layoutModules.ui.readUiRecord(window.__layoutModules.visits.navigationVisitAddress)).value);
    const before = await read();
    await page.evaluate(() => { const { app, toolbar } = window.__layoutModules; app.App.route = { ...app.App.route, tab: "tasks", taskId: "t" }; toolbar.toolbarRouteChanged(); });
    await tickNavigationPicker(page);
    expect(await read()).toEqual(before);
    await page.locator('[data-select="project"]').click();
    const shortcut = page.locator('[data-visit][data-workspace="nav-device/current"]');
    await shortcut.click();
    expect(await page.evaluate(() => window.__navigationHarness.navigations)).toEqual([]);
    expect(await read()).toEqual(before);
    await page.evaluate(async () => { const { toolbar } = window.__layoutModules; await toolbar.stopToolbar(); await toolbar.initToolbar(); });
    expect(await read()).toEqual(before);
    await page.evaluate(async () => { const { toolbar, visits, ui } = window.__layoutModules; await toolbar.stopToolbar(); visits.resetNavigationVisits(); await ui.wipeUiRecords(); });
    expect(await page.evaluate(async () => window.__layoutModules.ui.readUiRecord(window.__layoutModules.visits.navigationVisitAddress))).toBeUndefined();
  });
}, 40_000);

it("P3 watched_task_badges_update_without_reordering_or_double_counting", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath);
    await page.locator('[data-select="project"]').click();
    const heading = page.locator('[data-project-group="away-device/docs"]');
    const project = page.locator(`${grouped}[data-project="away-device/docs"]`);
    const before = await page.locator(grouped).evaluateAll((rows) => rows.map((row) => row.dataset.destination));
    await page.evaluate(async () => {
      const { tracker } = window.__layoutModules;
      await tracker.writeTasksRecord("away-device", "docs", tracker.tasksRecord([{ id: "attention", project_id: "docs", watched: true, status: "ready", unread_count: 3, updated_at: "2026-10-12T12:00:00Z" }]));
    });
    await page.waitForFunction(() => document.querySelector('[data-project-group="away-device/docs"] .badge')?.textContent === "3");
    expect(await heading.locator(".badge").textContent()).toBe("3");
    expect(await project.locator(".badge").textContent()).toBe("3");
    expect(await page.locator(grouped).evaluateAll((rows) => rows.map((row) => row.dataset.destination))).toEqual(before);
    expect(await page.locator(".tb-filter").evaluate((field) => document.activeElement === field)).toBe(true);
  });
}, 30_000);

it("P4 guarded_navigation_focuses_persistent_destination_main_after_acceptance", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath);
    await page.evaluate(() => { window.__layoutModules.app.App.routeLeaveGuard = () => new Promise((resolve) => { window.__allowNavigation = resolve; }); });
    await page.locator('[data-select="project"]').click();
    await page.locator(`${grouped}[data-workspace="away-device/release"]`).click();
    expect(await page.evaluate(() => document.activeElement.id === "root")).toBe(false);
    expect(await page.evaluate(() => window.__navigationHarness.navigations.length)).toBe(0);
    await page.evaluate(() => window.__allowNavigation(false));
    expect(await page.evaluate(() => document.activeElement.id === "root")).toBe(false);
    await page.locator('[data-select="project"]').click();
    await page.locator(`${grouped}[data-workspace="away-device/release"]`).click();
    await page.evaluate(() => window.__allowNavigation(true));
    await page.waitForFunction(() => document.activeElement.id === "root");
    await page.evaluate(() => { document.querySelector("#root").innerHTML = '<h1>Late cached destination heading</h1>'; });
    expect(await page.evaluate(() => document.activeElement.id)).toBe("root");
  });
}, 30_000);

it("P4 opening_switcher_dismisses_phone_inbox_overlay", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationPicker(page, basePath, { inboxPinned: true });
    expect(await page.evaluate(() => document.body.classList.contains("inbox-collapsed"))).toBe(false);
    await page.keyboard.press("Control+k");
    expect(await page.evaluate(() => document.body.classList.contains("inbox-collapsed") && !document.body.classList.contains("inbox-popover-open"))).toBe(true);
    expect(await page.locator(".tb-filter").evaluate((field) => document.activeElement === field)).toBe(true);
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);
