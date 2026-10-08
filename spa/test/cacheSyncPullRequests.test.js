// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import getFixture from "../../fixtures/api/v1/tasks.review.get.json";
import workspaceFixture from "../../fixtures/api/v1/workspace.list.json";

const state = vi.hoisted(() => ({ context: null, watchers: [] }));
vi.mock("../src/appState.js", () => ({ App: { route: { name: "inbox" } } }));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => state.context,
  // Tests start the cache lock, then await exactly one explicit sync pass.
  liveContexts: () => [],
  onDeviceStateChanged: () => () => {},
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "git", "tasks"] } }),
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
  watchChanges: (watcher) => { state.watchers.push(watcher); return { dispose() {} }; },
}));

let cache, sync, reviewCache, support, rpc, answer, workspaces, tasks, items;
const deviceId = "pr-sync";
const projectId = "proj-1";
const taskId = "task-1";
const summary = { task_id: taskId, workspace_id: "workspace-1", version: 3, status: "open", latest_published_snapshot_id: "snapshot-1" };
const scope = { deviceId, projectId, taskId };
const reviewCalls = () => rpc.mock.calls.filter(([method]) => method.startsWith("tasks.review."));
const heldReview = async () => (await cache.readCached(reviewCache.reviewAddress(scope)))?.value;
const pushed = (changes) => state.watchers.find(({ id }) => id === "s-inbox").onChanges(changes);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  state.watchers = [];
  workspaces = [];
  tasks = [];
  items = [];
  answer = structuredClone(getFixture.examples.find(({ result }) => result.review?.mode === "pull_request").result);
  rpc = vi.fn(async (method) => {
    const replies = {
      "board.list": { items }, "project.list": { projects: [{ project_id: projectId }] },
      "workspace.list": { workspaces }, "tasks.list": { tasks },
      "tasks.columns": { columns: [{ id: "in_review" }] }, "tasks.review.get": answer,
    };
    return replies[method] || {};
  });
  state.context = { deviceId, rpc, session: {}, greeted: Promise.resolve(), cacheScope: {}, active: () => true };
  cache = await import("../src/core/localCache.js");
  support = await import("../src/core/taskReviewSupport.js");
  reviewCache = await import("../src/core/taskReviewCache.js");
  sync = await import("../src/core/cacheSync.js");
  await support.rememberReviewSupport(deviceId, { reviews: { get: true, pullRequests: true } });
  sync.startCacheSync();
});

afterEach(() => sync.stopCacheSync());

it("discovers an unopened PR from task summaries and refreshes only metadata on reconnect", async () => {
  tasks = [{ id: taskId, number: 1, project_id: projectId, review_summary: summary }];
  expect(await sync.syncDevice(deviceId)).toBe(true);
  expect((await heldReview()).review).toEqual(answer.review);
  expect((await heldReview()).sync).toEqual(answer.sync);
  state.context.session = {};
  expect(await sync.syncDevice(deviceId)).toBe(true);
  expect(reviewCalls().map(([method]) => method)).toEqual(["tasks.review.get", "tasks.review.get"]);
});

it("warms a workspace's PR even when it is absent from tasks and the feed", async () => {
  const row = structuredClone(workspaceFixture.examples.find(({ result }) => result.workspaces[0].active_review).result.workspaces[0]);
  workspaces = [{ ...row, id: "workspace-1", workspace_id: "workspace-1", active_review: summary }];
  expect(await sync.syncDevice(deviceId)).toBe(true);
  expect((await heldReview()).review.task_id).toBe(taskId);
  expect(reviewCalls()).toHaveLength(1);
  expect((await cache.readCached({ deviceId, entityId: "", kind: "workspaces" })).value[0].active_review).toEqual(summary);
});

it("discovers a watched-task PR from the feed without a held task list row", async () => {
  items = [{ kind: "tracker_task", task_id: taskId, project_id: projectId, review_summary: summary }];
  expect(await sync.syncDevice(deviceId)).toBe(true);
  expect((await heldReview()).review.task_id).toBe(taskId);
  expect(reviewCalls()).toHaveLength(1);
});

it("does not discover new PR metadata on a bridge without the exact feature", async () => {
  await support.rememberReviewSupport(deviceId, { reviews: { get: true } });
  tasks = [{ id: taskId, project_id: projectId, review_summary: summary }];
  expect(await sync.syncDevice(deviceId)).toBe(true);
  expect(reviewCalls()).toEqual([]);
  expect(await heldReview()).toBeUndefined();
});

it("reads newly created PR summaries after task invalidation and never probes ordinary tasks", async () => {
  await sync.syncDevice(deviceId);
  tasks = [{ id: taskId, project_id: projectId, review_summary: summary }, { id: "ordinary-task", project_id: projectId }];
  await pushed([{ entity_id: projectId, tasks: { task_ids: [taskId, "ordinary-task"] } }]);
  expect((await heldReview()).review.task_id).toBe(taskId);
  expect(reviewCalls().map(([, params]) => params.task_id)).toEqual([taskId]);
});

it("updates per-directory observations after Git invalidation without replaying Push", async () => {
  workspaces = [{ id: "workspace-1", workspace_id: "workspace-1", entity_id: "run-1", project_id: projectId, active_review: summary }];
  items = [{ kind: "branch", run_id: "run-1", project_id: projectId, workspace_id: "workspace-1", state: "building", agents: [] }];
  await sync.syncDevice(deviceId);
  answer = { ...answer, sync: answer.sync.map((fact) => ({ ...fact, revision: 8, pending_commits: 2 })) };
  await pushed([{ entity_id: "run-1", git: { status: { files: [] } } }]);
  expect((await heldReview()).sync[0]).toMatchObject({ revision: 8, pending_commits: 2 });
  expect(reviewCalls().map(([method]) => method)).toEqual(["tasks.review.get", "tasks.review.get"]);
});

it("warms newly linked workspace metadata from a pushed board list", async () => {
  await sync.syncDevice(deviceId);
  await pushed([{ entity_id: "board", state: { workspaces: [{ id: "workspace-1", project_id: projectId, active_review: summary }] } }]);
  expect((await heldReview()).review.task_id).toBe(taskId);
  expect(reviewCalls()).toHaveLength(1);
});

it("discovers an unopened review from a newly watched task state", async () => {
  await sync.syncDevice(deviceId);
  await pushed([{ entity_id: taskId, state: { kind: "tracker_task", task_id: taskId, project_id: projectId, review_summary: summary } }]);
  expect((await heldReview()).review.task_id).toBe(taskId);
  expect(reviewCalls()).toHaveLength(1);
});

it("keeps a newer feed review summary when a late state reply names an older version", async () => {
  await sync.syncDevice(deviceId);
  const row = { kind: "tracker_task", task_id: taskId, project_id: projectId, review_summary: { ...summary, version: 8, status: "merged" } };
  await pushed([{ entity_id: taskId, state: row }]);
  await pushed([{ entity_id: taskId, state: { ...row, review_summary: summary } }]);
  const held = (await cache.readCached({ deviceId, entityId: taskId, kind: "row" })).value;
  expect(held.review_summary).toEqual(row.review_summary);
});
