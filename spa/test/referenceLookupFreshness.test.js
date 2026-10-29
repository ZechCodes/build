import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
let cache, aggregate, tracker, order, index, source, reader, stop, release;
const scope = { deviceId: "fresh-ref", projectId: "project-ref" }, projectKey = "fresh-ref/project-ref";
const target = { id: "target", project_id: scope.projectId, number: 42, title: "Title" };
const result = (task = null) => ({ project_id: scope.projectId, number: 42, task, identities: {}, basis: "basis" });
const resolve = () => index.referenceResolver({ place: scope }).task(42);
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; };
beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); aggregate = await import("../src/core/taskAggregateReads.js");
  tracker = await import("../src/core/trackerCache.js"); order = await import("../src/core/taskReadOrder.js");
  index = await import("../src/core/referenceIndex.js"); source = await import("../src/core/referenceIndexFeed.js");
});
afterEach(() => { reader?.dispose(); reader = null; release?.(); release = null; stop?.(); stop = null; });
const start = async (rpc) => {
  stop = source.feedReferenceIndex({ subscribeFeed(listener) { listener({ projects: [{ id: scope.projectId, deviceId: scope.deviceId, projectKey }], items: [], workspaces: [] }); return () => {}; } });
  release = (await import("../src/core/taskReferenceDemand.js")).admitTaskReferenceLookups(scope, [42]);
  reader = aggregate.createTaskLookupReader({ scope, callRpc: rpc, sessionOf: () => "session", capabilities: () => ({ lookup: true }) });
};
const listed = async (title, number = 42) => (await import("../src/core/taskWaitingOnCache.js")).cacheTaskWaitingOnResponse(
  scope.deviceId, scope.projectId, { task: { ...target, id: `task-${number}`, number, title, state: "open", status: "backlog" } });

it("does not let an older point miss or a failed refresh erase a newer created/listed positive", async () => {
  const pending = deferred(), rpc = vi.fn().mockResolvedValueOnce(result()).mockImplementationOnce(() => pending.promise);
  await start(rpc); await reader.refresh(42); await vi.waitFor(() => expect(resolve()).toBeNull());
  const address = aggregate.taskLookupAddress(scope, 42);
  const published = (await cache.readCached(address)).value.published_result_read_order;
  expect(Number.isSafeInteger(published)).toBe(true);
  await listed("Newly created"); await vi.waitFor(() => expect(resolve()?.title).toBe("Newly created"));
  const refresh = reader.refresh(42); void refresh.catch(() => {});
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  expect((await cache.readCached(address)).value.published_result_read_order).toBe(published);
  expect(resolve()?.title).toBe("Newly created");
  pending.reject(new Error("unavailable")); await expect(refresh).rejects.toThrow("unavailable");
  expect((await cache.readCached(address)).value.published_result_read_order).toBe(published);
  expect(resolve()?.title).toBe("Newly created");
});

it("prefers a newer listed title over an old found point, then accepts a proven newer authoritative miss", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(result({ ...target, title: "Old point" })).mockResolvedValueOnce(result());
  await start(rpc); await reader.refresh(42); await vi.waitFor(() => expect(resolve()?.title).toBe("Old point"));
  await listed("New list title"); await vi.waitFor(() => expect(resolve()?.title).toBe("New list title"));
  await reader.refresh(42); await vi.waitFor(() => expect(resolve()).toBeNull());
});

it("preserves a known positive when point or positive clocks are unknown", () => {
  index.holdReferenceSources({ tasks: { [projectKey]: [target] }, taskLookups: { [projectKey]: { 42: { status: "missing" } } } });
  expect(resolve()?.taskId).toBe(target.id);
});

it("does not promote an older row's authority when an unrelated row action raises the list clock", async () => {
  await listed("Old #42");
  const rpc = vi.fn().mockResolvedValue(result());
  await start(rpc); await reader.refresh(42);
  await vi.waitFor(() => expect(resolve()).toBeNull());
  await listed("New #99", 99);
  await vi.waitFor(async () => expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks).toHaveLength(2));
  expect(resolve()).toBeNull();
});

it("does not infer per-row authority from a no-note list maximum", async () => {
  const rpc = vi.fn().mockResolvedValue(result());
  await start(rpc); await reader.refresh(42);
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([{ ...target, title: "Unproved positive" }], [], 1));
  await vi.waitFor(() => expect(resolve()?.title).toBe("Unproved positive"));
});

