// A saved review must remain useful before any RPC answers.
import { expect, it } from "vitest";
import fixture from "../../../fixtures/api/v1/tasks.review.get.json";
import { task, comment, event } from "../trackerWireFixture.js";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const commit = "2".repeat(40);
const path = "src/review.js";
const saved = {
  ...fixture.result.review,
  snapshots: [1, 2].map((number) => ({
    ...fixture.result.review.snapshots[0], id: `revision-${number}`, number,
    directories: [fixture.result.review.snapshots[0].directories[0]],
  })),
};
const detail = {
  task: task({
    id: "task-1", number: 969, title: "Make review navigation useful", status: "in_review",
    review_summary: { task_id: "task-1", version: 1, status: "open" },
    links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: "task-parent" },
  }),
  timeline: [
    event({ id: "created", kind: "created" }),
    comment({ id: "baseline", body: "The first revision is a good start.", opinion: { snapshot_id: "revision-1", verdict: "request_changes" } }),
    comment({ id: "anchored", body: "Keep the selected file when changing commits.", anchor: {
      snapshot_id: "revision-2", directory_id: "dir-api", commit, path, side: "new", line: 2,
    } }),
  ],
};

async function mountReviewTask(page, basePath) {
  await mountLayout(page, '<div id="shell"><div id="view"><header id="toolbar">Build · Review</header><div id="view-body"><main id="root" class="surface"><div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div></main></div></div></div>', {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css"); #shell{height:100vh;box-sizing:border-box} #view-body,#root,#tabbody,#task-pane{min-width:0} #task-pane{height:100%;overflow:auto}`,
  });
  await page.evaluate(() => {
    document.documentElement.dataset.theme = "dark";
    document.head.prepend(Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" }));
  });
  await loadBrowserModules(page, {
    page: "src/core/trackerTaskPage.js", tasks: "src/core/trackerCache.js", reviews: "src/core/taskReviewCache.js",
    reads: "src/core/taskReviewReads.js", files: "src/core/taskReviewFiles.js",
    support: "src/core/taskReviewSupport.js", local: "src/core/localCache.js",
  }, basePath);
  await page.evaluate(async ({ saved, detail, commit, path }) => {
    const { page: taskPage, tasks, reviews, reads, files, support, local } = window.__layoutModules;
    const scope = { deviceId: "review-layout", projectId: "p1", taskId: "task-1" };
    const snapshot = saved.snapshots[1];
    const directory = snapshot.directories[0];
    const identity = { ...scope, snapshot, directory };
    const changed = [{ path, status: "Modified", additions: 2, deletions: 1, content_key: "code-v2" }];
    const patch = `diff --git a/${path} b/${path}\nindex 111..222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1 +1,2 @@\n-export const review = null;\n+export const review = "ready";\n+export const selection = "preserved";\n`;
    const listing = { files: changed, stat: { files_changed: 1, insertions: 2, deletions: 1 }, diff_key: "base:head", files_truncated: false };
    await tasks.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, detail);
    await reviews.writeReviewRecord(scope, saved, 1);
    await support.rememberReviewSupport(scope.deviceId, { reviews: { get: true, diff: true, comments: true, commits: true, commit: true, since: true, anchorCommit: true } });
    await local.writeCached(reads.reviewCommitsAddress(identity), { commits: [
      { oid: commit, parents: ["1".repeat(40)], subject: "Preserve review selection", author_name: "Ada", time: 1789012345, insertions: 2, deletions: 1, files_changed: 1 },
    ], commits_truncated: false });
    for (const comparison of [{}, { mode: "commit", commit }, { mode: "since", from_snapshot_id: "revision-1" }]) {
      await local.writeCached(reads.reviewDiffAddress(identity, comparison), listing);
      await local.writeCached(reads.reviewDiffAddress(identity, comparison, path), { patch, content_key: "code-v2", truncated: false });
    }
    const tree = (name) => files.taskReviewFileAddress(identity, "task-review-tree", name);
    await local.writeCached(tree(""), { path: "", entries: [{ name: "src", kind: "dir" }] });
    await local.writeCached(tree("src"), { path: "src", entries: [{ name: "review.js", kind: "file", size: 75 }] });
    const text = 'export const review = "ready";\nexport const selection = "preserved";\n';
    await local.writeCached(files.taskReviewFileAddress(identity, "task-review-file", path), { file: {
      path, size: text.length, mime: "text/plain", content_b64: btoa(text), editable: false, truncated: false,
    } });
    window.__reviewNavigation = [];
    const unanswered = new Promise(() => {});
    window.__reviewTaskPage = taskPage.mountTaskPage(document.querySelector("#task-pane"), {
      ...scope, projectKey: `${scope.deviceId}|${scope.projectId}`,
      callRpc: () => unanswered, catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ projects: [{ id: "p1", projectKey: `${scope.deviceId}|p1`, name: "Build" }], items: [], workspaces: [] }),
      navigate: (route) => { window.__reviewNavigation.push(route); },
      markReviewRoute: (route) => { window.__reviewNavigation.push(route); },
    });
  }, { saved, detail, commit, path });
  await page.waitForSelector(".review-task-template");
  await page.waitForSelector(`[data-review-commit="${commit}"]`, { state: "attached" });
  await page.waitForSelector(`[data-review-path="${path}"]`);
}

