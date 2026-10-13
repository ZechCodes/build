import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { BLOCK_DEVICE, BLOCK_PROJECT, HOSTILE_TITLE, mountBlockedInbox, mountBlockedTasks, pushBlockedTask } from "./taskBlockedSeed.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });
const groupIds = (page, selector) => page.locator(selector).evaluateAll((groups) => groups.map((group) => group.dataset.activeGroup || group.dataset.backlogGroup));
const taskIds = (page, selector) => page.locator(selector).evaluateAll((tasks) => tasks.map((task) => task.dataset.task));
for (const width of [390, 1280]) {
  it(`paints cached Active and Backlog before greeting or reads settle at ${width}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountBlockedTasks(page, basePath);
      await page.locator('[data-dashboard-tab="active"]').click();
      expect(await groupIds(page, "[data-active-group]")).toEqual(["working", "assigned", "blocked"]);
      expect(await taskIds(page, '[data-active-group="blocked"] [data-task]')).toEqual(["blocked-running", "blocked-user", "blocked-missing", "blocked-new-workspace"]);
      expect(await page.locator('[data-task="blocked-running"] .task-dashboard-link').textContent()).toContain(HOSTILE_TITLE);
      expect(await page.locator('[data-active-group="blocked"] a[href*="missing-blocker"]').count()).toBe(4);
      expect(await page.locator('img[src="x"], script:not([type="module"])').count()).toBe(0);
      expect(await page.evaluate(() => window.__injected || false)).toBe(false);
      expect(await page.evaluate(() => window.__blockedWireOpen)).toBe(false);
      await page.locator('[data-dashboard-tab="backlog"]').click();
      expect(await groupIds(page, ".task-dashboard [data-backlog-group]")).toEqual(["blocked", "ready"]);
      expect(await taskIds(page, '[data-backlog-group="blocked"] [data-task]')).toEqual(["backlog-blocked-high", "backlog-blocked-low"]);
      expect(await taskIds(page, '[data-backlog-group="ready"] [data-task]')).toEqual(["backlog-ready-high", "backlog-ready-low"]);
      await page.locator('[data-task-view="board"]').click();
      expect(await groupIds(page, '[data-column="backlog"] [data-backlog-group]')).toEqual(["blocked", "ready"]);
      expect(await page.locator('[data-column="backlog"] a[href*="missing-blocker"]').count()).toBe(6);
      expect(await page.locator('[data-column="done"]').count()).toBe(1);
      expect(await page.locator('#blocked-agent-rail .surface-task').allTextContents()).toEqual(expect.arrayContaining([
        expect.stringContaining("Blocked"), expect.stringContaining("Working"), expect.stringContaining("In review")]));
      expect(await page.locator('#blocked-agent-rail a[href*="missing-blocker"]').count()).toBe(1);
    }, { width, height: 1000 });
  }, 30_000);
}
it("normal pushed reads restore Blocked and keep history without column or label writes", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBlockedTasks(page, basePath);
    await page.locator('[data-dashboard-tab="active"]').click();
    await pushBlockedTask(page, "blocked-running", { dependencies: { blocked: false, waiting_on: [] } });
    await page.locator('[data-active-group="working"] [data-task="blocked-running"]').waitFor();
    await pushBlockedTask(page, "blocked-running", { dependencies: { blocked: true, waiting_on: [] } });
    await page.locator('[data-active-group="blocked"] [data-task="blocked-running"]').waitFor();
    await pushBlockedTask(page, "blocked-running", { status: "done" });
    await page.waitForFunction(() => !document.querySelector('[data-active-group="blocked"] [data-task="blocked-running"]'));
    await pushBlockedTask(page, "blocked-running", { status: "backlog", state: "closed" });
    expect(await page.locator('[data-active-group="blocked"] [data-task="blocked-running"]').count()).toBe(0);
    await pushBlockedTask(page, "blocked-running", { state: "open" });
    await page.locator('[data-active-group="blocked"] [data-task="blocked-running"]').waitFor();
    const calls = await page.evaluate(() => window.__blockedCalls);
    expect(calls.filter(({ method }) => /tasks\.(update|assign|columns\.set)/.test(method))).toEqual([]);
    expect(calls.filter(({ method }) => method === "tasks.list").length).toBeGreaterThan(1);
  });
}, 30_000);
it("legacy cache keeps flat Backlog and hides absent Blocked groups", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBlockedTasks(page, basePath, { legacy: true });
    await page.locator('[data-dashboard-tab="active"]').click();
    expect(await page.locator('[data-active-group="blocked"]').count()).toBe(0);
    await page.locator('[data-dashboard-tab="backlog"]').click();
    expect(await page.locator(".task-dashboard [data-backlog-group]").count()).toBe(0);
    await page.locator('[data-task-view="board"]').click();
    expect(await page.locator('[data-column="backlog"] [data-backlog-group]').count()).toBe(0);
  });
}, 30_000);
it("blocked-user attention retains questions and restores assignment on cached unblock", async () => {
  for (const question of [false, true]) await session.withPage(async ({ page, basePath }) => {
    await mountBlockedInbox(page, basePath, { question });
    const row = page.locator('[data-key="tracker_task:blocked-user"]');
    expect(await row.textContent()).toContain("Waiting on #558");
    expect(await row.locator('a[href*="missing-blocker"]').count()).toBe(1);
    expect(await row.textContent()).not.toContain("Assigned to you");
    if (question) expect(await row.textContent()).toMatch(/Needs you|asked|comment/i);
    await page.evaluate(async ({ deviceId, projectId }) => {
      const { tracker } = window.__layoutModules;
      const cached = await tracker.readTasksRecord(deviceId, projectId);
      const tasks = cached.tasks.map((task) => task.id === "blocked-user" ? { ...task, dependencies: { blocked: false, waiting_on: [] } } : task);
      await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord(tasks, []));
      await tracker.writeTaskRecord(deviceId, projectId, "blocked-user", tracker.taskRecord(tasks.find((task) => task.id === "blocked-user"), []));
    }, { deviceId: BLOCK_DEVICE, projectId: BLOCK_PROJECT });
    await page.waitForFunction(() => document.querySelector('[data-key="tracker_task:blocked-user"]')?.textContent.includes("Assigned to you"));
  }, { width: 390, height: 844 });
}, 30_000);
it("inbox blocker anchors receive click and keyboard navigation instead of opening the dependent row", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBlockedInbox(page, basePath);
    const blocker = page.locator('[data-key="tracker_task:blocked-user"] a[href*="missing-blocker"]');
    const target = await blocker.getAttribute("href");
    await page.evaluate(() => {
      window.__blockerClicks = [];
      document.addEventListener("click", (event) => {
        const anchor = event.target.closest("a");
        if (anchor) window.__blockerClicks.push({ href: anchor.getAttribute("href"), prevented: event.defaultPrevented });
        event.preventDefault();
      });
    });
    await blocker.click();
    expect(await page.evaluate(() => window.__blockerClicks)).toEqual([{ href: target, prevented: false }]);
    await blocker.focus(); await page.keyboard.press("Enter");
    expect(await page.evaluate(() => window.__blockerClicks)).toEqual([{ href: target, prevented: false }, { href: target, prevented: false }]);
  }, { width: 390, height: 844 });
}, 30_000);
