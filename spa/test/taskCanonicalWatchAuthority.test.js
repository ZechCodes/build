import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const deviceId = "canonical-watch", projectId = "proj-1", taskId = "task-42";
const scope = { deviceId, projectId };
const operationScope = { accountId: "canonical-account", principalId: "canonical-principal", deviceId };
const held = { id: taskId, project_id: projectId, number: 42, title: "Held", body: "Held body",
  state: "open", status: "ready", watched: true, watch_started_after: "", read_through: "tc-10",
  updated_at: "2026-10-29T00:00:00Z" };
const incoming = { ...held, title: "Full refresh", body: "Full refreshed body", read_through: "tc-20",
  updated_at: "2026-10-29T00:01:00Z" };
const published = [{ kind: "project", id: projectId, generation: 0, sequence: 7 }];
const timeline = [{ id: "comment", body: "Accepted comment" }];

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});
afterEach(() => vi.restoreAllMocks());

async function canonicalOperation() {
  const cache = await import("../src/core/localCache.js");
  const tracker = await import("../src/core/trackerCache.js");
  const content = await import("../src/core/taskContentAuthority.js");
  const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
  const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
  const address = tracker.taskAddress(deviceId, projectId, taskId);
  await cache.writeCached(address, tracker.taskRecord(held, []));
  const operations = createWriteOperationCache(operationScope), operationId = "watch-refresh";
  await operations.put({ operationId, revision: 0, owner: { projectId },
    request: { method: "tasks.comment", params: { task_id: taskId, body: "Accepted comment" } } });
  await operations.accept(operationId, { revision: 1, ack: {}, overlay: [{ address, kind: "upsert", path: ["timeline"], value: timeline[0] }] });
  const frame = { operation_id: operationId, revision: 2, state: "succeeded", published, refresh: [{ kind: "task", id: taskId }] };
  await operations.confirm(operationId, { revision: 2, confirmation: frame, refreshPending: true });
  const authority = { active: () => true, call: vi.fn(async () => ({ task: incoming, timeline })) };
  return { cache, tracker, content, operations, operationId, address, authority,
    refresh: async () => refreshWriteOperation({ cache: operations, authority, record: await operations.get(operationId),
      frame }) };
}

it("publishes ordinary full content despite optional unknown counter families", async () => {
  const state = await canonicalOperation();
  expect(await state.refresh()).toMatchObject({ complete: true });
  const saved = (await state.cache.readCanonicalCached(state.address)).value;
  expect(saved.task).toMatchObject(incoming);
  expect(saved.task.__task_watch_cache.uncertain).toBe(false);
  const proof = saved.task.__task_content_cache;
  expect(proof.header.read_order).toBe(proof.body.read_order);
  expect(saved.timeline).toEqual(timeline);
  expect((await state.operations.get(state.operationId)).overlayActive).toBe(false);
  expect(state.authority.call).toHaveBeenCalledTimes(1);
});

it("publishes useful content while retaining the canonical obligation during a pending watch intent", async () => {
  const state = await canonicalOperation();
  const watch = await import("../src/core/taskWatchAuthority.js");
  const owner = await watch.beginTaskWatchMutation(scope, taskId, { watched: false, metadataOnly: true });
  expect(owner).not.toBeNull();
  try {
    expect(await state.refresh()).toMatchObject({ complete: false, refresh: { state: "blocked" } });
    const saved = await state.cache.readCanonicalCached(state.address);
    expect(saved.value.task).toMatchObject({ title: incoming.title, body: incoming.body });
    expect(saved.publication || []).toEqual([]);
    expect((await state.operations.get(state.operationId)).overlayActive).toBe(true);
    expect(state.authority.call).toHaveBeenCalledTimes(1);
  } finally { await watch.settleTaskWatchMutation(owner, { rollback: true }); }
});

it("preserves an actual terminal watch-intent note when a covering publication advances its read note", async () => {
  const state = await canonicalOperation();
  const watch = await import("../src/core/taskWatchAuthority.js");
  const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
  const noteAddress = taskWatchIntentAddress(scope, taskId);
  const owner = await watch.beginTaskWatchMutation(scope, taskId, { watched: false, metadataOnly: true });
  expect(owner).not.toBeNull();
  await watch.settleTaskWatchMutation(owner, { rollback: true });
  const before = (await state.cache.readCanonicalCached(noteAddress)).value;
  expect(before.latest_watch_intent).toEqual(expect.objectContaining({ taskId, disposition: "rejected" }));
  await state.refresh();
  const after = (await state.cache.readCanonicalCached(noteAddress)).value;
  expect(after.latest_watch_intent).toEqual(before.latest_watch_intent);
  expect(after.rows[taskId]).toBeGreaterThan(before.rows?.[taskId] || 0);
  expect((await state.cache.readCanonicalCached(state.address)).value.timeline).toEqual(timeline);
  expect((await state.operations.get(state.operationId)).overlayActive).toBe(false);
  expect(state.authority.call).toHaveBeenCalledTimes(1);
});
