import { expect, it } from "vitest";
import { columns, task } from "../trackerWireFixture.js";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

for (const [label, width] of [["desktop", 1440], ["390px", 390]]) {
  it(`shows the work task review link from its cold cached detail at ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, '<main id="task-pane"></main>', {
        basePath, styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block} #task-pane{max-width:1100px;margin:20px auto;padding:0 16px;min-width:0}`,
      });
      await loadBrowserModules(page, { pane: "src/core/trackerTaskPage.js", cache: "src/core/trackerCache.js", support: "src/core/taskReviewSupport.js" }, basePath);
      await page.evaluate(async ({ work, columns }) => {
        const { pane, cache, support } = window.__layoutModules;
        await support.rememberReviewSupport("work-review-layout", { reviews: { forTask: true } });
        await cache.writeTasksRecord("work-review-layout", "p1", cache.tasksRecord([], columns));
        await cache.writeTaskRecord("work-review-layout", "p1", work.id, cache.taskRecord(work));
        window.__workTaskPage = pane.mountTaskPage(document.querySelector("#task-pane"), {
          deviceId: "work-review-layout", projectId: "p1", projectKey: "work-review-layout|p1", taskId: work.id,
          feed: () => ({ projects: [], items: [], workspaces: [] }), callRpc: () => new Promise(() => {}),
          catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
        });
      }, { columns: columns(), work: task({ id: "work-971", number: 971, title: "Make review requests easy to find",
        body: "Request a review from the workspace, then follow the linked review here.", status: "in_progress",
        linked_reviews: [{ task_id: "review-980", number: 980, for_task_id: "work-971", status: "changes_requested", version: 2 }],
      }) });
      await page.locator('[data-work-task-review]').waitFor();
      expect(await page.locator('[data-work-task-review]').textContent()).toBe("Review #980 · Changes requested");
      expect(await page.locator('[data-work-task-review]').getAttribute('href')).toBe("#/device/work-review-layout/project/p1/tasks/review-980");
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
      await captureLayout(page, `work-task-review-${label}.png`);
      await page.evaluate(() => window.__workTaskPage.dispose());
    }, { width, height: 844, plugins: [deviceShim] });
  }, 60_000);
}
