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

async function mountPrefix(commitPrefix, { revision = 1, rpc } = {}) {
  const reviewRoute = { revision, mode: "commit", commit: commitPrefix, directoryId: directory.id,
    path: paths[1], side: "new", line: 1 };
  App.route = { ...route, review: reviewRoute };
  history.replaceState(null, "", hashFromRoute(App.route));
  const callRpc = rpc || vi.fn(() => new Promise(() => {}));
  const markReviewRoute = vi.fn(markRoute);
  page = mountTaskReviewPage(host, { ...scope, callRpc, markReviewRoute, reviewRoute });
  await page.ready;
  return { callRpc, markReviewRoute };
}

const shortCommitReads = (mock) => mock.mock.calls.filter(([method, params]) => method === "tasks.review.diff"
  && params.mode === "commit" && params.commit.length < 40);

it("lands a unique short commit link on the cached full-OID patch and canonicalizes in place", async () => {
  const { callRpc } = await mountPrefix(commit.slice(0, 7));
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(host.querySelector("[data-review-content]").textContent).toContain("const cached = true");
  expect(App.route.review).toMatchObject({ commit, path: paths[1], side: "new", line: 1 });
  expect(location.hash).toContain(`/commit/${commit}/`);
  expect(shortCommitReads(callRpc)).toEqual([]);
});

it.each(["ambiguous", "missing"])("falls back to cached All changes when a short commit is %s", async (reason) => {
  await writeCached(reviewCommitsAddress(identity), { commits: reason === "ambiguous"
    ? [{ oid: commit }, { oid: `${commit.slice(0, 7)}${"3".repeat(33)}` }] : [{ oid: "3".repeat(40) }] });
  const { callRpc } = await mountPrefix(commit.slice(0, 7));
  await painted(() => host.querySelector("[data-review-error]")?.textContent.includes("Showing all changes"));
  expect(App.route.review.mode).toBe("all");
  expect(host.querySelector("[data-review-error]").textContent).toContain(reason === "ambiguous" ? "ambiguous" : "not found");
  expect(shortCommitReads(callRpc)).toEqual([]);
});

it("defers a commit prefix until its selected revision and cached commits arrive", async () => {
  await deleteCached([reviewAddress(scope), reviewCommitsAddress(identity)]);
  const { callRpc } = await mountPrefix(commit.slice(0, 7));
  expect(host.querySelector(".task-review-changes")).toBeNull();
  expect(App.route.review.commit).toBe(commit.slice(0, 7));
  await writeReviewRecord(scope, saved, 1);
  await page.whenPainted();
  expect(host.querySelector(".task-review-changes")).toBeNull();
  await writeCached(reviewCommitsAddress(identity), { commits: [{ oid: commit }] });
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(App.route.review.commit).toBe(commit);
  expect(shortCommitReads(callRpc)).toEqual([]);
});

it("waits for the complete commit list before treating a prefix as unique", async () => {
  await writeCached(reviewCommitsAddress(identity), { commits: [{ oid: commit }], commits_truncated: true, next_cursor: "page-2" });
  const { callRpc } = await mountPrefix(commit.slice(0, 7));
  expect(host.querySelector(".task-review-changes")).toBeNull();
  expect(App.route.review.commit).toBe(commit.slice(0, 7));
  await writeCached(reviewCommitsAddress(identity), { commits: [{ oid: commit }], commits_truncated: false });
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(App.route.review.commit).toBe(commit);
  expect(shortCommitReads(callRpc)).toEqual([]);
});

it("keeps a missing requested revision pending instead of resolving against the latest cached revision", async () => {
  const { callRpc } = await mountPrefix(commit.slice(0, 7), { revision: 2 });
  expect(host.querySelector(".task-review-changes")).toBeNull();
  expect(App.route.review.revision).toBe(2);
  expect(App.route.review.commit).toBe(commit.slice(0, 7));
  const next = { ...snapshot, id: "snapshot-later", number: 2 };
  const later = { ...identity, snapshot: next };
  const comparison = { mode: "commit", commit };
  await writeCached(reviewCommitsAddress(later), { commits: [{ oid: commit }] });
  await writeCached(reviewDiffAddress(later, comparison), { files: [{ path: paths[1], content_key: paths[1] }] });
  await writeCached(reviewDiffAddress(later, comparison, paths[1]), (await readCached(reviewDiffAddress(identity, comparison, paths[1]))).value);
  await writeReviewRecord(scope, { ...saved, version: 2, snapshots: [...saved.snapshots, next] }, 2);
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(App.route.review).toMatchObject({ revision: 2, commit });
  expect(shortCommitReads(callRpc)).toEqual([]);
});

