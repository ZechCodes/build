// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange } from "fake-indexeddb";
import getFixture from "../../fixtures/api/v1/tasks.review.get.json";
import workspaceFixture from "../../fixtures/api/v1/workspace.list.json";

const state = vi.hoisted(() => ({ context: null, watchers: [] }));
vi.mock("../src/appState.js", () => ({ App: { route: { name: "inbox" }, devices: [] } }));
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

let cache, sync, reviewCache, support, rpc, answer, workspaces, tasks, items, feed;
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

afterEach(() => { feed?.stopFeed(); feed = null; sync.stopCacheSync(); });

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

it("clears a retained workspace PR link when an authoritative board push omits it", async () => {
  const row = { id: "workspace-1", project_id: projectId, active_review: { ...summary, version: 7 } };
  workspaces = [row];
  await sync.syncDevice(deviceId);
  await pushed([{ entity_id: "board", state: { workspaces: [{ id: row.id, project_id: projectId }] } }]);
  const held = (await cache.readCached({ deviceId, entityId: "", kind: "workspaces" })).value[0];
  expect(held).not.toHaveProperty("active_review");
  expect(await reviewCache.readWorkspaceReview({ ...scope, workspaceId: row.id })).toBeNull();
});

it.each([true, false])("checks the sync lifetime when a queued workspace push commits (stopped: %s)", async (stopped) => {
  workspaces = [{ id: "workspace-1", project_id: projectId, active_review: { ...summary, version: 7 } }];
  await sync.syncDevice(deviceId);
  const address = { deviceId, entityId: "", kind: "workspaces" };
  const before = await cache.readCached(address);
  const queued = Promise.withResolvers();
  const original = IDBDatabase.prototype.transaction;
  let release;
  const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    const opened = original.apply(this, args);
    if (!release && this.name === "build-cache" && opened.mode === "readwrite") {
      const start = opened._start;
      release = () => { opened._start = start; start.call(opened); };
      opened._start = () => { queued.resolve(); };
    }
    return opened;
  });
  const heard = vi.fn();
  const stopListening = cache.subscribeCache(address, heard);
  try {
    const pending = pushed([{ entity_id: "board", state: { workspaces: [{ id: "workspace-1", project_id: projectId }] } }]);
    await queued.promise;
    if (stopped) sync.stopCacheSync();
    release();
    await pending;
    const after = await cache.readCached(address);
    if (stopped) {
      expect(after).toEqual(before);
      expect(heard).not.toHaveBeenCalled();
    } else {
      expect(after.value[0]).not.toHaveProperty("active_review");
      expect(heard).toHaveBeenCalledTimes(1);
    }
  } finally { transaction.mockRestore(); stopListening(); }
});

it("discovers an unopened review from a newly watched task state", async () => {
  await sync.syncDevice(deviceId);
  await pushed([{ entity_id: taskId, state: { kind: "tracker_task", task_id: taskId, project_id: projectId, review_summary: summary } }]);
  expect((await heldReview()).review.task_id).toBe(taskId);
  expect(reviewCalls()).toHaveLength(1);
});

it("keeps a newer pushed row review summary when a late state reply names an older version", async () => {
  await sync.syncDevice(deviceId);
  const row = { kind: "tracker_task", task_id: taskId, project_id: projectId, review_summary: { ...summary, version: 8, status: "merged" } };
  await pushed([{ entity_id: taskId, state: row }]);
  await pushed([{ entity_id: taskId, state: { ...row, review_summary: summary } }]);
  const held = (await cache.readCached({ deviceId, entityId: taskId, kind: "row" })).value;
  expect(held.review_summary).toEqual(row.review_summary);
});

it.each(["individual row", "feed copy"])("persists and paints the PR version from a protected %s before stopped sync can repair rows", async (source) => {
  await sync.syncDevice(deviceId);
  const feedAddress = { deviceId, entityId: "", kind: "feed" };
  const rowAddress = { deviceId, entityId: taskId, kind: "row" };
  const held = { kind: "tracker_task", task_id: taskId, project_id: projectId, title: "held fields",
    review_summary: { ...summary, version: 8, status: "merged" } };
  if (source === "individual row") await cache.writeCached(rowAddress, held);
  else {
    await cache.writeCached(rowAddress, null);
    await cache.writeCached(feedAddress, { items: [held], projects: [], workspaces: [] });
  }
  const ownBefore = await cache.readCached(rowAddress);
  items = [{ ...held, title: "board fields", review_summary: summary }];
  const stopAfterFeed = cache.subscribeCache(feedAddress, () => { state.context.session = {}; });
  try { expect(await sync.syncDevice(deviceId)).toBe(false); }
  finally { stopAfterFeed(); }
  expect(await cache.readCached(rowAddress)).toEqual(ownBefore);
  const cached = (await cache.readCached(feedAddress)).value.items[0];
  expect(cached.review_summary).toEqual(held.review_summary);
  expect(cached.title).toBe("board fields");

  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
  feed = await import("../src/core/taskFeed.js");
  let painted;
  const stopPaint = feed.subscribeFeed((snapshot) => { painted = snapshot; });
  try {
    await feed.startFeed();
    expect(painted.items.find((row) => row.task_id === taskId)).toMatchObject({
      title: "board fields", review_summary: held.review_summary,
    });
  } finally { stopPaint(); feed.stopFeed(); }
});

