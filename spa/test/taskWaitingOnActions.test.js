import { beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ context: {}, capabilities: { tasks: { waitingOn: true } }, cache: vi.fn(), blocks: vi.fn(), greet: vi.fn(), stands: vi.fn(() => true) }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => mocks.context, whenGreeted: (...args) => mocks.greet(...args) }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => mocks.capabilities }));
vi.mock("../src/core/taskWaitingOnCache.js", () => ({ cacheTaskWaitingOnResponse: (...args) => mocks.cache(...args), cacheTaskWaitingOnBlocks: (...args) => mocks.blocks(...args) }));
import { createTaskWaitingOnActions } from "../src/core/taskWaitingOnActions.js";

let call, actions;
beforeEach(() => {
  vi.clearAllMocks();
  mocks.capabilities = { tasks: { waitingOn: true } };
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
  await expect(actions.assign({ assignee: { kind: "agent", agent_id: "a" } }, { requiresWaitingOn: true })).rejects.toThrow(/Update.*bridge/);
  expect(call).not.toHaveBeenCalled();
  await actions.create({ title: "Legacy" });
  expect(call).toHaveBeenCalledWith("tasks.create", { project_id: "p", title: "Legacy" });
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
