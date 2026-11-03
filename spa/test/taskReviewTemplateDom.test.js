// @vitest-environment jsdom
// Review tasks must mount from held records before any wire request answers.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";
import { columns, comment, task } from "./trackerWireFixture.js";

const delayed = vi.hoisted(() => ({ review: null, commits: null, commitReads: 0 }));
vi.mock("../src/core/localCache.js", async (original) => {
  const cache = await original();
  return { ...cache, readCached: async (address) => {
    if (address.kind === "task-review") await delayed.review;
    if (address.kind === "task-review-commits") await delayed.commits;
    const held = await cache.readCached(address);
    if (address.kind === "task-review-commits") delayed.commitReads++;
    return held;
  } };
});
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({ tasks: {}, changes: { subscriptions: true, kinds: ["tasks"] } }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => true, reconnecting: () => true, moved: () => () => {} }),
}));

const scope = { deviceId: "review-device", projectId: "review-project", taskId: "task-1" };
const commit = "2".repeat(40);
const path = "src/a.js";
const saved = { ...fixture.result.review, snapshots: [{ ...fixture.result.review.snapshots[0],
  directories: [fixture.result.review.snapshots[0].directories[0]],
}] };
const anchor = { snapshot_id: saved.snapshots[0].id, directory_id: "dir-api", path, side: "new", line: 1, commit };

let cache, tasks, reviews, reads, support, mountTaskPage;
let host, page;
beforeEach(async () => {
  vi.resetModules();
  delayed.review = null;
  delayed.commits = null;
  delayed.commitReads = 0;
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  await cache.wipeCache();
  tasks = await import("../src/core/trackerCache.js");
  reviews = await import("../src/core/taskReviewCache.js");
  reads = await import("../src/core/taskReviewReads.js");
  support = await import("../src/core/taskReviewSupport.js");
  ({ mountTaskPage } = await import("../src/core/trackerTaskPage.js"));
  host = document.createElement("div");
  host.scrollTo = vi.fn();
  document.body.append(host);
  await tasks.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [], columns: columns() });
});
afterEach(() => { page?.dispose(); host.remove(); delayed.review = null; delayed.commits = null; });

async function seed({ summary = false, review = true } = {}) {
  const detail = { task: task({ id: scope.taskId, title: "Review the saved changes", ...(summary ? { review_summary: { status: "open" } } : {}) }),
    timeline: [comment({ id: "anchored-comment", body: "Keep this line.", anchor })] };
  await tasks.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, detail);
  if (!review) return;
  await reviews.writeReviewRecord(scope, saved, 1);
  await support.rememberReviewSupport(scope.deviceId, { reviews: { get: true, diff: true, commits: true, commit: true } });
  const identity = { ...scope, snapshot: saved.snapshots[0], directory: saved.snapshots[0].directories[0] };
  await cache.writeCached(reads.reviewCommitsAddress(identity), { commits: [{ oid: commit, subject: "Saved change", author_name: "Ada" }] });
  for (const comparison of [{}, { mode: "commit", commit }]) {
    await cache.writeCached(reads.reviewDiffAddress(identity, comparison), { files: [{ path, content_key: "saved", status: "M" }] });
    await cache.writeCached(reads.reviewDiffAddress(identity, comparison, path), {
      content_key: "saved", truncated: false,
      patch: `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -0,0 +1 @@\n+const saved = true;\n`,
    });
  }
}

function mount(options = {}) {
  const navigate = vi.fn();
  page = mountTaskPage(host, { ...scope, projectKey: `${scope.deviceId}|${scope.projectId}`,
    feed: () => ({ items: [], workspaces: [] }), catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }), callRpc: () => new Promise(() => {}), navigate, ...options,
  });
  return navigate;
}

describe("cached review task template", () => {
  it.each([true, false])("mounts the review template with summary=%s and no wire replies", async (summary) => {
    await seed({ summary });
    mount();
    await vi.waitFor(() => expect(host.querySelector(".review-task-template")).toBeTruthy());
    await vi.waitFor(() => expect(host.querySelector("[data-review-commit]")?.textContent).toContain("Saved change"));
    expect(host.querySelector("[data-task-composer]")).toBeNull();
    expect(host.querySelector(".task-review-comments #comment-anchored-comment")).toBeTruthy();
    expect(host.querySelector("[data-review-task-details] .task-rail")).toBeTruthy();
  });

  it("keeps an ordinary task's body and composer when no review is cached", async () => {
    await seed({ review: false });
    mount();
    await vi.waitFor(() => expect(host.querySelector("[data-task-composer]")).toBeTruthy());
    expect(host.querySelector(".review-task-template")).toBeNull();
    expect(host.querySelector(".task-page-head h1").textContent).toBe("Review the saved changes");
  });

  it("opens /c anchors from cached review metadata when a summary chooses the template", async () => {
    await seed({ summary: true });
    const navigate = mount({ commentId: "anchored-comment" });
    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ review: expect.objectContaining({
      revision: 1, mode: "commit", commit, path, side: "new", line: 1,
    }) }), expect.anything()));
    await vi.waitFor(() => expect(host.querySelector(".task-review-anchor")).toBeTruthy());
  });

  it("opens /c anchors when the task paints before its cached review metadata", async () => {
    await seed();
    let release;
    delayed.review = new Promise((resolve) => { release = resolve; });
    const navigate = mount({ commentId: "anchored-comment" });
    await vi.waitFor(() => expect(host.querySelector("[data-task-composer]")).toBeTruthy());
    release();
    await vi.waitFor(() => expect(host.querySelector(".review-task-template")).toBeTruthy());
    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ review: expect.objectContaining({
      mode: "commit", commit, path, line: 1,
    }) }), expect.anything()));
  });

  it("cannot navigate from a disposed page when a clicked anchor's cache read finishes", async () => {
    await seed({ summary: true });
    const navigate = mount();
    await vi.waitFor(() => expect(host.querySelector("[data-review-commit]")).toBeTruthy());
    navigate.mockClear();
    let release;
    delayed.commits = new Promise((resolve) => { release = resolve; });
    const completed = delayed.commitReads;
    host.querySelector("[data-review-anchor]").click();
    page.dispose();
    release();
    await vi.waitFor(() => expect(delayed.commitReads).toBeGreaterThan(completed));
    expect(navigate).not.toHaveBeenCalled();
  });
});