it("canonicalizes a prefix followed within an already mounted review", async () => {
  const { callRpc, markReviewRoute } = await mount({ revision: 1, mode: "all", directoryId: directory.id, path: paths[0] });
  const header = host.querySelector(".task-review-header");
  const link = document.createElement("a");
  link.className = "md-ref";
  link.href = hashFromRoute({ ...route, review: { revision: 1, mode: "commit", commit: commit.slice(0, 7),
    directoryId: directory.id, path: paths[1], side: "new", line: 1 } });
  document.body.append(link);
  link.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(App.route.review.commit).toBe(commit);
  expect(markReviewRoute.mock.calls.at(-1)[0].review.commit).toBe(commit);
  expect(host.querySelector(".task-review-header")).toBe(header);
  expect(shortCommitReads(callRpc)).toEqual([]);
  link.remove();
});

it("does not apply a deferred prefix from a directory after the reader switches", async () => {
  await deleteCached([reviewCommitsAddress(identity)]);
  const otherDirectory = { ...directory, id: "other-directory", name: "Other" };
  await writeReviewRecord(scope, { ...saved, version: 2, snapshots: [{ ...snapshot, directories: [directory, otherDirectory] }] }, 2);
  const { callRpc } = await mountPrefix(commit.slice(0, 7));
  expect(host.querySelector(".task-review-changes")).toBeNull();
  host.querySelector('[data-directory="other-directory"]').click();
  const otherIdentity = { ...identity, directory: otherDirectory };
  const otherCommit = `${commit.slice(0, 7)}${"4".repeat(33)}`;
  await writeCached(reviewCommitsAddress(identity), { commits: [{ oid: commit }] });
  expect(App.route.review.commit).toBe(commit.slice(0, 7));
  await writeCached(reviewCommitsAddress(otherIdentity), { commits: [{ oid: otherCommit }] });
  await vi.waitFor(() => expect(App.route.review.commit).toBe(otherCommit));
  expect(App.route.review.directoryId).toBe(otherDirectory.id);
  expect(callRpc.mock.calls.some(([method, params]) => method === "tasks.review.diff" && params.mode === "commit"
    && params.directory_id === directory.id)).toBe(false);
  expect(shortCommitReads(callRpc)).toEqual([]);
});

it("loads later commit pages to resolve a prefix beyond the first page", async () => {
  await deleteCached([reviewCommitsAddress(identity)]);
  const callRpc = vi.fn((method, params) => {
    if (method !== "tasks.review.diff" || params.mode !== "commits") return new Promise(() => {});
    if (!params.cursor) return Promise.resolve({ commits: [{ oid: "3".repeat(40), subject: "First page" }], next_cursor: "second-page", commits_truncated: true });
    return Promise.resolve({ commits: [{ oid: commit, subject: "Later commit" }], commits_truncated: false });
  });
  await mountPrefix(commit.slice(0, 7), { rpc: callRpc });
  await painted(() => host.querySelector("tr.task-review-anchor"));
  expect(App.route.review.commit).toBe(commit);
  expect(host.querySelector("[data-review-content]").textContent).toContain("const cached = true");
  expect(callRpc.mock.calls.filter(([, params]) => params?.mode === "commits").map(([, params]) => params.cursor || null))
    .toEqual([null, "second-page"]);
  expect(shortCommitReads(callRpc)).toEqual([]);
});

it("does not retry an unchanged failed commit page while resolving a prefix", async () => {
  await writeCached(reviewCommitsAddress(identity), { commits: [{ oid: "3".repeat(40) }], next_cursor: "unavailable-page", commits_truncated: true });
  const callRpc = vi.fn((method, params) => {
    if (method === "tasks.review.diff" && params.mode === "commits" && params.cursor) return Promise.reject(new Error("Page unavailable"));
    return new Promise(() => {});
  });
  await mountPrefix(commit.slice(0, 7), { rpc: callRpc });
  await painted(() => host.querySelector("[data-review-rail]")?.textContent.includes("Page unavailable"));
  page.feedMoved();
  page.feedMoved();
  expect(callRpc.mock.calls.filter(([, params]) => params?.cursor === "unavailable-page")).toHaveLength(1);
  expect(App.route.review.commit).toBe(commit.slice(0, 7));
  expect(shortCommitReads(callRpc)).toEqual([]);
});
