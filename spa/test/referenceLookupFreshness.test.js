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
const listed = async (title) => tracker.writeTasksRecord(scope.deviceId, scope.projectId,
  tracker.tasksRecord([{ ...target, title }], [], await order.nextTaskRead()));

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
