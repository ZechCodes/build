import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const scope = { accountId: "content-account", principalId: "content-principal", deviceId: "content-device" };
const projectId = "proj-1", taskId = "task-42";
const task = { id: taskId, project_id: projectId, number: 42, title: "Canonical task", body: "Canonical full body",
  updated_at: "2026-10-29T00:00:00.000900Z" };
beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

it("captures canonical task refresh ordinary order before RPC rather than assigning its receipt order", async () => {
  const local = await import("../src/core/localCache.js");
  const tracker = await import("../src/core/trackerCache.js");
  const { TASK_READ_COUNT_ADDRESS } = await import("../src/core/taskReadOrder.js");
  const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
  const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
  await tracker.writeTaskRecord(scope.deviceId, projectId, taskId, tracker.taskRecord(task, []));
  const before = (await local.readCanonicalCached(TASK_READ_COUNT_ADDRESS))?.value || 0;
  let issued;
  const authority = { active: () => true, call: vi.fn(async () => {
    issued = (await local.readCanonicalCached(TASK_READ_COUNT_ADDRESS))?.value || 0;
    return { task: { ...task, title: "Refreshed task", body: "Refreshed full body" }, timeline: [] };
  }) };
  await refreshWriteOperation({
    cache: createWriteOperationCache(scope), authority,
    frame: { state: "failed", published: [], refresh: [{ kind: "task", id: taskId }] },
    record: { deviceId: scope.deviceId, operationId: "refresh", owner: { projectId }, overlay: [], request: { params: { task_id: taskId } } },
  });
  expect(issued).toBeGreaterThan(before);
  expect(authority.call).toHaveBeenCalledTimes(1);
  expect((await tracker.readTaskRecord(scope.deviceId, projectId, taskId)).task.body).toBe("Refreshed full body");
});

it("does not certify a full canonical publication or retire its layer when the incoming body is older", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
  const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
  const cache = createWriteOperationCache(scope), operationId = "body-publication";
  const address = tracker.taskAddress(scope.deviceId, projectId, taskId);
  const published = [{ kind: "project", id: projectId, generation: 0, sequence: 7 }];
  await tracker.writeTaskRecord(scope.deviceId, projectId, taskId, tracker.taskRecord(task, []));
  await cache.put({ operationId, revision: 0, owner: { projectId },
    request: { method: "tasks.comment", params: { task_id: taskId, body: "Accepted comment" } } });
  await cache.accept(operationId, { revision: 1, ack: {}, overlay: [{ address, kind: "upsert", path: ["timeline"],
    value: { id: "comment", body: "Accepted comment" } }] });
  await cache.confirm(operationId, { revision: 2, confirmation: { state: "succeeded", published } });
  const authority = { active: () => true, call: vi.fn(async () => ({
    task: { ...task, title: "Old task", body: "Old full body", updated_at: "2026-10-28T23:59:59Z" },
    timeline: [{ id: "comment", body: "Accepted comment" }],
  })) };
  await expect(refreshWriteOperation({ cache, authority, record: await cache.get(operationId),
    frame: { state: "succeeded", published, refresh: [{ kind: "task", id: taskId }] } })).rejects.toThrow("superseded");
  expect((await cache.get(operationId)).overlayActive).toBe(true);
  expect((await tracker.readTaskRecord(scope.deviceId, projectId, taskId)).task.body).toBe(task.body);
  expect(authority.call).toHaveBeenCalledTimes(1);
});
