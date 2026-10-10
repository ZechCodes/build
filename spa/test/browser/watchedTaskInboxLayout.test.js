import { expect, it } from "vitest";
import { captureLayout, withLayoutPage } from "./layoutHarness.mjs";
import { ASK_TASK, QUIET_TASK, WATCH_DEVICE, WATCH_PROJECT, adoptWatchedTaskSession,
  disposeWatchedTaskInbox, mountWatchedTaskInbox } from "./watchedTaskInboxSeed.mjs";

const rowFor = (page, id) => page.locator(`#inbox-list [data-key="tracker_task:${id}"]`);

for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 720, hasTouch: false },
  { label: "phone", width: 390, height: 760, hasTouch: true },
]) {
  it(`paints every unfinished watched task from cache in byAnchor order with a visible eye on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountWatchedTaskInbox(page, basePath);
      for (const view of ["projects", "inbox"]) {
        await page.evaluate((view) => window.__layoutModules.inbox.setInboxView(view), view);
        expect(await page.locator('#inbox-list [data-key^="tracker_task:"]').evaluateAll((rows) =>
          rows.map((row) => row.dataset.key)), `${view}: byAnchor order regardless of task attention`).toEqual([
          `tracker_task:${QUIET_TASK}`, `tracker_task:${ASK_TASK}`, "tracker_task:review-watched-task",
        ]);
      }
      expect(await page.evaluate(() => window.__layoutModules.contexts.liveContexts().length)).toBe(0);
      const quiet = rowFor(page, QUIET_TASK);
      expect(await quiet.getAttribute("class")).toContain("inbox-quiet");
      expect(await quiet.locator(".inbox-facts, .inbox-reason").count()).toBe(0);
      expect(await rowFor(page, ASK_TASK).textContent()).toContain("Assigned to you");
      const title = await quiet.locator(".stitle").boundingBox();
      for (const row of await page.locator('#inbox-list [data-key^="tracker_task:"]').all()) {
        const eye = row.locator("button.inbox-watch[data-unwatch]");
        expect(await eye.count()).toBe(1);
        expect(await eye.getAttribute("aria-pressed")).toBe("true");
        expect(await eye.getAttribute("aria-label")).toMatch(/^Stop watching #\d+ /);
        expect(await eye.locator("svg").count()).toBe(1);
        const layout = await eye.evaluate((button) => {
          const bounds = button.getBoundingClientRect();
          const styles = getComputedStyle(button);
          const actions = getComputedStyle(button.parentElement);
          return { x: bounds.x, right: bounds.right, width: bounds.width, height: bounds.height,
            opacity: styles.opacity, parentOpacity: actions.opacity, pointerEvents: styles.pointerEvents,
            hit: document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)?.closest("button") === button };
        });
        expect(layout.opacity).toBe("1");
        expect(layout.parentOpacity).toBe("1");
        expect(layout.pointerEvents).toBe("auto");
        expect(layout.hit).toBe(true);
        expect(layout.right).toBeLessThanOrEqual(width);
        expect(layout.width).toBeGreaterThanOrEqual(hasTouch ? 44 : 32);
        expect(layout.height).toBeGreaterThanOrEqual(hasTouch ? 44 : 32);
      }
      const quietEye = await quiet.locator("[data-unwatch]").boundingBox();
      expect(title.x + title.width).toBeLessThanOrEqual(quietEye.x);
      const quietBox = await quiet.boundingBox();
      const askedBox = await rowFor(page, ASK_TASK).boundingBox();
      expect(quietBox.height).toBeLessThan(askedBox.height);
      await captureLayout(page, `watched-task-inbox-${label}.png`);
      await disposeWatchedTaskInbox(page);
    }, { width, height, hasTouch });
  }, 30_000);

  it(`unwatches directly from the visible eye and restores a refused watch on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountWatchedTaskInbox(page, basePath, { unread: true });
      expect(await rowFor(page, QUIET_TASK).count()).toBe(1);
      await page.waitForFunction(() => document.querySelector(".inbox-open-count").textContent === "1");
      await adoptWatchedTaskSession(page);
      const eye = rowFor(page, QUIET_TASK).locator("[data-unwatch]");
      if (hasTouch) await eye.tap();
      else await eye.click();
      await page.waitForFunction(() => Boolean(window.__settleWatchedTaskUnwatch));
      await page.waitForFunction(() => document.querySelector(".inbox-open-count").textContent === "");
      expect(await rowFor(page, QUIET_TASK).count()).toBe(0);
      expect(await page.locator(".inbox-menu:not([hidden])").count()).toBe(0);
      expect(await page.evaluate(async ({ deviceId, projectId, taskId }) => {
        const record = await window.__layoutModules.tracker.readTasksRecord(deviceId, projectId);
        return record.tasks.find((task) => task.id === taskId).watched;
      }, { deviceId: WATCH_DEVICE, projectId: WATCH_PROJECT, taskId: QUIET_TASK })).toBe(false);
      expect(await page.evaluate(() => window.__watchedTaskCalls.filter((call) => call.method === "tasks.unwatch")))
        .toEqual([{ method: "tasks.unwatch", params: { task_id: QUIET_TASK } }]);
      await page.evaluate(() => window.__settleWatchedTaskUnwatch.reject(new Error("Watch update refused")));
      await rowFor(page, QUIET_TASK).waitFor({ state: "visible" });
      await page.waitForFunction(() => document.querySelector(".inbox-open-count").textContent === "1");
      expect(await rowFor(page, QUIET_TASK).locator('[data-unwatch][aria-pressed="true"]').count()).toBe(1);
      expect(await rowFor(page, QUIET_TASK).textContent()).toContain("Watch update refused");
      await disposeWatchedTaskInbox(page);
    }, { width, height, hasTouch });
  }, 30_000);
}
