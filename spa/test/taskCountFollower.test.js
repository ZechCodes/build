import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, support, counts, followerModule, projectModule;
const scope = { deviceId: "device-counts", projectId: "project-counts" };
const answer = (total = 42, agents = []) => ({ project_id: scope.projectId, unfinished_open_count: 17,
  watched_unread_total: total, agents, basis: "opaque-basis" });
const put = (result, agentIds = []) => cache.writeCached(counts.taskCountsAddress(scope, agentIds), { generation: 1, result });
const enable = () => support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: true } });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  support = await import("../src/core/taskCountsSupport.js");
  counts = await import("../src/core/taskAggregateReads.js");
  followerModule = await import("../src/core/taskCountFollower.js");
  projectModule = await import("../src/core/projectTasksUnread.js");
});

it("persists monotonic rendering support while preserving current counts authority", async () => {
  expect(await support.readTaskCountsSupport(scope.deviceId)).toBe(false);
  await enable();
  await support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: false } });
  expect(await support.readTaskCountsSupport(scope.deviceId)).toBe(true);
  expect((await cache.readCached(support.taskCountsSupportAddress(scope.deviceId))).value).toEqual({ counts: false, everCounts: true });
  vi.resetModules();
  const restarted = await import("../src/core/taskCountsSupport.js");
  expect(await restarted.readTaskCountsSupport(scope.deviceId)).toBe(true);
});

it("checks captured greeting authority inside the support transaction", async () => {
  await support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: false } });
  let current = true;
  const pending = support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: true } }, () => current);
  current = false;
  await pending;
  expect(await support.readTaskCountsSupport(scope.deviceId)).toBe(false);
  await enable();
  await support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: false } }, () => false);
  expect((await cache.readCached(support.taskCountsSupportAddress(scope.deviceId))).value).toEqual({ counts: true, everCounts: true });
});

it("follows exact cached counts and support without any transport", async () => {
  await enable();
  await put(answer());
  const changed = vi.fn();
  const follower = followerModule.createTaskCountFollower({ scope, onChange: changed });
  try {
    await vi.waitFor(() => expect(follower.read()?.watched_unread_total).toBe(42));
    expect(follower.supported()).toBe(true);
    await put(answer(43));
    await vi.waitFor(() => expect(follower.read()?.watched_unread_total).toBe(43));
    expect(changed).toHaveBeenCalled();
  } finally { follower.dispose(); }
});

it("retargets only explicit exact agent selections and stops notifications on disposal", async () => {
  await enable();
  await put(answer(42));
  await put(answer(50, [{ agent_id: "A", unfinished_open_count: 2, watched_unread_total: 3 }]), ["A"]);
  const changed = vi.fn();
  const follower = followerModule.createTaskCountFollower({ scope, onChange: changed });
  await vi.waitFor(() => expect(follower.read()?.watched_unread_total).toBe(42));
  follower.follow(["A", "A"]);
  await vi.waitFor(() => expect(follower.read()?.watched_unread_total).toBe(50));
  expect(follower.read().agents[0].agent_id).toBe("A");
  expect(() => follower.follow(Array(101).fill("A"))).toThrow();
  follower.dispose();
  changed.mockClear();
  await put(answer(60), ["A"]);
  expect(changed).not.toHaveBeenCalled();
});

it("uses authoritative project unread rather than loaded task rows and retains totals on refusal or downgrade", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([
    { id: "loaded", state: "open", status: "in_progress", watched: true, unread_count: 2 },
  ]));
  const badge = projectModule.followProjectTasksUnread();
  badge.follow(scope.deviceId, scope.projectId);
  try {
    await vi.waitFor(() => expect(badge.count()).toBe(2));
    await enable();
    await put(answer(420));
    await vi.waitFor(() => expect(badge.count()).toBe(420));
    await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([]));
    expect(badge.count()).toBe(420);
    const reader = counts.createTaskAggregateReader({ scope, capabilities: () => ({ counts: true }),
      sessionOf: () => "session", callRpc: async () => { throw Object.assign(new Error("unsupported markers"), { retryable: false }); } });
    await expect(reader.refresh()).rejects.toThrow("unsupported markers");
    expect(badge.count()).toBe(420);
    await support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: false } });
    await cache.deleteCached([counts.taskCountsAddress(scope)]);
    await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([
      { id: "other-loaded", state: "open", watched: true, unread_count: 99 },
    ]));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(badge.count()).toBe(420);
    reader.dispose();
  } finally { badge.dispose(); }
});

it("keeps unsupported project badges on their existing cache-only path", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  await put(answer(420));
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([
    { id: "legacy", state: "open", watched: true, unread_count: 7 },
  ]));
  const badge = projectModule.followProjectTasksUnread();
  badge.follow(scope.deviceId, scope.projectId);
  try { await vi.waitFor(() => expect(badge.count()).toBe(7)); } finally { badge.dispose(); }
});
