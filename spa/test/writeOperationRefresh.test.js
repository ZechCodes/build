import { beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";

const failure = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/writes/confirmed-failure.json", import.meta.url)));
const data = vi.hoisted(() => ({ previous: undefined, addresses: [], detailBasis: 7 }));
vi.mock("../src/core/localCache.js", () => ({
  readCanonicalCached: vi.fn(async () => data.previous),
  cachedAddresses: vi.fn(async () => data.addresses),
  recordWriteOf: (record) => record?.write ?? null,
}));
vi.mock("../src/core/trackerCache.js", () => ({
  TRACKER_TASK_KIND: "tracker-task",
  taskAddress: (deviceId, entityId, sub) => ({ deviceId, entityId, kind: "tracker-task", sub }),
  captureTaskDetailRead: vi.fn(async () => data.detailBasis),
}));
vi.mock("../src/core/conversationCache.js", () => ({
  threadCacheAddress: ({ deviceId, entityId, conversationId, agentId, threadId }) => ({ deviceId, entityId, kind: "thread", sub: conversationId || agentId || "", ...(threadId ? { threadId } : {}) }),
}));
vi.mock("../src/core/threadSync.js", () => ({ threadWindow: vi.fn(() => null) }));
import { refreshWriteOperation } from "../src/core/writeOperationRefresh.js";

const address = { deviceId: "device-A", entityId: "project-A", kind: "tracker-task", sub: "task-A" };
const record = { deviceId: "device-A", operationId: "operation-A", owner: { kind: "task-comment", taskId: "task-A", projectId: "project-A" }, request: { method: "tasks.comment", params: { task_id: "task-A" } },
  overlay: [{ address }] };
const frame = { state: "succeeded", published: [{ kind: "project", id: "project-A", generation: 1, sequence: 42 }] };
let cache;
beforeEach(() => {
  data.previous = { write: "captured-base", value: { task: { title: "before" }, timeline: [] } };
  data.addresses = [];
  cache = { observePublication: vi.fn(async ({ canonical }) => canonical.map(({ address }) => address)), get: vi.fn(async () => ({ overlayActive: false })) };
});

describe("canonical publication refresh", () => {
  it("reads after confirmation and guards publication against the captured base and opening", async () => {
    const answer = { task: { title: "after" }, timeline: [{ body: "normalized" }] };
    const authority = { active: () => true, call: vi.fn(async () => answer) };
    await refreshWriteOperation({ cache, frame, record, authority });
    expect(authority.call).toHaveBeenCalledWith("tasks.get", { task_id: "task-A" });
    expect(cache.observePublication).toHaveBeenCalledWith({ accept: authority.active,
      canonical: [{ address, value: answer, publication: frame.published, guard: { written: "captured-base", detailBasis: 7 } }] });
  });

  it("does not publish an answer from a replaced opening", async () => {
    let active = true;
    const authority = { active: () => active, call: vi.fn(async () => {
      active = false;
      return { task: {}, timeline: [] };
    }) };
    await expect(refreshWriteOperation({ cache, frame, record, authority })).rejects.toThrow("opening");
    expect(cache.observePublication).not.toHaveBeenCalled();
  });

  it("keeps retrying when an intervening canonical write prevented layer retirement", async () => {
    cache.get.mockResolvedValue({ overlayActive: true });
    const authority = { active: () => true, call: vi.fn(async () => ({ task: {}, timeline: [] })) };
    await expect(refreshWriteOperation({ cache, frame, record, authority })).rejects.toThrow("publication");
  });

  it("never retires a thread layer with a rejected generation's null window", async () => {
    const thread = { ...record, request: { params: { entity_id: "workspace-A", conversation_id: "conversation-A" } },
      overlay: [{ address: { ...address, kind: "thread" } }] };
    const authority = { active: () => true, call: vi.fn(async () => ({ items: [] })) };
    await expect(refreshWriteOperation({ cache, frame, record: thread, authority })).rejects.toThrow("generation");
    expect(cache.observePublication).not.toHaveBeenCalled();
  });

  it("refreshes the failed-unapplied fixture before ack using persisted task ownership", async () => {
    const failedRecord = { ...record, overlay: [], request: { method: "tasks.comment", params: { task_id: failure.refresh[0].id } } };
    const authority = { active: () => true, call: vi.fn(async () => ({ task: { id: failure.refresh[0].id }, timeline: [] })) };
    await refreshWriteOperation({ cache, frame: failure, record: failedRecord, authority });
    expect(authority.call).toHaveBeenCalledWith("tasks.get", { task_id: failure.refresh[0].id });
    expect(cache.observePublication.mock.calls[0][0].canonical[0]).toMatchObject({ address: { ...address, sub: failure.refresh[0].id }, publication: [], guard: { detailBasis: 7 } });
  });

  it("finds the requested task in known cache addresses when the request has no project", async () => {
    const heldRecord = { ...record, overlay: [], owner: undefined };
    data.addresses = [{ ...address, deviceId: "unrelated-device" }, address];
    const authority = { active: () => true, call: vi.fn(async () => ({ task: {}, timeline: [] })) };
    await refreshWriteOperation({ cache, frame: { ...frame, refresh: [{ kind: "task", id: "task-A" }] }, record: heldRecord, authority });
    expect(cache.observePublication.mock.calls[0][0].canonical[0].address).toEqual(address);
  });

  it.each([
    { kind: "future-kind", id: "task-A", address },
    { kind: "task", id: "unmapped-task" },
    { kind: "thread", id: "unmapped-thread" },
  ])("keeps an unmappable required refresh pending: %j", async (refresh) => {
    const authority = { active: () => true, call: vi.fn() };
    const unmapped = { ...record, owner: undefined, overlay: [], request: { method: "unknown", params: {} } };
    await expect(refreshWriteOperation({ cache, frame: { ...frame, published: [], refresh: [refresh] }, record: unmapped, authority })).rejects.toThrow("refresh");
    expect(authority.call).not.toHaveBeenCalled();
    expect(cache.observePublication).not.toHaveBeenCalled();
  });

  it("retains a required refresh when a newer task detail or canonical write rejects publication", async () => {
    cache.observePublication.mockResolvedValue([]);
    const authority = { active: () => true, call: vi.fn(async () => ({ task: {}, timeline: [] })) };
    await expect(refreshWriteOperation({ cache, frame: { ...frame, state: "failed", refresh: [{ kind: "task", id: "task-A" }] }, record: { ...record, overlay: [] }, authority })).rejects.toThrow("publication");
  });

  it("requires the persisted thread generation even when its canonical window is cold", async () => {
    data.previous = undefined;
    const thread = { ...record, owner: { kind: "chat" }, overlay: [], request: { method: "thread.post", params: {
      entity_id: "workspace-A", conversation_id: "conversation-A", thread_id: "thread-A", thread_generation_revision: 3,
    } } };
    const authority = { active: () => true, call: vi.fn(async () => ({ items: [], thread_id: "replacement-thread", thread_generation_revision: 4 })) };
    await expect(refreshWriteOperation({ cache, frame: { ...frame, refresh: [{ kind: "thread", id: "thread-A" }] }, record: thread, authority })).rejects.toThrow("generation");
    expect(authority.call).toHaveBeenCalledWith("thread.page", expect.objectContaining({ thread_id: "thread-A", thread_generation_revision: 3 }));
    expect(cache.observePublication).not.toHaveBeenCalled();
  });
});
