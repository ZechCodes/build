import { beforeEach, expect, it, vi } from "vitest";
const mock = vi.hoisted(() => ({ cache: vi.fn(), blocks: vi.fn(), capture: vi.fn(), callOrder: [], read: 0,
  tickets: [], current: () => true }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => ({}),
  whenGreeted: async (_context, send) => ({ sent: send(mock.current), stands: mock.current }) }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => ({ tasks: { waitingOn: true } }) }));
vi.mock("../src/core/taskWaitingOnSupport.js", () => ({ readTaskWaitingOnEverSupported: async () => false,
  TASK_WAITING_ON_UPDATE: "Update", TASK_WAITING_ON_DOWNGRADED: "Downgraded" }));
vi.mock("../src/core/taskContentAuthority.js", () => ({ captureTaskContentRead: (...args) => mock.capture(...args) }));
vi.mock("../src/core/taskWaitingOnCache.js", () => ({ captureTaskWaitingOnBlocksRead: async () => ({ readOrder: 1 }),
  cacheTaskWaitingOnResponse: (...args) => mock.cache(...args), cacheTaskWaitingOnBlocks: (...args) => mock.blocks(...args) }));
import { createTaskWaitingOnActions } from "../src/core/taskWaitingOnActions.js";
beforeEach(() => {
  vi.clearAllMocks(); mock.callOrder = []; mock.read = 0; mock.tickets = [];
  mock.capture.mockImplementation(async () => {
    const readOrder = ++mock.read;
    const contentRead = { readOrder, ticket: { storeGeneration: "store", scopeGeneration: "scope", retirementEpoch: "epoch" }, revalidate: false };
    mock.callOrder.push(["issued", readOrder]); mock.tickets.push(contentRead);
    return contentRead;
  });
});

it.each([
  ["add", [["blocker"]], "tasks.waiting_on.add"], ["remove", [["blocker"]], "tasks.waiting_on.remove"],
  ["assign", [{ assignee: { kind: "user" } }], "tasks.assign"], ["retry", ["saved-intent"], "tasks.assign"],
  ["create", [{ title: "New" }], "tasks.create"],
])("captures dispatch order before %s and passes that exact token to the cache", async (action, args, method) => {
  const call = vi.fn(async (method) => { mock.callOrder.push([method]); return { task: { id: "task" } }; });
  const actions = createTaskWaitingOnActions({ deviceId: "d", projectId: "p", taskId: "task", call });
  await actions[action](...args);
  expect(mock.callOrder).toEqual([["issued", 1], [method]]);
  expect(mock.capture).toHaveBeenCalledWith({ deviceId: "d", projectId: "p" });
  expect(mock.cache).toHaveBeenCalledWith("d", "p", { task: { id: "task" } }, expect.objectContaining({ issuedRead: 1,
    contentRead: mock.tickets[0] }));
  expect(mock.cache.mock.calls[0][3].contentRead).toBe(mock.tickets[0]);
});

it("does not allocate a mutation dispatch token for reverse reads", async () => {
  const call = vi.fn().mockResolvedValue({ blocks: [] });
  const actions = createTaskWaitingOnActions({ deviceId: "d", projectId: "p", taskId: "task", call });
  await actions.blocks();
  expect(mock.callOrder).toEqual([]); expect(mock.cache).not.toHaveBeenCalled();
  expect(mock.capture).not.toHaveBeenCalled();
});
