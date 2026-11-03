// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { App, markRoute } from "../src/app.js";
import { deleteCached, readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { reviewAddress, writeReviewRecord } from "../src/core/taskReviewCache.js";
import { reviewCommitsAddress, reviewDiffAddress } from "../src/core/taskReviewReads.js";
import { taskReviewFileAddress } from "../src/core/taskReviewFiles.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { mountTaskReviewPage } from "../src/core/taskReviewPage.js";
import { hashFromRoute } from "../src/core/router.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";
import { called, painted } from "./waits.js";

const scope = { deviceId: "navigation-device", projectId: "navigation-project", taskId: "navigation-task" };
const saved = { ...fixture.result.review, task_id: scope.taskId, snapshots: [{ ...fixture.result.review.snapshots[0],
  directories: [fixture.result.review.snapshots[0].directories[0]],
}] };
const snapshot = saved.snapshots[0];
const directory = snapshot.directories[0];
const identity = { ...scope, snapshot, directory };
const commit = "2".repeat(40);
const paths = ["first.js", "second.js"];
const route = { name: "trackerTask", ...scope };
let page;
let host;

beforeEach(async () => {
  await wipeCache();
  await wipeUiRecords();
  document.body.innerHTML = '<div id="review"></div>';
  host = document.querySelector("#review");
  App.viewingContext = { clear: vi.fn() };
  App.route = route;
  history.replaceState(null, "", hashFromRoute(route));
  await writeReviewRecord(scope, saved, 1);
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, diff: true, commits: true, commit: true } });
  await writeCached(reviewCommitsAddress(identity), { commits: [{ oid: commit, subject: "Cached commit", author_name: "Ada" }] });
  for (const comparison of [{}, { mode: "commit", commit }]) {
    await writeCached(reviewDiffAddress(identity, comparison), { files: paths.map((path) => ({ path, status: "M", content_key: path })) });
    for (const path of paths) await writeCached(reviewDiffAddress(identity, comparison, path), {
      content_key: path, truncated: false,
      patch: `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -0,0 +1 @@\n+const cached = true;\n`,
    });
  }
  await writeCached(taskReviewFileAddress(identity, "task-review-tree"), {
    path: "", entries: paths.map((path) => ({ name: path, path, kind: "file" })),
  });
  for (const path of paths) await writeCached(taskReviewFileAddress(identity, "task-review-file", path), {
    file: { path, mime: "text/javascript", content_b64: btoa("const cached = true;\n"), size: 21 },
  });
});
afterEach(() => { page?.dispose(); page = null; });

async function mount(reviewRoute) {
  const callRpc = vi.fn(() => new Promise(() => {}));
  const navigate = vi.fn(() => page?.dispose());
  const markReviewRoute = vi.fn(markRoute);
  page = mountTaskReviewPage(host, { ...scope, callRpc, navigate, markReviewRoute, reviewRoute,
    task: () => ({ id: scope.taskId, title: "Cached review" }),
  });
  await page.ready;
  await painted(() => host.querySelector("[data-review-commit]"));
  await painted(() => host.querySelector("[data-review-content]")?.textContent.includes("const cached = true"));
  await called(callRpc, (mock) => mock.mock.calls.some(([method]) => method === "tasks.review.get"));
  callRpc.mockClear();
  navigate.mockClear();
  markReviewRoute.mockClear();
  return { callRpc, navigate, markReviewRoute };
}

it("selects a cached commit in place and replaces the link without a refresh RPC", async () => {
  const { callRpc, navigate, markReviewRoute } = await mount({ revision: 1, mode: "all", directoryId: directory.id, path: paths[0] });
  const header = host.querySelector(".task-review-header");
  const more = host.querySelector("[data-review-more]");
  more.open = true;
  const entries = history.length;
  host.querySelector("[data-review-commit]").click();
  await vi.waitFor(() => expect(markReviewRoute).toHaveBeenCalledWith(expect.objectContaining({
    review: expect.objectContaining({ mode: "commit", commit, path: paths[0] }),
  })));
  await vi.waitFor(() => expect(host.querySelector("[data-review-path]")?.textContent).toContain("const cached = true"));
  expect(host.querySelector(".task-review-header")).toBe(header);
  expect(host.querySelector("[data-review-more]")).toBe(more);
  expect(more.open).toBe(true);
  expect(navigate).not.toHaveBeenCalled();
  expect(history.length).toBe(entries);
  expect(location.hash).toBe(hashFromRoute(App.route));
  expect(location.hash).toContain(`/commit/${commit}/${directory.id}/${paths[0]}`);
  expect(callRpc).not.toHaveBeenCalled();
});

