import { expect, it } from "vitest";
import fixture from "../../../fixtures/api/v1/tasks.review.get.json";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

async function mountReview(page, basePath) {
  await mountLayout(page, '<div id="shell"><div id="view"><header id="toolbar">Review</header><div id="view-body"><main id="root" class="surface"><div id="tabbody" class="flush"><div id="review"></div></div></main></div></div></div>', {
    basePath, styles: '#shell{height:100vh;box-sizing:border-box} #view-body,#root,#tabbody{min-width:0} #review{width:100%;box-sizing:border-box;padding:1rem}',
  });
  await page.evaluate(() => {
    const viewport = Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" });
    document.head.prepend(viewport);
  });
  // The app's production entry establishes its connection/module cycle first.
  await loadBrowserModules(page, { app: "src/app.js" }, basePath);
  await page.evaluate(() => { delete window.__layoutModules; });
  await loadBrowserModules(page, {
    review: "src/core/taskReviewPage.js",
    support: "src/core/taskReviewSupport.js",
    cache: "src/core/taskReviewCache.js",
  }, basePath);
  await page.evaluate(async (saved) => {
    const { review, support, cache } = window.__layoutModules;
    const scope = { deviceId: "layout-device", projectId: "layout-project", taskId: "task-1" };
    const encode = (text) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
    const body = (path, text) => ({ path, size: text.length, mime: "text/plain", truncated: false, editable: false, content_b64: encode(text) });
    const tree = {
      "": [{ name: "src", kind: "dir" }, { name: "changed.txt", kind: "file", size: 20 }, { name: "unchanged.txt", kind: "file", size: 23 }],
      src: [{ name: "nested.txt", kind: "file", size: 12 }],
    };
    const callRpc = async (method, params) => {
      if (method === "tasks.review.get") return { review: saved };
      if (method === "tasks.review.diff" && params.mode === "changes") return {
        stat: { files_changed: 1, insertions: 1, deletions: 0 }, files: [{ path: "changed.txt", status: "Modified", additions: 1, deletions: 0, content_key: "change-1" }],
        files_truncated: false, patch: null, truncated: false, diff_key: "base:head",
      };
      if (method === "tasks.review.diff" && params.mode === "tree") return { path: params.path, entries: tree[params.path] || [] };
      if (method === "tasks.review.diff" && params.mode === "blob") return body(params.path, params.path === "unchanged.txt" ? "Saved unchanged content\n" : "Saved changed content\n");
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "notes.md", kind: "file", size: 16 }] };
      if (method === "fs.read") return body(params.path, "Live notes here\n");
      throw new Error(`unexpected ${method}`);
    };
    await support.rememberReviewSupport(scope.deviceId, { reviews: { get: true, snapshot: true, diff: true, complete: true, comments: false } });
    await cache.writeReviewRecord(scope, saved, 1);
    window.__reviewLayout = review.mountTaskReviewPage(document.querySelector("#review"), {
      ...scope, callRpc, workspaces: () => [], task: () => ({ id: "task-1" }),
    });
  }, fixture.result.review);
  await page.waitForSelector('[data-review-path="changed.txt"]');
}

for (const { label, width } of [{ label: "mobile", width: 390 }, { label: "desktop", width: 1280 }]) {
  it(`keeps saved and live review files usable without page overflow on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReview(page, basePath);
      await page.locator('[data-open-file="changed.txt"]').click();
      await page.waitForSelector('.frow[data-path="unchanged.txt"]');
      await page.locator('.frow[data-path="unchanged.txt"]').dblclick();
      await page.waitForFunction(() => document.querySelector(".trf-preview")?.textContent.includes("Saved unchanged content"));
      const gitWidth = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
      expect(gitWidth.page, JSON.stringify(gitWidth)).toBeLessThanOrEqual(gitWidth.viewport + 1);
      await captureLayout(page, `task-review-files-${label}.png`);

      await page.locator('[data-directory="dir-notes"]').click();
      await page.waitForSelector('.task-review-files .trf-labels');
      await page.waitForFunction(() => document.querySelector(".trf-labels")?.textContent.includes("Live files — not saved with this review"));
      expect(await page.locator("#review").textContent()).toContain("Not a Git repository");
      const liveWidth = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
      expect(liveWidth.page, JSON.stringify(liveWidth)).toBeLessThanOrEqual(liveWidth.viewport + 1);
      await captureLayout(page, `task-review-live-${label}.png`);
      await page.evaluate(() => window.__reviewLayout.dispose());
    }, { width, height: 800 });
  }, 60_000);
}