it.each(["individual row", "feed copy"])("retains a same-version fallback observation from the %s over an older cached clear", async (source) => {
  await sync.syncDevice(deviceId);
  const tracker = await import("../src/core/trackerCache.js");
  const seed = { kind: "tracker_task", task_id: taskId, project_id: projectId, title: "held fields",
    updated_at: "2026-10-08T20:00:20Z", review_summary: summary };
  const { review_summary: _summary, ...absent } = seed;
  const clear = tracker.preserveTaskReviewSummary(seed, { ...absent, updated_at: "2026-10-08T20:00:30Z" });
  const visible = tracker.preserveTaskReviewSummary(null, {
    ...seed, updated_at: "2026-10-08T20:00:40Z", review_summary: { ...summary, version: 8 },
  });
  const delayed = tracker.preserveTaskReviewSummary(visible, {
    ...seed, updated_at: "2026-10-08T20:00:10Z", review_summary: { ...summary, version: 8 },
  });
  const rowAddress = { deviceId, entityId: taskId, kind: "row" };
  const feedAddress = { deviceId, entityId: "", kind: "feed" };
  await cache.writeCached(rowAddress, source === "individual row" ? delayed : clear);
  await cache.writeCached(feedAddress, { items: [source === "feed copy" ? delayed : clear], projects: [], workspaces: [] });
  const ownBefore = await cache.readCached(rowAddress);
  items = [{ ...seed, title: "board fields", updated_at: "2026-10-08T20:00:10Z" }];
  const stopAfterFeed = cache.subscribeCache(feedAddress, () => { state.context.session = {}; });
  try { expect(await sync.syncDevice(deviceId)).toBe(false); }
  finally { stopAfterFeed(); }
  expect(await cache.readCached(rowAddress)).toEqual(ownBefore);
  const cached = (await cache.readCached(feedAddress)).value.items[0];
  expect(cached).toMatchObject({ title: "board fields", review_summary: { ...summary, version: 8 } });

  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
  feed = await import("../src/core/taskFeed.js");
  let painted;
  const stopPaint = feed.subscribeFeed((snapshot) => { painted = snapshot; });
  try {
    await feed.startFeed();
    expect(painted.items.find((row) => row.task_id === taskId)).toMatchObject({
      title: "board fields", review_summary: { ...summary, version: 8 },
    });
  } finally { stopPaint(); feed.stopFeed(); }
});

