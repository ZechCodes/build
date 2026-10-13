import { beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ context: {}, capabilities: { tasks: { waitingOn: true } }, cache: vi.fn(), blocks: vi.fn(), seen: vi.fn(), greet: vi.fn(), stands: vi.fn(() => true) }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => mocks.context, whenGreeted: (...args) => mocks.greet(...args) }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => mocks.capabilities }));
vi.mock("../src/core/taskWaitingOnCache.js", () => ({ captureTaskWaitingOnBlocksRead: async () => ({ readOrder: 1 }), cacheTaskWaitingOnResponse: (...args) => mocks.cache(...args), cacheTaskWaitingOnBlocks: (...args) => mocks.blocks(...args) }));
vi.mock("../src/core/taskWaitingOnSupport.js", async (original) => ({
  ...await original(), readTaskWaitingOnEverSupported: (...args) => mocks.seen(...args),
}));
import { createTaskWaitingOnActions } from "../src/core/taskWaitingOnActions.js";

let call, actions;
beforeEach(() => {
  vi.clearAllMocks();
  mocks.capabilities = { tasks: { waitingOn: true } };
  mocks.context = {};
  mocks.seen.mockResolvedValue(false);
  mocks.stands.mockReturnValue(true);
  mocks.greet.mockImplementation(async (_context, dispatch) => ({ sent: dispatch(mocks.stands), stands: mocks.stands }));
  call = vi.fn(async () => ({ task: { id: "t" } }));
  actions = createTaskWaitingOnActions({ deviceId: "d", projectId: "p", taskId: "t", call });
});

it("adds and removes exact edges, writing successful replies before returning", async () => {
  await actions.add(["a", "b"]);
  await actions.remove(["a"]);
  expect(call.mock.calls).toEqual([["tasks.waiting_on.add", { task_id: "t", waiting_on: ["a", "b"] }], ["tasks.waiting_on.remove", { task_id: "t", waiting_on: ["a"] }]]);
  expect(mocks.cache).toHaveBeenCalledTimes(2);
});

it("waits for greeting then checks current support, preserving old request shapes", async () => {
  mocks.greet.mockImplementation(async (_context, dispatch) => {
    mocks.capabilities = { tasks: { waitingOn: false } };
    return { sent: dispatch(mocks.stands), stands: mocks.stands };
  });
  await expect(actions.add(["a"])).rejects.toThrow(/Update.*bridge/);
  await expect(actions.create({ title: "Work", waiting_on: ["a"] })).rejects.toThrow(/Update.*bridge/);
  await expect(actions.retry("intent")).rejects.toThrow(/Update.*bridge/);
  mocks.seen.mockResolvedValue(true);
  await expect(actions.assign({ assignee: { kind: "new_workspace" } })).rejects.toThrow(/downgraded.*update/i);
  expect(call).not.toHaveBeenCalled();
  await actions.create({ title: "Legacy" });
  expect(call).toHaveBeenCalledWith("tasks.create", { project_id: "p", title: "Legacy" });
});

it("retains greeting authority across the asynchronous send-time cache read", async () => {
  let finish;
  mocks.seen.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const request = actions.assign({ assignee: { kind: "new_workspace" } });
  await vi.waitFor(() => expect(mocks.seen).toHaveBeenCalledWith("d"));
  expect(call).not.toHaveBeenCalled();
  mocks.stands.mockReturnValue(false);
  finish(false);
  await expect(request).rejects.toThrow(/changed|superseded/);
  expect(call).not.toHaveBeenCalled();
  expect(mocks.cache).not.toHaveBeenCalled();
});

it("retries the saved failed intent without replaying an assignment target or note", async () => {
  await actions.retry("saved-id");
  expect(call).toHaveBeenCalledWith("tasks.assign", { task_id: "t", retry_deferred_id: "saved-id" });
});

it("caches reverse paging independently and never caches a refused or superseded reply", async () => {
  const answer = { blocks: [{ task_id: "dependent" }], next_cursor: "next" };
  call.mockResolvedValueOnce(answer);
  await actions.blocks({ cursor: "first", limit: 20 });
  expect(call).toHaveBeenCalledWith("tasks.waiting_on.blocks", { task_id: "t", cursor: "first", limit: 20 });
  expect(mocks.blocks).toHaveBeenCalledWith("d", "p", "t", answer, expect.objectContaining({ cursor: "first" }));
  call.mockRejectedValueOnce(new Error("cycle"));
  await expect(actions.add(["a"])).rejects.toThrow("cycle");
  expect(mocks.cache).not.toHaveBeenCalled();
  mocks.stands.mockReturnValue(false);
  await expect(actions.remove(["a"])).rejects.toThrow(/changed|superseded/);
  expect(mocks.cache).not.toHaveBeenCalled();
});

it("refuses ordinary new-workspace assignment only on a previously capable device", async () => {
  mocks.capabilities = { tasks: {} };
  await actions.assign({ assignee: { kind: "new_workspace" }, note: "Legacy note" });
  expect(call).toHaveBeenLastCalledWith("tasks.assign", { task_id: "t", assignee: { kind: "new_workspace" }, note: "Legacy note" });
  call.mockClear();
  mocks.seen.mockResolvedValue(true);
  await expect(actions.assign({ assignee: { kind: "new_workspace" } })).rejects.toThrow(/downgraded.*update/i);
  expect(call).not.toHaveBeenCalled();
  expect(mocks.cache).toHaveBeenCalledTimes(1);
  mocks.capabilities = { tasks: { waitingOn: true } };
  await actions.assign({ assignee: { kind: "new_workspace" } });
  expect(call).toHaveBeenCalledExactlyOnceWith("tasks.assign", { task_id: "t", assignee: { kind: "new_workspace" } });
});

it("keeps established non-workspace assignment and unassignment shapes on a downgraded device", async () => {
  mocks.capabilities = { tasks: {} };
  mocks.seen.mockResolvedValue(true);
  for (const assignee of [null, { kind: "user" }, { kind: "agent", agent_id: "a" }]) {
    await actions.assign({ assignee });
    expect(call).toHaveBeenLastCalledWith("tasks.assign", { task_id: "t", assignee });
  }
  expect(mocks.seen).not.toHaveBeenCalled();
});

it.each([null, undefined])("refuses unknown history (%s) and offers retry/refresh", async (history) => {
  mocks.capabilities = { tasks: {} };
  mocks.seen.mockResolvedValue(history);
  await expect(actions.assign({ assignee: { kind: "new_workspace" } })).rejects.toThrow(/retry|refresh/i);
  expect(call).not.toHaveBeenCalled();
});