it("selects cached Files paths in place without replacing the pane or refreshing", async () => {
  const { callRpc, navigate, markReviewRoute } = await mount({ revision: 1, mode: "all", directoryId: directory.id, path: paths[0], view: "files" });
  await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain("const cached = true"));
  const header = host.querySelector(".task-review-header");
  const preview = host.querySelector(".trf-preview");
  const tree = host.querySelector(".trf-tree");
  const entries = history.length;
  const file = [...tree.querySelectorAll("[data-path]")].find((row) => row.dataset.path === paths[1]);
  expect(file).toBeTruthy();
  file.click();
  await vi.waitFor(() => expect(markReviewRoute).toHaveBeenCalledWith(expect.objectContaining({
    review: expect.objectContaining({ view: "files", path: paths[1] }),
  })));
  await vi.waitFor(() => expect(preview.textContent).toContain(paths[1]));
  expect(host.querySelector(".task-review-header")).toBe(header);
  expect(host.querySelector(".trf-preview")).toBe(preview);
  expect(host.querySelector(".trf-tree")).toBe(tree);
  expect(navigate).not.toHaveBeenCalled();
  expect(history.length).toBe(entries);
  expect(location.hash).toBe(hashFromRoute(App.route));
  expect(callRpc).not.toHaveBeenCalled();
  expect((await readCached(taskReviewFileAddress(identity, "task-review-file", paths[1])))?.value.file).toBeTruthy();
});


it("follows a rendered reference to this review in place while RPC remains unanswered", async () => {
  const { callRpc, navigate, markReviewRoute } = await mount({ revision: 1, mode: "all", directoryId: directory.id, path: paths[0] });
  const header = host.querySelector(".task-review-header");
  const link = document.createElement("a");
  link.className = "md-ref";
  link.href = hashFromRoute({ ...route, review: { revision: 1, mode: "commit", commit, directoryId: directory.id,
    path: paths[1], side: "new", line: 1 } });
  document.body.append(link);
  const event = new MouseEvent("click", { bubbles: true, cancelable: true });
  link.dispatchEvent(event);
  expect(event.defaultPrevented).toBe(true);
  await vi.waitFor(() => expect(markReviewRoute).toHaveBeenCalledWith(expect.objectContaining({
    review: expect.objectContaining({ mode: "commit", commit, path: paths[1], side: "new", line: 1 }),
  })));
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(host.querySelector(".task-review-header")).toBe(header);
  expect(navigate).not.toHaveBeenCalled();
  expect(callRpc).not.toHaveBeenCalled();
  link.remove();
});


it("resolves an uncached reference when the saved revision arrives and canonicalizes its route", async () => {
  await deleteCached([reviewAddress(scope)]);
  const fileRef = `${directory.id}:${paths[1]}`;
  const reviewRoute = { revision: 1, mode: "commit", commit, fileRef, side: "new", line: 1 };
  App.route = { ...route, review: reviewRoute };
  history.replaceState(null, "", hashFromRoute(App.route));
  const markReviewRoute = vi.fn(markRoute);
  page = mountTaskReviewPage(host, { ...scope, callRpc: () => new Promise(() => {}), markReviewRoute, reviewRoute });
  await page.ready;
  expect(location.hash).toContain("fileRef=");
  await writeReviewRecord(scope, saved, 1);
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(host.querySelector("tr.task-review-anchor").getAttribute("data-new-line")).toBe("1");
  expect(App.route.review).toMatchObject({ directoryId: directory.id, path: paths[1], mode: "commit", commit });
  expect(location.hash).not.toContain("fileRef=");
});

it("opens All changes with a short notice when a linked directory is unknown", async () => {
  await mount({ revision: 1, mode: "commit", commit, fileRef: "missing:src/name:part.js", side: "old", line: 42 });
  expect(host.querySelector("[data-review-error]").textContent).toBe("File not found in this revision. Showing all changes.");
  expect(App.route.review).toMatchObject({ revision: 1, mode: "all" });
  expect(App.route.review.fileRef).toBeUndefined();
  expect(App.route.review.path).toBeUndefined();
});