it.each([
  { source: "individual row", version: 7, legacy: false, rejectedRefresh: false },
  { source: "feed copy", version: 7, legacy: false, rejectedRefresh: false },
  { source: "individual row", version: 8, legacy: false, rejectedRefresh: false },
  { source: "feed copy", version: 8, legacy: false, rejectedRefresh: false },
  { source: "individual row", version: 8, legacy: true, rejectedRefresh: false },
  { source: "feed copy", version: 8, legacy: true, rejectedRefresh: false },
  { source: "individual row", version: 8, legacy: false, rejectedRefresh: true },
  { source: "feed copy", version: 8, legacy: false, rejectedRefresh: true },
])("preserves a clear from the $source over an earlier cached v$version copy (legacy: $legacy, rejected refresh: $rejectedRefresh)", async ({ source, version, legacy, rejectedRefresh }) => {
  await sync.syncDevice(deviceId);
  const tracker = await import("../src/core/trackerCache.js");
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  const visible = { kind: "tracker_task", task_id: taskId, project_id: projectId, title: "held fields",
    updated_at: "2026-10-08T20:00:20Z", review_summary: { ...summary, version: 7 } };
  const oldRow = {
    ...visible, updated_at: "2026-10-08T20:00:10Z", review_summary: { ...summary, version },
  };
  let earlier = legacy ? oldRow : tracker.preserveTaskReviewSummary(null, oldRow, { readOrder: await nextTaskRead() });
  await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord([visible], [], await nextTaskRead()));
  const { review_summary: _summary, ...absent } = visible;
  await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord([
    { ...absent, updated_at: "2026-10-08T20:00:30Z" },
  ], [], await nextTaskRead()));
  const cleared = (await tracker.readTasksRecord(deviceId, projectId)).tasks[0];
  if (rejectedRefresh) earlier = tracker.preserveTaskReviewSummary(earlier, {
    ...oldRow, updated_at: "2026-10-08T20:00:05Z", review_summary: summary,
  }, { readOrder: await nextTaskRead() });
  const rowAddress = { deviceId, entityId: taskId, kind: "row" };
  const feedAddress = { deviceId, entityId: "", kind: "feed" };
  await cache.writeCached(rowAddress, source === "individual row" ? cleared : earlier);
  await cache.writeCached(feedAddress, { items: [source === "feed copy" ? cleared : earlier], projects: [], workspaces: [] });
  const ownBefore = await cache.readCached(rowAddress);
  items = [{ ...visible, title: "board fields", updated_at: "2026-10-08T20:00:10Z", review_summary: summary }];
  const stopAfterFeed = cache.subscribeCache(feedAddress, () => { state.context.session = {}; });
  try { expect(await sync.syncDevice(deviceId)).toBe(false); }
  finally { stopAfterFeed(); }
  expect(await cache.readCached(rowAddress)).toEqual(ownBefore);
  const cached = (await cache.readCached(feedAddress)).value.items[0];
  expect(cached).not.toHaveProperty("review_summary");
  expect(cached.title).toBe("board fields");

  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
  feed = await import("../src/core/taskFeed.js");
  let painted;
  const stopPaint = feed.subscribeFeed((snapshot) => { painted = snapshot; });
  try {
    await feed.startFeed();
    const row = painted.items.find((candidate) => candidate.task_id === taskId);
    expect(row).not.toHaveProperty("review_summary");
    expect(row.title).toBe("board fields");
  } finally { stopPaint(); feed.stopFeed(); }
});

it("admits a fresh higher board version after merging two cached authorities with unknown read order", async () => {
  await sync.syncDevice(deviceId);
  const tracker = await import("../src/core/trackerCache.js");
  const visible = { kind: "tracker_task", task_id: taskId, project_id: projectId,
    updated_at: "2026-10-08T20:00:20Z", review_summary: { ...summary, version: 7 } };
  const { review_summary: _summary, ...absent } = visible;
  const cleared = tracker.preserveTaskReviewSummary(visible, { ...absent, updated_at: "2026-10-08T20:00:30Z" });
  const older = tracker.preserveTaskReviewSummary(null, {
    ...visible, updated_at: "2026-10-08T20:00:10Z", review_summary: summary,
  });
  const feedAddress = { deviceId, entityId: "", kind: "feed" };
  const rowAddress = { deviceId, entityId: taskId, kind: "row" };
  await cache.writeCached(rowAddress, cleared);
  await cache.writeCached(feedAddress, { items: [older], projects: [], workspaces: [] });
  const ownBefore = await cache.readCached(rowAddress);
  items = [{ ...visible, updated_at: "2026-10-08T20:00:10Z", review_summary: { ...summary, version: 8 } }];
  const stopAfterFeed = cache.subscribeCache(feedAddress, () => { state.context.session = {}; });
  try {
    expect(await sync.syncDevice(deviceId)).toBe(false);
    expect((await cache.readCached(feedAddress)).value.items[0]).toHaveProperty("review_summary.version", 8);
    expect(await cache.readCached(rowAddress)).toEqual(ownBefore);
    items = [{ ...items[0], review_summary: summary }];
    expect(await sync.syncDevice(deviceId)).toBe(false);
  } finally { stopAfterFeed(); }
  expect((await cache.readCached(feedAddress)).value.items[0]).toHaveProperty("review_summary.version", 8);
  expect(await cache.readCached(rowAddress)).toEqual(ownBefore);

  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
  feed = await import("../src/core/taskFeed.js");
  let painted;
  const stopPaint = feed.subscribeFeed((snapshot) => { painted = snapshot; });
  try {
    await feed.startFeed();
    expect(painted.items.find((candidate) => candidate.task_id === taskId)).toHaveProperty("review_summary.version", 8);
  } finally { stopPaint(); feed.stopFeed(); }
});