async function assertNoOverflow(page) {
  const size = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
  expect(size.page, JSON.stringify(size)).toBeLessThanOrEqual(size.viewport + 1);
}

async function openCommit(page, phone) {
  if (phone) await page.locator("[data-review-commits-open]").click();
  await page.locator(`[data-review-commit="${commit}"]`).click();
  await page.waitForFunction(() => window.__reviewNavigation.at(-1)?.review?.mode === "commit");
}

for (const { label, width } of [{ label: "desktop", width: 1440 }, { label: "phone", width: 390 }]) {
  it(`lays out cached review changes, commits, Files and anchor jumps on ${label}`, async () => {
    const phone = width <= 640;
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReviewTask(page, basePath);
      expect(await page.locator(".task-review-title").textContent()).toContain("Make review navigation useful");
      expect(await page.locator(".task-review-comments [data-task-composer]").count()).toBe(1);
      expect(await page.locator(".task-review-comments [data-review-feedback]").count()).toBe(1);
      expect(await page.locator("[data-review-comment], [data-review-file-comment]").count()).toBe(0);
      const description = page.locator(".task-review-description");
      await description.locator("summary").click();
      expect(await description.locator(".task-page-body").isVisible()).toBe(true);
      await description.locator("summary").click();
      expect(await page.locator("[data-review-snapshot]").textContent()).toContain("Revision 2 of 2");
      if (phone) {
        expect(await page.locator("[data-review-commits-open]").isVisible()).toBe(true);
        expect(await page.locator(".task-review-navigation").isVisible()).toBe(false);
        expect(await page.locator('[data-review-view="comments"]').isVisible()).toBe(true);
        expect(await page.locator(".task-review-comments").isVisible()).toBe(false);
      } else {
        const boxes = await Promise.all([".task-review-navigation", ".task-review-centre", ".task-review-comments"].map((selector) => page.locator(selector).boundingBox()));
        expect(boxes.every(Boolean)).toBe(true);
        expect(boxes[0].x + boxes[0].width).toBeLessThanOrEqual(boxes[1].x + 1);
        expect(boxes[1].x + boxes[1].width).toBeLessThanOrEqual(boxes[2].x + 1);
        expect(Math.max(...boxes.map((box) => box.y)) - Math.min(...boxes.map((box) => box.y))).toBeLessThan(50);
      }
      const expansion = page.locator(`[data-review-path="${path}"] [data-review-expand]`);
      if (await expansion.getAttribute("aria-expanded") === "false") await expansion.click();
      await page.waitForSelector('tr[data-side="new"]');
      await assertNoOverflow(page);
      await captureLayout(page, `review-page-all-${label}.png`);

      await openCommit(page, phone);
      await page.waitForSelector(`[data-review-path="${path}"]`);
      await assertNoOverflow(page);
      await captureLayout(page, `review-page-commit-${label}.png`);

      await page.locator(`[data-open-file="${path}"]`).click();
      await page.waitForFunction(() => document.querySelector(".trf-preview")?.textContent.includes('selection = "preserved"'));
      await page.waitForSelector(".trf-preview .task-review-inline");
      expect(await page.locator(".trf-preview .task-review-inline").textContent()).toContain("Keep the selected file");
      expect(await page.locator("[data-review-comment], [data-review-file-comment]").count()).toBe(0);
      await assertNoOverflow(page);
      await captureLayout(page, `review-page-files-${label}.png`);

      if (phone) await page.locator('[data-review-view="comments"]').click();
      await page.locator('#comment-anchored [data-review-anchor]').click();
      await page.waitForSelector("tr.task-review-anchor");
      expect(await page.locator("tr.task-review-anchor").getAttribute("data-new-line")).toBe("2");
      expect(await page.locator(".task-review-diff-scroll .task-review-inline").textContent()).toContain("Keep the selected file");
      const target = await page.evaluate(() => window.__reviewNavigation.at(-1).review);
      expect(target).toMatchObject({ revision: 2, mode: "commit", commit, directoryId: "dir-api", path, side: "new", line: 2 });
      await assertNoOverflow(page);
      await captureLayout(page, `review-page-anchor-${label}.png`);
      await page.evaluate(() => window.__reviewTaskPage.dispose());
    }, { width, height: 900, plugins: [deviceShim] });
  }, 90_000);
}
