import { beforeEach, describe, expect, it, vi } from "vitest";

const data = vi.hoisted(() => ({ previous: undefined }));
vi.mock("../src/core/localCache.js", () => ({
  readCanonicalCached: vi.fn(async () => data.previous),
  recordWriteOf: (record) => record?.write ?? null,
}));
vi.mock("../src/core/threadSync.js", () => ({ threadWindow: vi.fn(() => null) }));
import { refreshWriteOperation } from "../src/core/writeOperationRefresh.js";

const address = { deviceId: "device-A", entityId: "project-A", kind: "tracker-task", sub: "task-A" };
const record = { operationId: "operation-A", request: { method: "tasks.comment", params: { task_id: "task-A" } },
  overlay: [{ address }] };
const frame = { state: "succeeded", published: [{ kind: "project", id: "project-A", generation: 1, sequence: 42 }] };
let cache;
beforeEach(() => {
  data.previous = { write: "captured-base", value: { task: { title: "before" }, timeline: [] } };
  cache = { observePublication: vi.fn(), get: vi.fn(async () => ({ overlayActive: false })) };
});

describe("canonical publication refresh", () => {
  it("reads after confirmation and guards publication against the captured base and opening", async () => {
    const answer = { task: { title: "after" }, timeline: [{ body: "normalized" }] };
    const authority = { active: () => true, call: vi.fn(async () => answer) };
    await refreshWriteOperation({ cache, frame, record, authority });
    expect(authority.call).toHaveBeenCalledWith("tasks.get", { task_id: "task-A" });
    expect(cache.observePublication).toHaveBeenCalledWith({ accept: authority.active,
      canonical: [{ address, value: answer, publication: frame.published, guard: { written: "captured-base" } }] });
  });

  it("does not publish an answer from a replaced opening", async () => {
    let active = true;
    const authority = { active: () => active, call: vi.fn(async () => {
      active = false;
      return { task: {}, timeline: [] };
    }) };
    await refreshWriteOperation({ cache, frame, record, authority });
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
});
