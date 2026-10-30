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
  await operations.confirm(operationId, { revision: 2, confirmation: { state: "succeeded", published } });
  const authority = { active: () => true, call: vi.fn(async () => ({ task: incoming, timeline })) };
  return { cache, tracker, content, operations, operationId, address, authority,
    refresh: async () => refreshWriteOperation({ cache: operations, authority, record: await operations.get(operationId),
      frame: { state: "succeeded", published, refresh: [{ kind: "task", id: taskId }] } }) };
}

it("honors a facts-helper contract stub with valid retained watch lineage despite optional unknown counters", async () => {
  const summaries = await import("../src/core/taskSummaryCache.js");
  // Conditional adapter control only: this does not bootstrap persisted shared
  // authority or claim end-to-end recovery from a fresh/unknown namespace.
  const facts = vi.spyOn(summaries, "reconcileTaskReadFacts").mockReturnValue({
    task: incoming, acceptedWatch: false, watchCertain: true, needsUnreadRefresh: true,
  });
  const state = await canonicalOperation();
  await state.refresh();
  expect(facts).toHaveBeenCalledWith(scope, expect.objectContaining({ id: taskId }), incoming,
    expect.objectContaining({ markerPolicy: "legacy", watchGuard: expect.objectContaining({ id: taskId }) }));
  const saved = (await state.cache.readCanonicalCached(state.address)).value;
  const { __task_content_cache: proof, ...publicTask } = saved.task;
  expect(publicTask).toEqual(incoming);
  expect(proof.header.read_order).toBe(proof.body.read_order);
  expect(saved.timeline).toEqual(timeline);
  expect((await state.operations.get(state.operationId)).overlayActive).toBe(false);
  expect(state.authority.call).toHaveBeenCalledTimes(1);
});

it.each(["fresh", "pending"])("does not certify ordinary full content as a complete canonical receipt under %s watch uncertainty", async (kind) => {
  const state = await canonicalOperation();
  const watch = await import("../src/core/taskWatchAuthority.js");
  const selected = vi.spyOn(state.content, "reconcileTaskContent");
  const owner = kind === "pending" ? await watch.beginTaskWatchMutation(scope, taskId, { watched: false, metadataOnly: true }) : null;
  if (kind === "pending") expect(owner).not.toBeNull();
  const before = await state.cache.readCanonicalCached(state.address);
  try {
    await expect(state.refresh()).rejects.toThrow("superseded");
    expect(selected.mock.results.some(({ value }) => value?.acceptedHeader && value?.acceptedBody && !value?.needsFullRefresh)).toBe(true);
    expect(await state.cache.readCanonicalCached(state.address)).toEqual(before);
    expect((await state.operations.get(state.operationId)).overlayActive).toBe(true);
    expect(state.authority.call).toHaveBeenCalledTimes(1);
  } finally {
    if (owner) await watch.settleTaskWatchMutation(owner, { rollback: true });
  }
});

it("preserves an actual terminal watch-intent note when a conditional valid-lineage publication advances its read note", async () => {
  const summaries = await import("../src/core/taskSummaryCache.js");
  // Adapter control only: fresh watch authority remains unknown in production.
  // This stub admits the full publication so its independent note write runs.
  vi.spyOn(summaries, "reconcileTaskReadFacts").mockReturnValue({
    task: incoming, acceptedWatch: false, watchCertain: true, needsUnreadRefresh: true,
  });
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
