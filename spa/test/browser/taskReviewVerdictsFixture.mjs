// Seed saved source bytes; comment writes publish through the real task cache.
import fixture from "../../../fixtures/api/v1/tasks.review.get.json";
import { task, comment, event } from "../trackerWireFixture.js";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

export const commit = "2".repeat(40);
export const path = "src/review.js";
const saved = {
  ...fixture.result.review,
  snapshots: [1, 2].map((number) => ({
    ...fixture.result.review.snapshots[0], id: `revision-${number}`, number,
    directories: [fixture.result.review.snapshots[0].directories[0]],
  })),
};
const detail = {
  task: task({
    id: "task-1", number: 970, title: "Review inline comments and verdicts", status: "in_review",
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

export async function mountVerdictReviewTask(page, basePath) {
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
    references: "src/core/referenceIndex.js", router: "src/core/router.js",
  }, basePath);
  await page.evaluate(async ({ saved, detail, commit, path }) => {
    const { page: taskPage, tasks, reviews, reads, files, support, local, references, router } = window.__layoutModules;
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
    const referenceProjectKey = `${scope.deviceId}/${scope.projectId}`;
    references.holdReferenceSources({
      feed: { projects: [{ id: "p1", projectKey: referenceProjectKey, name: "Build" }], workspaces: [], items: [] },
      tasks: { [referenceProjectKey]: [detail.task] },
      reviews: { [referenceProjectKey]: { [scope.taskId]: saved } },
    });
    history.replaceState(null, "", router.hashFromRoute({ name: "trackerTask", ...scope }));
    window.__reviewNavigation = [];
    window.__reviewCommentCalls = [];
    const unanswered = new Promise(() => {});
    window.__reviewTaskPage = taskPage.mountTaskPage(document.querySelector("#task-pane"), {
      ...scope, projectKey: `${scope.deviceId}|${scope.projectId}`,
      callRpc: async (method, params) => {
        if (method === "tasks.get") return detail;
        if (method === "tasks.review.get") return { review: saved };
        if (method === "tasks.comment") {
          const entry = {
            type: "comment", id: `posted-${window.__reviewCommentCalls.length + 1}`,
            task_id: scope.taskId, author: { kind: "user" }, refs: [],
            created_at: new Date().toISOString(), ...params,
          };
          window.__reviewCommentCalls.push(params);
          detail.timeline.push(entry);
          await tasks.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, detail);
          return { comment: entry };
        }
        return unanswered;
      }, catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ projects: [{ id: "p1", projectKey: `${scope.deviceId}|p1`, name: "Build" }], items: [], workspaces: [] }),
      navigate: (route) => { window.__reviewNavigation.push(route); },
      markReviewRoute: (route) => { window.__reviewNavigation.push(route); history.replaceState(null, "", router.hashFromRoute(route)); },
    });
  }, { saved, detail, commit, path });
  await page.waitForSelector(".review-task-template");
  await page.waitForSelector(`[data-review-commit="${commit}"]`, { state: "attached" });
  await page.waitForSelector(`[data-review-path="${path}"]`);
}
