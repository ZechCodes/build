// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});
afterEach(() => vi.restoreAllMocks());

it("returns an unscoped assignment acknowledgment without writing any cache records", async () => {
  const { createTaskWaitingOnActions } = await import("../src/core/taskWaitingOnActions.js");
  const writes = vi.spyOn(IDBObjectStore.prototype, "put");
  const answer = { task: { id: "task-acceptance", project_id: "p", title: "Accept project workspace" },
    dispatch: { agent_id: "worker", conversation_id: "conversation-worker" } };
  const call = vi.fn().mockResolvedValue(answer);
  const actions = createTaskWaitingOnActions({ deviceId: "", taskId: answer.task.id, call });
  await expect(actions.assign({ assignee: { kind: "agent", agent_id: "worker" } })).resolves.toBe(answer);
  expect(call).toHaveBeenCalledExactlyOnceWith("tasks.assign", { task_id: answer.task.id,
    assignee: { kind: "agent", agent_id: "worker" } });
  expect(writes).not.toHaveBeenCalled();
});
