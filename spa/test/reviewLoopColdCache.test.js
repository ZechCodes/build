/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => true, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/notify.js", () => ({ notifyError() {} }));

const deviceId = "cold-review-rows";
const project = { id: "project", deviceId, projectKey: `${deviceId}|project`, name: "Build" };
let cache, tracker, contexts, panes, follower;
const address = (kind, entityId = "", sub) => ({ deviceId, entityId, kind, ...(sub ? { sub } : {}) });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="list"></div><div id="board"></div>';
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  contexts = await import("../src/core/deviceContexts.js");
  panes = [];
});
afterEach(() => {
  panes.forEach((pane) => pane.dispose());
  follower?.dispose();
  contexts.resetDeviceContexts();
  vi.restoreAllMocks();
});

it("paints Legacy chips from compact summaries on a cold cache without review probes", async () => {
  await cache.writeCached(address("task-review-support"), { reject: true, snapshotRetired: true, get: true, verdictDelivery: true });
  await cache.writeCached(address("needs-you-rule"), { askedOnly: true });
  const ordinary = task({ id: "ordinary", project_id: project.id, legacy_review: true, watched: true, assignee: { kind: "user" } });
  await tracker.writeTasksRecord(deviceId, project.id, tracker.tasksRecord([ordinary], columns()));
  const repository = await import("../src/core/taskReviewCache.js");
  const createReviewRepository = vi.spyOn(repository, "createTaskReviewRepository");
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  const rpc = vi.fn(() => Promise.reject(new Error("Offline")));
  for (const view of ["list", "board"]) {
    panes.push(mountTasksPane(document.querySelector(`#${view}`), {
      deviceId, projectId: project.id, projectKey: project.projectKey, projectName: project.name,
      defaultView: view, callRpc: rpc, catalog: () => ({ providers: [] }),
      feed: () => ({ projects: [project], items: [], workspaces: [] }), navigate: vi.fn(),
    }));
  }
  const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
  const { inboxRowHtml } = await import("../src/core/inbox.js");
  follower = followWatchedTasks({ onChange() {} });
  follower.follow([project]);
  await vi.waitFor(() => {
    expect(document.querySelector('#list .task-row[data-task="ordinary"]')).not.toBeNull();
    expect(document.querySelector('#board .task-card[data-task="ordinary"]')).not.toBeNull();
    expect(follower.entries()).toHaveLength(1);
  });
  expect(document.querySelectorAll(".task-review-summary")).toHaveLength(2);
  expect(inboxRowHtml(follower.entries()[0])).toContain("Legacy review");
  expect(rpc.mock.calls.filter(([method]) => method === "tasks.review.get")).toEqual([]);
  expect(createReviewRepository).not.toHaveBeenCalled();
  expect(contexts.contextFor(deviceId)).toBeNull();

  // Snapshot mode is omitted by the bridge serializer. A cache publication,
  // including sync in another tab, supplies the fact without opening the task.
  await cache.writeCached(address("task-review", project.id, ordinary.id), {
    review: { task_id: ordinary.id, state: "open", snapshots: [] },
  });
  await vi.waitFor(() => {
    expect(document.querySelector("#list .task-review-summary")?.textContent).toBe("Legacy review");
    expect(document.querySelector("#board .task-review-summary")?.textContent).toBe("Legacy review");
    expect(inboxRowHtml(follower.entries()[0])).toContain("Legacy review");
  });
  expect(rpc.mock.calls.filter(([method]) => method === "tasks.review.get")).toEqual([]);
  expect(createReviewRepository).not.toHaveBeenCalled();
});
