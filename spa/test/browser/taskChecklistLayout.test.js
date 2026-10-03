import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

for (const theme of ["light", "dark"]) {
  it(`saves a task checkbox with Space and keeps keyboard focus on a phone, ${theme}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, `<link rel="stylesheet" href="${basePath}src/styles/tasks.css"><div id="pane"></div>`,
        { basePath, styles: "body { padding:16px; } .task-page { display:block; } .task-rail { display:none; }" });
      await loadBrowserModules(page, {
        support: "src/core/taskChecklistSupport.js", cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js",
      }, basePath);
      await page.evaluate(async (theme) => {
        document.documentElement.dataset.theme = theme;
        const { cache, taskPage, support } = window.__layoutModules;
        window.__checklistSaved = { task: {
          id: "task-347", project_id: "proj-1", number: 347, title: "Release checklist",
          body: "- [ ] Verify the release on desktop and phone\n  - [x] Read the changelog\n- [ ] Publish",
          state: "open", status: "in_progress", priority: "none", labels: [], assignee: null,
          links: {}, created_by: { kind: "user" }, updated_at: "2026-10-03T12:00:00Z",
        }, timeline: [] };
        window.__checklistUpdates = [];
        await support.rememberTaskChecklistSupport("dev-347", { tasks: { bodyPrecondition: true } });
        await cache.writeTaskRecord("dev-347", "proj-1", "task-347", window.__checklistSaved);
        window.__mountChecklist = () => taskPage.mountTaskPage(document.querySelector("#pane"), {
          deviceId: "dev-347", projectId: "proj-1", projectKey: "dev-347|proj-1", taskId: "task-347",
          catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }),
          callRpc: async (method, params) => {
            if (method === "tasks.get") return structuredClone(window.__checklistSaved);
            if (method !== "tasks.update") return {};
            window.__checklistUpdates.push(params);
            if (window.__immediateChecklist) {
              window.__checklistSaved.task.body = params.body;
              return { task: structuredClone(window.__checklistSaved.task) };
            }
            return new Promise((resolve) => {
              window.__finishChecklist = () => {
                window.__checklistSaved.task.body = params.body;
                resolve({ task: structuredClone(window.__checklistSaved.task) });
              };
            });
          },
        });
        window.__checklistPage = window.__mountChecklist();
      }, theme);
      const first = page.locator('.task-page-body input[data-task-index="0"]');
      await first.waitFor();
      await first.focus();
      await page.keyboard.press("Space");
      await page.waitForFunction(() => window.__checklistUpdates.length === 1);
      expect(await first.isChecked()).toBe(true);
      expect(await page.evaluate(async () => (await window.__layoutModules.cache.readTaskRecord("dev-347", "proj-1", "task-347")).task.body))
        .toBe("- [x] Verify the release on desktop and phone\n  - [x] Read the changelog\n- [ ] Publish");
      await page.evaluate(() => window.__finishChecklist());
      await page.waitForFunction(() => {
        const input = document.querySelector('.task-page-body input[data-task-index="0"]');
        return !input.disabled && document.activeElement === input;
      });
      const layout = await first.evaluate((input) => {
        const item = input.closest("li");
        const label = input.getBoundingClientRect();
        const bounds = input.closest('.task-page-body').getBoundingClientRect();
        return { marker: getComputedStyle(item).listStyleType, visible: label.width > 0 && label.height > 0,
          within: label.left >= bounds.left - 1 && label.right <= bounds.right + 1,
          overflow: document.documentElement.scrollWidth > innerWidth };
      });
      expect(layout).toEqual({ marker: "none", visible: true, within: true, overflow: false });
      await captureLayout(page, `task-checklist-${theme}.png`);
      await page.keyboard.press("Space");
      await page.waitForFunction(() => window.__checklistUpdates.length === 2);
      expect(await first.isChecked()).toBe(false);
      await page.evaluate(() => window.__finishChecklist());
      await page.waitForFunction(() => !document.querySelector('.task-page-body input[data-task-index="0"]').disabled);
      await page.evaluate(() => {
        window.__checklistPage.dispose();
        document.querySelector("#pane").replaceChildren();
        window.__checklistPage = window.__mountChecklist();
      });
      await first.waitFor();
      expect(await first.isChecked()).toBe(false);
      await page.evaluate(() => { window.__immediateChecklist = true; });
      await first.focus();
      await page.keyboard.press("Space");
      await page.waitForFunction(() => window.__checklistUpdates.length === 3 &&
        document.querySelector('.task-page-body input[data-task-index="0"]').checked);
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      expect(await first.evaluate((input) => input === document.activeElement)).toBe(true);
    }, { width: 390, height: 844, plugins: [deviceShim] });
  });
}

it("holds another tab's read until a pending checklist save settles", async () => {
  await withLayoutPage(async ({ page: initialPage, basePath }) => {
    const context = await initialPage.context().browser().newContext();
    await context.route(`${new URL(initialPage.url()).origin}/**`, async (route) => {
      const response = await route.fetch({ maxRetries: 0 });
      await route.fulfill({ response });
    });
    const page = await context.newPage();
    await page.goto(initialPage.url());
    const mount = async (tab, seed) => {
      await mountLayout(tab, '<div id="pane"></div>', { basePath });
      await loadBrowserModules(tab, { support: "src/core/taskChecklistSupport.js", cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js" }, basePath);
      await tab.evaluate(async (seed) => {
        const { cache, taskPage, support } = window.__layoutModules;
        window.__tabTask = { id: "task-tabs", number: 347, title: "Release", body: "- [ ] Verify\n- [ ] Publish",
          state: "open", status: "in_progress", labels: [], links: {}, assignee: null, priority: "none" };
        window.__tabReads = 0;
        await support.rememberTaskChecklistSupport("device", { tasks: { bodyPrecondition: true } });
        if (seed) await cache.writeTaskRecord("device", "project", "task-tabs", { task: window.__tabTask, timeline: [] });
        window.__tabPage = taskPage.mountTaskPage(document.querySelector("#pane"), {
          deviceId: "device", projectId: "project", projectKey: "device|project", taskId: "task-tabs",
          catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }),
          callRpc: async (method, params) => {
            if (method === "tasks.get") { window.__tabReads += 1; return { task: structuredClone(window.__tabTask), timeline: [] }; }
            if (method !== "tasks.update") return {};
            return new Promise((resolve) => {
              window.__finishTab = () => { window.__tabTask.body = params.body; resolve({ task: structuredClone(window.__tabTask) }); };
            });
          },
        });
      }, seed);
    };
    await mount(page, true);
    await page.waitForFunction(() => window.__tabReads === 1);
    await page.locator('.task-page-body input[data-task-index="0"]').click();
    await page.waitForFunction(() => window.__finishTab);
    const other = await context.newPage();
    await other.goto(page.url());
    await mount(other, false);
    const secondBox = other.locator('.task-page-body input[data-task-index="0"]');
    await secondBox.waitFor();
    expect(await secondBox.isChecked()).toBe(true);
    expect(await other.evaluate(() => window.__tabReads)).toBe(0);
    await other.evaluate(() => { window.__tabTask.body = "- [x] Verify\n- [ ] Publish"; });
    await page.evaluate(() => window.__finishTab());
    await other.waitForFunction(() => window.__tabReads === 1);
    expect(await secondBox.isChecked()).toBe(true);
    expect(await other.evaluate(async () => (await window.__layoutModules.cache.readTaskRecord("device", "project", "task-tabs")).task.body))
      .toBe("- [x] Verify\n- [ ] Publish");
  }, { plugins: [deviceShim] });
});
