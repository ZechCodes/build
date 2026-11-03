import { expect, it } from "vitest";
import { columns, task } from "../trackerWireFixture.js";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountVerdictReviewTask } from "./taskReviewVerdictsFixture.mjs";

async function assertNoOverflow(page) {
  const size = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
  expect(size.page, JSON.stringify(size)).toBeLessThanOrEqual(size.viewport + 1);
}

async function prepareLoop(page) {
  await page.evaluate(async () => {
    const { reviews, support, local, reads } = window.__layoutModules;
    const scope = { deviceId: "review-layout", projectId: "p1", taskId: "task-1" };
    const address = reviews.reviewAddress(scope);
    const record = (await local.readCached(address)).value;
    window.__loopReview = structuredClone(record.review);
    window.__loopReview.version += 10;
    const latest = window.__loopReview.snapshots.at(-1);
    const previous = window.__loopReview.snapshots[0];
    const latestIdentity = { ...scope, snapshot: latest, directory: latest.directories[0] };
    const previousIdentity = { ...scope, snapshot: previous, directory: previous.directories[0] };
    for (const path of [undefined, "src/review.js"]) {
      const cached = await local.readCached(reads.reviewDiffAddress(latestIdentity, {}, path));
      await local.writeCached(reads.reviewDiffAddress(previousIdentity, {}, path), cached.value);
    }
    const commits = await local.readCached(reads.reviewCommitsAddress(latestIdentity));
    await local.writeCached(reads.reviewCommitsAddress(previousIdentity), commits.value);
    window.__loopReview.verdict_deliveries = [{ comment_id: "baseline", snapshot_id: "revision-1", state: "delivered", comment_ids: ["baseline"], anchors: [] }];
    const directory = window.__loopReview.snapshots.at(-1).directories[0];
    window.__loopSync = [{ directory_id: directory.id, revision: 2, snapshot_head: directory.head, reviewed_snapshot_id: "revision-1", commits_since_review: 3 }];
    window.__loopRetries = [];
    window.__reviewFixtureRpc = (method, params) => {
      if (method === "tasks.review.get") return { review: window.__loopReview, sync: window.__loopSync };
      if (method !== "tasks.review.retryDelivery") return undefined;
      window.__loopRetries.push(params);
      window.__loopReview = { ...window.__loopReview, version: window.__loopReview.version + 1, verdict_deliveries: window.__loopReview.verdict_deliveries.map((delivery) => ({ ...delivery, state: "pending" })) };
      return { review: window.__loopReview, sync: window.__loopSync };
    };
    window.__setLoopDelivery = async (state) => {
      window.__loopReview = { ...window.__loopReview, version: window.__loopReview.version + 1, verdict_deliveries: window.__loopReview.verdict_deliveries.map((delivery) => ({ ...delivery, state })) };
      await reviews.writeReviewReply(scope, { review: window.__loopReview, sync: window.__loopSync }, Date.now());
    };
    await reviews.writeReviewReply(scope, { review: window.__loopReview, sync: window.__loopSync }, Date.now());
    await support.rememberReviewSupport(scope.deviceId, { reviews: { get: true, diff: true, comments: true, pullRequests: true, reject: true, author: true, verdictDelivery: true, retryDelivery: true, commits: true, commit: true, since: true, anchorCommit: true } });
  });
  await page.waitForSelector("[data-review-delivery]");
}

async function mountRejectedBoard(page, basePath) {
  await page.evaluate(() => window.__reviewTaskPage.dispose());
  await mountLayout(page, '<main id="tasks"></main>', {
    basePath, styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block} #tasks{margin:16px;min-width:0;max-width:none;padding:0}`,
  });
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  await loadBrowserModules(page, { pane: "src/core/trackerTasksPane.js", cache: "src/core/trackerCache.js", support: "src/core/taskReviewSupport.js" }, basePath);
  await page.evaluate(async ({ tasks, columns }) => {
    const { pane, cache, support } = window.__layoutModules;
    await support.rememberReviewSupport("loop-board", { reviews: { reject: true, verdictDelivery: true } });
    await cache.writeTasksRecord("loop-board", "p1", cache.tasksRecord(tasks, columns, 1));
    const unanswered = new Promise(() => {});
    window.__loopBoard = pane.mountTasksPane(document.querySelector("#tasks"), {
      deviceId: "loop-board", projectId: "p1", projectKey: "loop-board|p1", projectName: "Build",
      defaultView: "board", feed: () => ({ projects: [], items: [], workspaces: [] }), callRpc: () => unanswered,
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
  }, { tasks: [task({ id: "rejected-review", number: 973, title: "Review upload recovery", status: "done", review_summary: { task_id: "rejected-review", version: 3, status: "rejected" } })], columns: columns() });
  await page.locator('[data-task="rejected-review"] .task-review-summary').waitFor();
}

for (const { label, width } of [{ label: "desktop", width: 1440 }, { label: "phone", width: 390 }]) {
  it(`renders cached review-loop facts, retry, new revision and Rejected board on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountVerdictReviewTask(page, basePath);
      await prepareLoop(page);
      expect(await page.locator("[data-review-people]").textContent()).toContain("by @Builder");
      for (const [state, text] of [["delivered", "Delivered"], ["pending", "Pending (agent unavailable)"], ["failed", "Failed"]]) {
        await page.evaluate((state) => window.__setLoopDelivery(state), state);
        await page.waitForFunction((text) => document.querySelector("[data-review-delivery]")?.textContent.includes(text), text);
        expect(await page.locator("[data-review-retry-delivery]").count()).toBe(state === "failed" ? 1 : 0);
        await assertNoOverflow(page);
        await captureLayout(page, `review-delivery-${state}-${label}.png`);
      }
      await page.locator("[data-review-retry-delivery]").click();
      await page.waitForFunction(() => window.__loopRetries.length === 1);
      expect(await page.evaluate(() => window.__loopRetries[0])).toEqual({ task_id: "task-1", comment_id: "baseline" });
      await page.waitForFunction(() => document.querySelector("[data-review-delivery]")?.textContent.includes("Pending"));
      await page.locator("[data-review-snapshot]").selectOption("revision-1");
      const banner = page.locator("[data-review-newer-snapshot]");
      await banner.waitFor({ state: "visible" });
      expect(await banner.textContent()).toContain("Revision 2 is ready · 3 commits");
      await assertNoOverflow(page);
      await captureLayout(page, `review-new-revision-${label}.png`);
      await page.locator("[data-review-new-revision]").click();
      await page.waitForFunction(() => window.__reviewNavigation.at(-1)?.review?.mode === "since");
      expect(await page.locator('[data-review-range="since"]').getAttribute("aria-pressed")).toBe("true");
      expect(await page.locator("[data-review-snapshot]").inputValue()).toBe("revision-2");
      await captureLayout(page, `review-since-my-review-${label}.png`);
      await mountRejectedBoard(page, basePath);
      const card = page.locator('[data-task="rejected-review"]');
      expect(await card.locator(".task-review-summary").textContent()).toContain("Rejected");
      expect(await card.getAttribute("data-status")).toBe("done");
      await card.scrollIntoViewIfNeeded();
      await assertNoOverflow(page);
      await captureLayout(page, `review-rejected-board-${label}.png`);
      await page.evaluate(() => window.__loopBoard.dispose());
    }, { width, height: 900, plugins: [deviceShim] });
  }, 90_000);
}
