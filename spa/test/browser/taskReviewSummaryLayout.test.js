import { expect, it } from "vitest";
import { columns, task } from "../trackerWireFixture.js";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const saved = [
  ["open", "in_review", "Review upload recovery"],
  ["approved", "in_review", "Review cache observation fences"],
  ["changes_requested", "in_progress", "Review the file picker"],
  ["merged", "done", "Review task reply links"],
  ["closed", "done", "Review the board filters"],
].map(([status, column, title], index) => task({
  id: `task-layout-${index}`, number: 408 + index, title, status: column, assignee: { kind: "user" },
  review_summary: { task_id: `task-layout-${index}`, workspace_id: "ws-layout", version: 3, status },
}));

async function mountCachedBoard(page, basePath) {
  await mountLayout(page, '<main id="tasks"></main>', {
    basePath, styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block} #tasks{margin:16px;min-width:0;max-width:none;padding:0}`,
  });
  await page.evaluate(() => {
    document.documentElement.dataset.theme = "dark";
    document.head.prepend(Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" }));
  });
  await loadBrowserModules(page, {
    pane: "src/core/trackerTasksPane.js", cache: "src/core/trackerCache.js",
  }, basePath);
  await page.evaluate(async ({ tasks, columns }) => {
    const { pane, cache } = window.__layoutModules;
    await cache.writeTasksRecord("pr-summary-layout", "p1", cache.tasksRecord(tasks, columns, 1));
    // Hold reads unanswered: the saved list must paint before any RPC result.
    const unansweredRead = new Promise(() => {});
    window.__taskBoard = pane.mountTasksPane(document.querySelector("#tasks"), {
      deviceId: "pr-summary-layout", projectId: "p1", projectKey: "pr-summary-layout|p1", projectName: "Build",
      defaultView: "board", feed: () => ({ projects: [], items: [], workspaces: [] }),
      callRpc: () => unansweredRead,
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
  }, { tasks: saved, columns: columns() });
  await page.locator('.task-card[data-task="task-layout-0"] .task-review-summary').waitFor();
}

for (const { label, width } of [{ label: "desktop", width: 1440 }, { label: "mobile", width: 390 }]) {
  it(`shows cached PR statuses across board columns on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountCachedBoard(page, basePath);
      expect((await page.locator(".task-review-summary").allTextContents()).map((text) => text.trim())).toEqual([
        "Changes requested", "Approved", "Open", "Closed", "Merged",
      ]);
      expect((await page.locator('[data-column="done"] .task-review-summary').allTextContents()).map((text) => text.trim())).toEqual(["Closed", "Merged"]);
      const size = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
      expect(size.page, JSON.stringify(size)).toBeLessThanOrEqual(size.viewport + 1);
      await page.locator('.task-card[data-task="task-layout-0"]').scrollIntoViewIfNeeded();
      await captureLayout(page, `task-pr-board-${label}.png`);

      await page.evaluate(async ({ tasks, columns }) => {
        const { cache } = window.__layoutModules;
        window.__keptTaskCard = document.querySelector('[data-task="task-layout-0"]');
        const next = tasks.map((task) => task.id === "task-layout-0"
          ? { ...task, review_summary: { ...task.review_summary, version: 4, status: "changes_requested" } } : task);
        await cache.writeTasksRecord("pr-summary-layout", "p1", cache.tasksRecord(next, columns, 2));
      }, { tasks: saved, columns: columns() });
      await page.waitForFunction(() => document.querySelector('[data-task="task-layout-0"] .task-review-summary')?.textContent.trim() === "Changes requested");
      expect(await page.evaluate(() => document.querySelector('[data-task="task-layout-0"]') === window.__keptTaskCard)).toBe(true);
      // Board organization can differ from PR status; drawing never invents a lifecycle change.
      expect(await page.locator('.task-card[data-task="task-layout-0"]').getAttribute("data-status")).toBe("in_review");
      await page.evaluate(() => window.__taskBoard.dispose());
    }, { width, height: 850, plugins: [deviceShim] });
  }, 60_000);
}
