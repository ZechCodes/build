import { expect, it } from "vitest";
import fixture from "../../../fixtures/api/v1/tasks.review.get.json";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const commit = "2".repeat(40);
const path = "src/review.js";
const start = "src/start.js";
const saved = { ...fixture.result.review, snapshots: [{ ...fixture.result.review.snapshots[0],
  directories: [fixture.result.review.snapshots[0].directories[0]],
}] };

async function mountReviewLinks(page, basePath, pendingCommits = false) {
  await mountLayout(page, '<main id="diff-link-demo"><header id="toolbar">Build · Diff references</header><article id="references" class="md"></article><section id="review"></section></main>', {
    basePath,
    styles: "html,body{height:100%;margin:0}body{display:block}#diff-link-demo{box-sizing:border-box;min-height:100vh;padding:20px;max-width:1440px;margin:auto}#toolbar{position:static;display:block;height:auto;min-height:0;padding:0 0 12px;background:none;border:0}#references{padding:12px 0 20px}#review{min-width:0} .task-review-diff-scroll{max-height:62vh}",
  });
  await page.evaluate(() => {
    document.documentElement.dataset.theme = "dark";
    document.head.prepend(Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" }));
  });
  await loadBrowserModules(page, {
    review: "src/core/taskReviewPage.js", reviews: "src/core/taskReviewCache.js", reads: "src/core/taskReviewReads.js",
    support: "src/core/taskReviewSupport.js", local: "src/core/localCache.js", markdown: "src/core/markdown.js",
    references: "src/core/referenceIndex.js", router: "src/core/router.js",
  }, basePath);
  await page.evaluate(async ({ saved, commit, path, start, pendingCommits }) => {
    const { review, reviews, reads, support, local, markdown, references, router } = window.__layoutModules;
    const scope = { deviceId: "diff-link-browser", projectId: "p1", taskId: "task-1" };
    const route = { name: "trackerTask", ...scope };
    const projectKey = `${scope.deviceId}/${scope.projectId}`;
    const snapshot = saved.snapshots[0];
    const directory = snapshot.directories[0];
    const identity = { ...scope, snapshot, directory };
    await reviews.writeReviewRecord(scope, saved, 1);
    await support.rememberReviewSupport(scope.deviceId, { reviews: { get: true, diff: true, commits: true, commit: true } });
    references.holdReferenceSources({
      feed: { projects: [{ id: "p1", projectKey, name: "Build" }], workspaces: [], items: [] },
      tasks: { [projectKey]: [{ id: scope.taskId, number: 969, title: "Review diff references" }] },
      reviews: { [projectKey]: { [scope.taskId]: saved } },
    });
    document.querySelector("#references").innerHTML = markdown.markdownHtml(
      `**Review a precise change:** [[#969:r1:commit/${commit.slice(0, 8)}:${directory.id}:${path}#L2]]\n\n**Compare the removed line:** [[#969:r1:all:${path}#old-L1]]`, { place: scope },
    );
    for (const comparison of [{}, { mode: "commit", commit }]) {
      await local.writeCached(reads.reviewDiffAddress(identity, comparison), {
        files: [start, path].map((name) => ({ path: name, status: "Modified", additions: 2, deletions: 1, content_key: name })),
        stat: { files_changed: 2, insertions: 4, deletions: 2 }, diff_key: "base:head", files_truncated: false,
      });
      for (const name of [start, path]) {
        await local.writeCached(reads.reviewDiffAddress(identity, comparison, name), {
          patch: `diff --git a/${name} b/${name}\n--- a/${name}\n+++ b/${name}\n@@ -1 +1,2 @@\n-export const review = null;\n+export const review = "ready";\n+export const selection = "preserved";\n`,
          content_key: name, truncated: false,
        });
      }
    }
    if (!pendingCommits) await local.writeCached(reads.reviewCommitsAddress(identity), {
      commits: [{ oid: commit, subject: "Preserve review selection", author_name: "Ada", time: 1789012345 }], commits_truncated: false,
    });
    history.replaceState(null, "", router.hashFromRoute(route));
    window.__requests = [];
    window.__navigations = [];
    window.__reviewPage = review.mountTaskReviewPage(document.querySelector("#review"), {
      ...scope, callRpc: (method, params) => {
        window.__requests.push(method);
        if (pendingCommits && method === "tasks.review.diff" && params.mode === "commits") {
          if (!window.__failCommits) return new Promise((resolve, reject) => { window.__failCommits = () => reject(new Error("Source unavailable")); });
          return Promise.resolve({ commits: [{ oid: commit }], commits_truncated: false });
        }
        return new Promise(() => {});
      },
      reviewRoute: { revision: 1, mode: pendingCommits ? "commit" : "all", commit: pendingCommits ? commit.slice(0, 8) : null,
        directoryId: directory.id, path: start },
      task: () => ({ id: scope.taskId, title: "Link directly to a review diff" }),
      markReviewRoute: (next) => { window.__navigations.push(next); history.replaceState(null, "", router.hashFromRoute(next)); },
    });
    await window.__reviewPage.ready;
  }, { saved, commit, path, start, pendingCommits });
  await page.waitForSelector(pendingCommits ? "[data-review-notice]:not([hidden])" : `[data-review-path="${start}"]`);
  await page.waitForSelector("#references a.md-ref");
}

async function assertNoOverflow(page) {
  const size = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
  expect(size.page, JSON.stringify(size)).toBeLessThanOrEqual(size.viewport + 1);
}

for (const { label, width } of [{ label: "desktop", width: 1440 }, { label: "phone", width: 390 }]) {
  it(`explains a failed first commit-list read and clears the notice on recovery on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReviewLinks(page, basePath, true);
      const notice = page.locator("[data-review-notice]");
      expect(await notice.textContent()).toBe("Waiting for this revision’s commits…");
      await page.waitForFunction(() => Boolean(window.__failCommits));
      await page.evaluate(() => window.__failCommits());
      await page.waitForFunction(() => document.querySelector("[data-review-notice]")?.textContent.includes("Couldn’t load"));
      expect(await notice.isVisible()).toBe(true);
      expect(await notice.textContent()).toBe("Couldn’t load this revision’s commits. Waiting for a successful refresh.");
      expect(await page.locator("[data-review-content]").textContent()).toBe("");
      await assertNoOverflow(page);
      await captureLayout(page, `review-sha-notice-${label}.png`);
      await page.locator("[data-review-refresh]").click();
      await page.waitForSelector(`[data-review-path="${start}"]`);
      expect(await notice.isVisible()).toBe(false);
      expect(await notice.textContent()).toBe("");
      expect(await page.evaluate(() => window.__navigations.at(-1).review.commit)).toBe(commit);
      await page.evaluate(() => window.__reviewPage.dispose());
    }, { width, height: 900, plugins: [deviceShim] });
  }, 90_000);

  it(`follows rendered review diff links from cache in place on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReviewLinks(page, basePath);
      expect(await page.locator("#references a.md-ref").count()).toBe(2);
      const href = await page.locator("#references a.md-ref").first().getAttribute("href");
      expect(href).toContain(`/review/r1/commit/${commit.slice(0, 8)}/dir-api/${path}?side=new&line=2`);
      await assertNoOverflow(page);
      await captureLayout(page, `diff-link-review-rendered-${label}.png`);
      await page.evaluate(() => { window.__requests = []; window.__reviewHeader = document.querySelector(".task-review-header"); });
      await page.locator("#references a.md-ref").first().click();
      await page.waitForSelector("tr.task-review-anchor[data-new-line='2']");
      expect(await page.evaluate(() => document.querySelector(".task-review-header") === window.__reviewHeader)).toBe(true);
      expect(await page.evaluate(() => window.__requests)).toEqual([]);
      expect(await page.evaluate(() => window.__navigations.at(-1).review)).toMatchObject({ mode: "commit", commit, path, side: "new", line: 2 });
      await assertNoOverflow(page);
      await captureLayout(page, `diff-link-review-landed-${label}.png`);
      await page.locator("#references a.md-ref").nth(1).click();
      await page.waitForSelector("tr.task-review-anchor[data-old-line='1']");
      expect(await page.evaluate(() => window.__navigations.at(-1).review)).toMatchObject({ mode: "all", path, side: "old", line: 1 });
      expect(await page.evaluate(() => window.__requests)).toEqual([]);
      await page.evaluate(() => window.__reviewPage.dispose());
    }, { width, height: 900, plugins: [deviceShim] });
  }, 90_000);
}

