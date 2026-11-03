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

it("publishes admitted full content with its independent watch read and review fields", async () => {
  const local = await import("../src/core/localCache.js");
  const tracker = await import("../src/core/trackerCache.js");
  const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
  const { TASK_READ_COUNT_ADDRESS } = await import("../src/core/taskReadOrder.js");
  const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
  const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
  const address = tracker.taskAddress(scope.deviceId, projectId, taskId);
  const contentRead = await captureTaskContentRead(address, { revalidate: "full" });
  await tracker.writeTaskRecord(scope.deviceId, projectId, taskId, tracker.taskRecord({ ...task,
    watched: true, read_through: "tc-10", watch_started_after: "te-05", unread_count: 3,
    unread_mentions: 1, unread_agent_comments: 2, session_started_ms: 10, last_activity_ms: 20,
    review_summary: { task_id: taskId, version: 1, status: "open", mode: "pull_request" },
  }, []), { contentRead });
  const cache = createWriteOperationCache(scope), operationId = "independent-publication";
  const published = [{ kind: "project", id: projectId, generation: 0, sequence: 7 }];
  await cache.put({ operationId, revision: 0, owner: { projectId },
    request: { method: "tasks.comment", params: { task_id: taskId, body: "Accepted comment" } } });
  await cache.accept(operationId, { revision: 1, ack: {}, overlay: [{ address, kind: "upsert", path: ["timeline"],
    value: { id: "comment", body: "Accepted comment" } }] });
  const frame = { operation_id: operationId, revision: 2, state: "succeeded", published, refresh: [{ kind: "task", id: taskId }] };
  await cache.confirm(operationId, { revision: 2, confirmation: frame, refreshPending: true });
  const incoming = { ...task, title: "New task", body: "New full body", updated_at: "2026-10-29T00:01:00Z",
    watched: false, read_through: "tc-20",
    review_summary: { task_id: taskId, version: 2, status: "closed", mode: "pull_request" } };
  const timeline = [{ id: "comment", body: "Accepted comment" }];
  let issued;
  const authority = { active: () => true, call: vi.fn(async () => {
    issued = (await local.readCanonicalCached(TASK_READ_COUNT_ADDRESS)).value;
    return { task: incoming, timeline };
  }) };
  await refreshWriteOperation({ cache, authority, record: await cache.get(operationId),
    frame });
  const canonical = await local.readCanonicalCached(address);
  const proof = canonical.value.task.__task_content_cache;
  expect(canonical.value.task).toMatchObject(incoming);
  expect(proof.body.read_order).toBe(issued);
  expect(canonical.value.timeline).toEqual(timeline);
  expect((await cache.get(operationId)).overlayActive).toBe(false);
  expect(authority.call).toHaveBeenCalledTimes(1);
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
  const local = await import("../src/core/localCache.js");
  const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
  const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
  const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
  const cache = createWriteOperationCache(scope), operationId = "body-publication";
  const address = tracker.taskAddress(scope.deviceId, projectId, taskId);
  const published = [{ kind: "project", id: projectId, generation: 0, sequence: 7 }];
  const contentRead = await captureTaskContentRead(address, { revalidate: "full" });
  await tracker.writeTaskRecord(scope.deviceId, projectId, taskId, tracker.taskRecord({ ...task,
    watched: true, read_through: "tc-10", review_summary: { task_id: taskId, version: 1, status: "open" },
  }, []), { contentRead });
  const before = await local.readCanonicalCached(address);
  await cache.put({ operationId, revision: 0, owner: { projectId },
    request: { method: "tasks.comment", params: { task_id: taskId, body: "Accepted comment" } } });
  await cache.accept(operationId, { revision: 1, ack: {}, overlay: [{ address, kind: "upsert", path: ["timeline"],
    value: { id: "comment", body: "Accepted comment" } }] });
  const frame = { operation_id: operationId, revision: 2, state: "succeeded", published, refresh: [{ kind: "task", id: taskId }] };
  await cache.confirm(operationId, { revision: 2, confirmation: frame, refreshPending: true });
  const authority = { active: () => true, call: vi.fn(async () => ({
    task: { ...task, title: "Old task", body: "Old full body", updated_at: "2026-10-28T23:59:59Z",
      watched: false, read_through: "tc-20", review_summary: { task_id: taskId, version: 2, status: "closed" } },
    timeline: [{ id: "comment", body: "Accepted comment" }],
  })) };
  expect(await refreshWriteOperation({ cache, authority, record: await cache.get(operationId),
    frame })).toMatchObject({ complete: false });
  expect((await cache.get(operationId)).overlayActive).toBe(true);
  expect((await tracker.readTaskRecord(scope.deviceId, projectId, taskId)).task.body).toBe(task.body);
  expect((await local.readCanonicalCached(address)).value.task.body).toBe(before.value.task.body);
  expect(authority.call).toHaveBeenCalledTimes(1);
});

it("retains canonical refresh write guards across an optimistic body change followed by an identical rollback", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const local = await import("../src/core/localCache.js");
  const { mergeTaskContentRecordsTogether, retireTaskContent } = await import("../src/core/taskContentAuthority.js");
  const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
  const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
  const cache = createWriteOperationCache(scope), address = tracker.taskAddress(scope.deviceId, projectId, taskId);
  await local.writeCached(address, tracker.taskRecord(task, []));
  const before = await local.readCanonicalCached(address);
  let resolve;
  const authority = { active: () => true, call: vi.fn(() => new Promise((yes) => { resolve = yes; })) };
  const pending = refreshWriteOperation({ cache, authority,
    frame: { state: "failed", published: [], refresh: [{ kind: "task", id: taskId }] },
    record: { deviceId: scope.deviceId, operationId: "aba-refresh", owner: { projectId }, overlay: [], request: { params: { task_id: taskId } } },
  });
  const refused = expect(pending).resolves.toMatchObject({ complete: false });
  await vi.waitFor(() => expect(authority.call).toHaveBeenCalledTimes(1));
  for (const body of ["Optimistic body", task.body]) await mergeTaskContentRecordsTogether(
    { deviceId: scope.deviceId, projectId }, [address], ([record]) => ({
      values: [{ ...record.value, task: retireTaskContent({ ...record.value.task, body }, "body") }], retire: true,
    }),
  );
  expect((await local.readCanonicalCached(address)).write).not.toBe(before.write);
  resolve({ task: { ...task, title: "Older refreshed title" }, timeline: [] });
  await refused;
  const held = (await tracker.readTaskRecord(scope.deviceId, projectId, taskId)).task;
  expect(held.title).toBe(task.title);
  expect(held.body).toBe(task.body);
  expect(authority.call).toHaveBeenCalledTimes(1);
});
