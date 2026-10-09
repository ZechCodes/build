import { expect, it } from "vitest";
import opened from "../../../fixtures/api/v1/tasks.review.open.json";
import pushed from "../../../fixtures/api/v1/tasks.review.push.json";
import merged from "../../../fixtures/api/v1/tasks.review.merge.json";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const viewports = [
  { label: "mobile", width: 390, hasTouch: true },
  { label: "desktop", width: 1280, hasTouch: false },
];

async function assertNoPageOverflow(page) {
  const size = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
  expect(size.page, JSON.stringify(size)).toBeLessThanOrEqual(size.viewport + 1);
}

async function activate(page, selector, hasTouch) {
  const control = page.locator(selector);
  if (hasTouch) await control.tap();
  else {
    await control.scrollIntoViewIfNeeded();
    await control.focus();
    await control.press("Enter");
  }
}

async function mountPullRequest(page, basePath) {
  await mountLayout(page, '<div id="shell"><div id="view"><header id="toolbar">Build · PR review</header><div id="view-body"><main id="root" class="surface"><div id="tabbody" class="flush"><div id="task-header"></div><div id="review"></div></div></main></div></div></div>', {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css"); #shell{height:100vh;box-sizing:border-box} #view-body,#root,#tabbody{min-width:0} #task-header{padding:var(--pane-top) var(--pane-gutter) 0} #review{width:100%;box-sizing:border-box;padding:1rem}`,
  });
  await page.evaluate(() => {
    document.head.prepend(Object.assign(document.createElement("meta"), {
      name: "viewport", content: "width=device-width, initial-scale=1",
    }));
  });
  await loadBrowserModules(page, {
    review: "src/core/taskReviewPage.js",
    support: "src/core/taskReviewSupport.js",
    cache: "src/core/taskReviewCache.js",
    local: "src/core/localCache.js",
    taskRender: "src/core/trackerTaskRender.js",
  }, basePath);
  await page.evaluate(async (answer) => {
    const { review, support, cache, local, taskRender } = window.__layoutModules;
    document.querySelector("#task-header").innerHTML = taskRender.taskHeadHtml(answer.task);
    const scope = { deviceId: "pr-layout-device", projectId: "pr-layout-project", taskId: "task-1" };
    const saved = structuredClone(answer.review);
    const snapshot = saved.snapshots[0];
    saved.destinations = saved.bindings.map((binding) => ({
      snapshot_id: snapshot.id, directory_id: binding.directory_id,
      source_path: binding.source_repository, branches: ["main"],
      remotes: [{ name: "origin", branches: ["main"] }], live_head: "1".repeat(40),
    }));
    const workspaceAddress = { deviceId: scope.deviceId, entityId: "", kind: "workspaces" };
    const workspace = { id: "workspace-1", workspace_id: "workspace-1", project_id: scope.projectId,
      name: "review-api-and-ui", locked: true, directories: snapshot.directories };
    const sync = saved.bindings.map((binding) => ({ directory_id: binding.directory_id,
      working_head: snapshot.directories.find((row) => row.id === binding.directory_id).head,
      received_head: binding.last_received_head, target_head: "1".repeat(40),
      health: "current", pending_commits: 0 }));
    const patch = "diff --git a/changed.txt b/changed.txt\n--- a/changed.txt\n+++ b/changed.txt\n@@ -1 +1 @@\n-before\n+Reviewed API change\n";
    const calls = [];
    const callRpc = async (method, params) => {
      calls.push({ method, params });
      // These reads remain outstanding: all lifecycle facts must paint from
      // the seeded or subsequently updated cache before any RPC resolves.
      if (method === "tasks.review.get") return new Promise(() => {});
      if (method === "tasks.review.diff") return {
        stat: { files_changed: 1, insertions: 1, deletions: 1 },
        files: [{ path: "changed.txt", status: "Modified", additions: 1, deletions: 1, content_key: `${params.snapshot_id}:change` }],
        files_truncated: false, patch: params.paths ? patch : null, truncated: false,
      };
      throw new Error(`Unexpected layout RPC ${method}`);
    };
    await support.rememberReviewSupport(scope.deviceId, { reviews: {
      get: true, diff: true, comments: true, pullRequests: true,
      update: true, merge: true, close: true, reopen: true, refresh: true,
    } });
    await local.writeCached(workspaceAddress, [workspace]);
    await cache.writeReviewReply(scope, { ...answer, review: saved, sync }, 1);
    const mounted = review.mountTaskReviewPage(document.querySelector("#review"), {
      ...scope, callRpc, task: () => answer.task, workspaces: () => [workspace],
    });
    window.__pullRequestLayout = { mounted, scope, calls, workspaceAddress, workspace };
    await mounted.ready;
  }, opened.result);
  await page.waitForSelector('[data-review-pr-status]');
  await page.waitForSelector('[data-review-path="changed.txt"]');
}