it("keeps a stronger same-task point title when a newer page read retains an older legacy body", async () => {
  const actions = await import("../src/core/taskWaitingOnCache.js");
  const pages = await import("../src/core/trackerPages.js");
  const row = (hour, title, id = target.id) => ({ ...target, id, title, state: "open", status: "backlog",
    updated_at: `2026-10-29T${hour}:00:00Z` });
  await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: row("13", "Retained T13") });
  await start(vi.fn().mockResolvedValue(result(row("14", "Stronger T14"))));
  await reader.refresh(42);
  await vi.waitFor(() => expect(resolve()?.title).toBe("Stronger T14"));
  const before = index.referenceIndexVersion(), read = await order.nextTaskRead();
  await pages.foldTasksPage(tracker.tasksAddress(scope.deviceId, scope.projectId), {
    tasks: [row("12", "Incoming T12")], above: Infinity, through: -Infinity, read, pullRead: read,
    readAt: Date.parse("2026-10-29T12:00:00Z"),
  }, () => []);
  expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0].title).toBe("Retained T13");
  await vi.waitFor(() => expect(index.referenceIndexVersion()).toBeGreaterThan(before));
  expect(resolve()?.title).toBe("Stronger T14");
});

it("does not hide a cacheNewTask-shaped new positive behind a stale miss and inherited old spans", async () => {
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId), note = order.readsAddress(address);
  const prior = await order.nextTaskRead();
  await cache.mergeCachedRecordsTogether([address, note], () => [tracker.tasksRecord([], [], prior), {
    stretches: [{ above: Infinity, through: -Infinity, read: prior }], rows: {},
  }]);
  await start(vi.fn().mockResolvedValue(result())); await reader.refresh(42);
  await vi.waitFor(() => expect(resolve()).toBeNull());
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([{ ...target, title: "New unnoted positive" }], [], await order.nextTaskRead()));
  await vi.waitFor(() => expect(resolve()?.title).toBe("New unnoted positive"));
});

it("keeps stronger same-ID listed content but does not transfer its timestamp authority across IDs", async () => {
  const actions = await import("../src/core/taskWaitingOnCache.js");
  await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: { ...target,
    title: "Future same identity", updated_at: "2026-10-29T15:00:00Z" } });
  await start(vi.fn().mockResolvedValueOnce(result({ ...target, title: "Older body newer membership", updated_at: "2026-10-29T14:00:00Z" }))
    .mockResolvedValueOnce(result({ ...target, id: "different-task", title: "New logical identity", updated_at: "2026-10-29T13:00:00Z" })));
  await vi.waitFor(() => expect(resolve()?.title).toBe("Future same identity"));
  const before = index.referenceIndexVersion();
  await reader.refresh(42);
  await vi.waitFor(() => expect(index.referenceIndexVersion()).toBeGreaterThan(before));
  expect(resolve()?.title).toBe("Future same identity");
  await reader.refresh(42);
  await vi.waitFor(() => expect(resolve()?.taskId).toBe("different-task"));
  expect(resolve()?.title).toBe("New logical identity");
});

it("does not resurrect a stale miss by falling back behind a later span after an unrelated rewrite", async () => {
  const actions = await import("../src/core/taskWaitingOnCache.js");
  const pages = await import("../src/core/trackerPages.js");
  const row = { ...target, title: "Page-positive", state: "open", status: "backlog", updated_at: "2026-10-29T13:00:00Z" };
  await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: row });
  await start(vi.fn().mockResolvedValue(result())); await reader.refresh(42);
  await vi.waitFor(() => expect(resolve()).toBeNull());
  const read = await order.nextTaskRead();
  await pages.foldTasksPage(tracker.tasksAddress(scope.deviceId, scope.projectId), {
    tasks: [row], above: Infinity, through: -Infinity, read, pullRead: read,
  }, () => []);
  await vi.waitFor(() => expect(resolve()?.title).toBe("Page-positive"));
  const before = index.referenceIndexVersion();
  await listed("Unrelated #99", 99);
  await vi.waitFor(() => expect(index.referenceIndexVersion()).toBeGreaterThan(before));
  expect(resolve()?.title).toBe("Page-positive");
});
