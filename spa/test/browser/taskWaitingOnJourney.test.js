import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountWaitingJourney } from "./taskWaitingOnJourneyHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const source of ["task-page", "list", "board"]) {
  it.each(["done", "close"])(`${source} waiting assignment releases once after %s and updates Active/Backlog`, async (resolution) => {
    await session.withPage(async ({ page, basePath }) => {
      await mountWaitingJourney(page, basePath);
      if (source === "task-page") await page.locator('#task [data-task-assign]').click();
      else {
        await page.locator(`#journey-dashboard [data-task-view="${source}"]`).click();
        await page.locator('#journey-dashboard [data-task="task-waiting"] [data-task-assign]').click();
      }
      await page.locator('#task-assign-assignee').selectOption("new_workspace");
      await page.locator('#task-assign-note').fill("Assess closures before starting");
      await page.locator('[data-assign-go]').click();
      await page.locator('#task-assign-scrim').waitFor({ state: "detached" });
      expect(await page.evaluate(() => ({ provisions: window.__waiting.provisions, deliveries: window.__waiting.deliveries })))
        .toEqual({ provisions: [], deliveries: [] });
      await page.locator('#journey-dashboard [data-task-view="dashboard"]').click();
      await page.locator('#journey-dashboard [data-dashboard-tab="active"]').click();
      await page.locator('#journey-dashboard [data-active-group="blocked"] [data-task="task-waiting"]').waitFor();
      await page.evaluate(async (resolution) => {
        const fixture = window.__waiting;
        await fixture.context.rpc(resolution === "close" ? "tasks.close" : "tasks.update",
          { task_id: "prerequisite", ...(resolution === "done" ? { status: "done" } : {}) });
      }, resolution);
      await page.locator('#journey-dashboard [data-active-group="working"] [data-task="task-waiting"]').waitFor();
      const retained = page.locator('#task [data-waiting-list] a[href*="/prerequisite"]').locator('..');
      expect(await retained.textContent()).toMatch(resolution === "close" ? /Closed/ : /Done/);
      const delivery = await page.evaluate(() => window.__waiting.deliveries);
      expect(delivery).toHaveLength(1);
      expect(delivery[0].closed_without_done_task_ids).toEqual(resolution === "close" ? ["prerequisite", "closed-prerequisite"] : ["closed-prerequisite"]);
      expect(delivery[0].body).toMatch(/Assess.*closed without Done.*escalate/i);
      await page.locator('#journey-dashboard [data-dashboard-tab="backlog"]').click();
      await page.locator('#journey-dashboard [data-backlog-group="ready"] [data-task="unassigned"]').waitFor();
      await page.evaluate(async () => {
        const fixture = window.__waiting;
        await fixture.context.rpc("tasks.update", { task_id: "prerequisite", state: "open", status: "ready" });
      });
      await page.locator('#journey-dashboard [data-backlog-group="blocked"] [data-task="unassigned"]').waitFor();
      await page.locator('#journey-dashboard [data-dashboard-tab="active"]').click();
      await page.locator('#journey-dashboard [data-active-group="blocked"] [data-task="task-waiting"]').waitFor();
      await page.evaluate(async (resolution) => {
        const fixture = window.__waiting;
        await fixture.context.rpc(resolution === "close" ? "tasks.close" : "tasks.update",
          { task_id: "prerequisite", ...(resolution === "done" ? { status: "done" } : {}) });
      }, resolution);
      const evidence = await page.evaluate(() => {
        const fixture = window.__waiting;
        return { assignments: fixture.calls.filter(({ method }) => method === "tasks.assign"), provisions: fixture.provisions,
          deliveries: fixture.deliveries, notices: fixture.notices,
          unassigned: fixture.tasks.find((task) => task.id === "unassigned") };
      });
      expect(evidence.assignments).toHaveLength(1);
      expect(evidence.provisions).toHaveLength(1);
      expect(evidence.deliveries).toHaveLength(1);
      expect(evidence.notices.every((notice) => notice.mentions_user === false)).toBe(true);
      expect(evidence.unassigned).toMatchObject({ status: "backlog", labels: ["keep-label"], assignee: null });
      expect(evidence.unassigned.deferred_assignment).toBeUndefined();
    }, { width: source === "task-page" ? 390 : 1280, height: 1000 });
  }, 30_000);
}