async function publishNewerSnapshot(page) {
  await page.evaluate(async (answer) => {
    const { cache, local } = window.__layoutModules;
    const { scope } = window.__pullRequestLayout;
    const held = (await local.readCached(cache.reviewAddress(scope))).value;
    const latest = structuredClone(answer.review.snapshots.at(-1));
    latest.id = "snapshot-2";
    latest.number = 2;
    const review = { ...held.review, version: 2, snapshots: [...held.review.snapshots, latest],
      pull_request: { ...held.review.pull_request, status: "changes_requested", latest_published_snapshot_id: latest.id } };
    await cache.writeReviewReply(scope, { review, sync: held.sync }, 2);
  }, pushed.result);
  await page.waitForFunction(() => document.querySelector('[data-review-pr-status]')?.textContent.includes("Changes requested"));
}

async function setPrStatus(page, status, version) {
  await page.evaluate(async ({ status, version }) => {
    const { cache, local } = window.__layoutModules;
    const { scope } = window.__pullRequestLayout;
    const held = (await local.readCached(cache.reviewAddress(scope))).value;
    const review = { ...held.review, version, pull_request: { ...held.review.pull_request, status } };
    await cache.writeReviewReply(scope, { review }, version);
  }, { status, version });
}

for (const { label, width, hasTouch } of viewports) {
  it(`keeps PR lifecycle controls and historical feedback usable on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await mountPullRequest(page, basePath);
      expect(await page.locator(".task-page-title").count()).toBe(1);
      expect(await page.locator(".task-page-title").textContent()).toBe(opened.result.task.title);
      expect(await page.locator('[data-review-pr-status]').textContent()).toContain("Open");
      expect(await page.locator('[data-review-save], [data-review-complete], [data-review-act]').count()).toBe(0);

      await activate(page, '[data-pr-merge-sheet] > summary', hasTouch);
      await page.locator('[data-pr-merge-push="dir-api"]').check();
      const branch = page.locator('[data-pr-merge-branch="dir-api"]');
      await branch.fill("release/review-api");
      await branch.evaluate((field) => field.setSelectionRange(8, 14));
      await branch.focus();
      await page.waitForFunction(() => document.querySelector('[data-pr-merge-submit]')?.disabled === false);
      expect(await page.locator('[data-pr-merge-submit]').isEnabled()).toBe(true);
      await assertNoPageOverflow(page);
      await page.locator(".task-page-head").scrollIntoViewIfNeeded();
      await captureLayout(page, `task-pr-open-${label}.png`);

      await activate(page, '[data-review-path="changed.txt"] [data-review-expand]', hasTouch);
      await page.waitForSelector('[data-review-comment][data-side="new"][data-line="1"]');
      const fileName = await page.locator('[data-review-path="changed.txt"] [data-review-expand]').boundingBox();
      expect(fileName.height).toBeLessThanOrEqual(48);
      await activate(page, '[data-review-comment][data-side="new"][data-line="1"]', hasTouch);
      const feedback = page.locator("#task-review-feedback-body");
      await feedback.fill("Keep this feedback attached to the snapshot I reviewed.");
      await page.locator('[data-review-opinion]').selectOption("request_changes");
      await feedback.focus();
      await feedback.evaluate((field) => {
        field.setSelectionRange(10, 18);
        window.__historicalFeedback = field;
      });
      await publishNewerSnapshot(page);
      await page.waitForSelector('[data-review-newer-snapshot]');
      expect(await page.locator('[data-review-newer-snapshot]').textContent()).toBe(
        "A newer snapshot arrived while you were reviewing. Review the latest snapshot before merging.",
      );
      expect(await page.locator('[data-review-snapshot]').inputValue()).toBe("snapshot-1");
      expect(await page.locator('[data-review-target]').textContent()).toContain("changed.txt · new line 1");
      expect(await page.locator('[data-review-opinion]').inputValue()).toBe("request_changes");
      expect(await feedback.evaluate((field) => ({
        retained: field === window.__historicalFeedback, focused: document.activeElement === field,
        start: field.selectionStart, end: field.selectionEnd, body: field.value,
      }))).toEqual({ retained: true, focused: true, start: 10, end: 18,
        body: "Keep this feedback attached to the snapshot I reviewed." });
      expect(await page.locator('[data-pr-merge-submit]').isDisabled()).toBe(true);
      await assertNoPageOverflow(page);
      await page.locator(".task-page-head").scrollIntoViewIfNeeded();
      await captureLayout(page, `task-pr-newer-snapshot-${label}.png`);

      const closeSummary = page.locator("details").filter({ has: page.locator('[data-review-close]') }).locator(":scope > summary");
      if (hasTouch) await closeSummary.tap();
      else { await closeSummary.focus(); await closeSummary.press("Enter"); }
      const closeDescription = page.locator('[data-review-close-description]');
      await closeDescription.fill("Closing this review after the new publication.");
      await setPrStatus(page, "closed", 3);
      await page.waitForFunction(() => document.querySelector('[data-review-pr-status]')?.textContent.includes("Closed"));
      await page.waitForSelector('[data-review-reopen]');
      await activate(page, '[data-review-advanced] > summary', hasTouch);
      expect(await page.locator('[data-review-repair]').isVisible()).toBe(true);
      await assertNoPageOverflow(page);
      await page.locator(".task-page-head").scrollIntoViewIfNeeded();
      await captureLayout(page, `task-pr-closed-${label}.png`);

      await page.evaluate(async (answer) => {
        const { cache, local } = window.__layoutModules;
        const { scope } = window.__pullRequestLayout;
        const held = (await local.readCached(cache.reviewAddress(scope))).value;
        await cache.writeReviewReply(scope, { ...answer,
          review: { ...held.review, version: 4, actions: answer.review.actions,
            pull_request: { ...held.review.pull_request, status: "merged" } },
        }, 4);
      }, merged.result);
      await page.waitForFunction(() => document.querySelector('[data-review-pr-status]')?.textContent.includes("Merged"));
      expect(await page.locator('[data-review-reopen]').count()).toBe(0);
      expect(await page.locator('[data-review-close]').count()).toBe(0);
      const reclaim = page.locator('[data-review-reclaim]');
      await page.waitForSelector('[data-review-reclaim]');
      expect(await reclaim.isDisabled()).toBe(true);
      expect(await reclaim.getAttribute("title")).toBe("Unlock the workspace to delete it");
      await page.evaluate(async () => {
        const { local } = window.__layoutModules;
        const fixture = window.__pullRequestLayout;
        window.__keptReclaim = document.querySelector('[data-review-reclaim]');
        await local.writeCached(fixture.workspaceAddress, [{ ...fixture.workspace, locked: false }]);
      });
      await page.waitForFunction(() => document.querySelector('[data-review-reclaim]')?.disabled === false);
      expect(await reclaim.isEnabled()).toBe(true);
      expect(await reclaim.evaluate((button) => button === window.__keptReclaim)).toBe(true);
      await reclaim.focus();
      await page.evaluate(async () => {
        const { local } = window.__layoutModules;
        const fixture = window.__pullRequestLayout;
        await local.writeCached(fixture.workspaceAddress, [{ ...fixture.workspace, locked: true }]);
      });
      await page.waitForFunction(() => document.querySelector('[data-review-reclaim]')?.disabled === true);
      expect(await reclaim.getAttribute("title")).toBe("Unlock the workspace to delete it");
      await assertNoPageOverflow(page);
      await page.locator(".task-page-head").scrollIntoViewIfNeeded();
      await captureLayout(page, `task-pr-merged-locked-${label}.png`);
      expect(errors).toEqual([]);
      expect(await page.evaluate(() => window.__pullRequestLayout.calls.every(({ method }) =>
        method === "tasks.review.get" || method === "tasks.review.diff"))).toBe(true);
      await page.evaluate(() => window.__pullRequestLayout.mounted.dispose());
    }, { width, height: 900, hasTouch, plugins: [deviceShim] });
  }, 60_000);

}