async function mountWorkspaceLinks(page, basePath) {
  await mountLayout(page, '<main id="diff-link-demo"><header>Build · Workspace Changes</header><article id="references" class="md"></article><section id="git"></section></main>', {
    basePath,
    styles: "html,body{height:100%;margin:0}body{display:block}#diff-link-demo{height:100vh;display:flex;flex-direction:column;box-sizing:border-box;padding:20px}#references{padding:14px 0}#git{position:relative;overflow:hidden;flex:1;min-height:0;border-top:1px solid var(--line)}",
  });
  await page.evaluate(() => {
    document.documentElement.dataset.theme = "dark";
    document.head.prepend(Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" }));
  });
  await loadBrowserModules(page, {
    git: "src/core/gitPane.js", local: "src/core/localCache.js", scopes: "src/core/cacheScope.js",
    markdown: "src/core/markdown.js", references: "src/core/referenceIndex.js", router: "src/core/router.js",
    directory: "src/core/directoryScope.js",
  }, basePath);
  await page.evaluate(async ({ commit, path }) => {
    const { git, local, scopes, markdown, references, router, directory } = window.__layoutModules;
    const deviceId = "diff-link-workspace";
    const projectId = "p1";
    const projectKey = `${deviceId}/${projectId}`;
    const scope = { workspace_id: "ws-1", source_id: "source-1" };
    const entityId = directory.directoryCacheId(scope);
    const historyRow = { hash: commit, short: commit.slice(0, 8), subject: "Keep a precise diff reading place", author: "Ada", time: Math.floor(Date.now() / 1000) - 30 };
    const address = (kind, sub) => ({ deviceId, entityId, kind, ...(sub ? { sub } : {}) });
    await local.writeCached(address("status"), {
      branch: "feature", head: commit, repo_state: "clean", status_key: "status-1", files: [],
      upstream: "origin/main", ahead: 0, behind: 0, stash_count: 0, stat: { files_changed: 0, insertions: 0, deletions: 0 },
    });
    await local.writeCached(address("log"), { commits: [historyRow], more: false });
    await local.writeCached(address("patch", commit), {
      ...historyRow, body: "A file and line can be shared directly.", stat: { files_changed: 1, insertions: 2, deletions: 1 }, truncated: false,
      patch: `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1 +1,2 @@\n-export const review = null;\n+export const review = "ready";\n+export const selection = "preserved";\n`,
    });
    references.holdReferenceSources({ feed: {
      projects: [{ id: projectId, projectKey, name: "Build" }], items: [],
      workspaces: [{ id: "ws-1", projectKey, name: "review-work", directories: [{ source_id: "source-1", name: "Code" }] }],
    } });
    document.querySelector("#references").innerHTML = markdown.markdownHtml(
      `**Open the changed line:** [[review-work:commit:${commit}:${path}#L2]]\n\n**Read the removed line:** [[review-work:commit:${commit}:${path}#old-L1]]`,
      { place: { deviceId, projectId } },
    );
    window.__requests = [];
    window.__workspacePane = null;
    window.__openCommitLink = (hash) => {
      const route = router.routeFromHash(hash);
      history.replaceState(null, "", hash);
      window.__workspacePane?.dispose();
      window.__workspacePane = git.mountGitPane(document.querySelector("#git"), {
        scope, cacheScope: scopes.scopeFor(deviceId),
        callRpc: (method) => { window.__requests.push(method); return new Promise(() => {}); },
        requestedCommit: route.commit, requestedDiff: { path: route.path, side: route.side, line: route.line },
      });
    };
    document.querySelector("#references").addEventListener("click", (event) => {
      const anchor = event.target.closest("a.md-ref");
      if (!anchor) return;
      event.preventDefault();
      window.__openCommitLink(anchor.getAttribute("href"));
    });
  }, { commit, path });
}

for (const { label, width } of [{ label: "desktop", width: 1440 }, { label: "phone", width: 390 }]) {
  it(`opens a rendered workspace commit file and line from cache on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountWorkspaceLinks(page, basePath);
      expect(await page.locator("#references a.md-ref").count()).toBe(2);
      const href = await page.locator("#references a.md-ref").first().getAttribute("href");
      expect(href).toContain(`/changes?commit=${commit}&path=src%2Freview.js&side=new&line=2`);
      await captureLayout(page, `diff-link-workspace-rendered-${label}.png`);
      await page.locator("#references a.md-ref").first().click();
      await page.waitForSelector("tr.diff-link-anchor[data-new-line='2']");
      expect(await page.evaluate(() => window.__requests.includes("git.show"))).toBe(false);
      await assertNoOverflow(page);
      await captureLayout(page, `diff-link-workspace-landed-${label}.png`);
      await page.locator("#references a.md-ref").nth(1).click();
      await page.waitForSelector("tr.diff-link-anchor[data-old-line='1']");
      expect(await page.evaluate(() => window.__requests.includes("git.show"))).toBe(false);
      await page.evaluate(() => window.__workspacePane.dispose());
    }, { width, height: 900, plugins: [deviceShim] });
  }, 90_000);
}
