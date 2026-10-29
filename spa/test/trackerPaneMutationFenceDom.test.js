/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

let blockedStatus, enteredWrite, writeGate, composerOptions;
vi.mock("../src/core/trackerCache.js", async (actual) => {
  const source = await actual();
  return { ...source, writeTasksQueryRecord: async (...args) => {
    if (blockedStatus && args[3].tasks.some((row) => row.status === blockedStatus)) {
      blockedStatus = null;
      enteredWrite = true;
      await writeGate.promise;
    }
    return source.writeTasksQueryRecord(...args);
  } };
});
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ tasks: {} }), watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/notify.js", () => ({ notifyError() {} }));
vi.mock("../src/core/taskComposer.js", () => ({
  openTaskComposer: (_host, options) => { composerOptions = options; return { isOpen: () => true, close() {} }; },
}));

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const scope = { deviceId: "move-device", projectId: "move-project" };
let pane, aggregate, fence, host, mutation, oldAnswer, rpc, paneCall, summaries, cache;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="host"></div>';
  host = document.querySelector("#host");
  blockedStatus = null; enteredWrite = false; writeGate = deferred();
  composerOptions = null;
  mutation = deferred(); oldAnswer = deferred();
  fence = await import("../src/core/taskMutationFence.js");
  const held = task({ id: "move-task", title: "Move me" });
  const tracker = await import("../src/core/trackerCache.js");
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [held], columns: columns() });
  cache = await import("../src/core/localCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  const order = await nextTaskRead();
  const ticket = await summaries.beginSummaryQuery(scope, {}, order);
  await summaries.landSummaryPage(ticket, { summary: true, basis: "before-move", next_cursor: "old-next",
    identities: {}, tasks: [{ id: held.id, project_id: scope.projectId, title: held.title, number: 1,
      state: "open", status: "backlog", review_summary: { mode: "snapshot", status: "open" } }] }, { read: order });
  paneCall = vi.fn(async (method) => {
    if (method === "tasks.list") return { tasks: [held] };
    if (method === "tasks.get") return { task: held, timeline: [] };
    if (method === "tasks.update") return mutation.promise;
    return {};
  });
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(host, { ...scope, projectKey: "move-device|move-project", defaultView: "board", callRpc: paneCall,
    feed: () => ({ items: [], projects: [], workspaces: [] }), catalog: () => ({ providers: [] }), navigate() {} });
  await vi.waitFor(() => expect(host.querySelector('[data-column="backlog"] .task-card')).not.toBeNull(), { timeout: 10_000 });
  const { createTaskAggregateReader } = await import("../src/core/taskAggregateReads.js");
  rpc = vi.fn().mockResolvedValueOnce({ project_id: scope.projectId, unfinished_open_count: 7,
    watched_unread_total: 3, agents: [], basis: "first" }).mockImplementationOnce(() => oldAnswer.promise);
  aggregate = createTaskAggregateReader({ scope, callRpc: rpc, capabilities: () => ({ counts: true }), sessionOf: () => null,
    active: () => !fence.taskMutationsPending(scope.deviceId), mutationEpoch: () => fence.taskMutationEpoch(scope.deviceId) });
  await aggregate.refresh();
});

afterEach(() => {
  pane?.dispose(); aggregate?.dispose();
  writeGate.resolve(); mutation.resolve({}); oldAnswer.resolve({});
  fence?.resetTaskMutationFences();
});

it.each(["ack", "rollback"])("fences the whole optimistic move intent through %s, including blocked cache writes", async (outcome) => {
  const reading = aggregate.refresh();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  blockedStatus = "ready";
  host.querySelector('[data-column="backlog"] .task-card')
    .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  // The intent starts synchronously before noteWritten or either cache write.
  expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
  await vi.waitFor(() => expect(enteredWrite).toBe(true));
  oldAnswer.resolve({ project_id: scope.projectId, unfinished_open_count: 99,
    watched_unread_total: 99, agents: [], basis: "before-move" });
  expect(await reading).toBe(false);
  expect((await aggregate.read()).unfinished_open_count).toBe(7);
  expect(await aggregate.refresh()).toBe(false);
  expect(rpc).toHaveBeenCalledTimes(2);
  writeGate.resolve();
  await vi.waitFor(() => expect(host.querySelector('[data-column="ready"] .task-card')).not.toBeNull());
  await vi.waitFor(() => expect(paneCall).toHaveBeenCalledWith("tasks.update", { task_id: "move-task", status: "ready" }));
  expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
  if (outcome === "rollback") {
    enteredWrite = false; writeGate = deferred(); blockedStatus = "backlog";
    mutation.reject(new Error("move refused"));
    await vi.waitFor(() => expect(enteredWrite).toBe(true));
    expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
    writeGate.resolve();
    await vi.waitFor(() => expect(host.querySelector('[data-column="backlog"] .task-card')).not.toBeNull());
  } else mutation.resolve({});
  await vi.waitFor(() => expect(fence.taskMutationsPending(scope.deviceId)).toBe(0));
});

it("projects optimistic moves and rollback into shared summaries while invalidating old memberships", async () => {
  host.querySelector('[data-column="backlog"] .task-card')
    .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  await vi.waitFor(async () => expect((await summaries.readSummaryQuery(scope, {})).tasks[0].status).toBe("ready"));
  const moved = await summaries.readSummaryQuery(scope, {});
  expect(moved.stale).toBe(true);
  expect(moved.next_cursor).toBe("old-next");
  expect(moved.tasks[0].review_summary).toEqual({ mode: "snapshot", status: "open" });
  await vi.waitFor(() => expect(paneCall).toHaveBeenCalledWith("tasks.update", { task_id: "move-task", status: "ready" }));
  mutation.reject(new Error("move refused"));
  await vi.waitFor(async () => expect((await summaries.readSummaryQuery(scope, {})).tasks[0].status).toBe("backlog"));
  await vi.waitFor(() => expect(fence.taskMutationsPending(scope.deviceId)).toBe(0));
});

it("projects an actual create action response compactly without claiming full query membership", async () => {
  host.querySelector("[data-task-new]").click();
  const created = task({ id: "new-task", number: 2, title: "A new task", body: "large body excluded",
    attachments: [{ id: "file", name: "file" }], dependencies: { blocked: true, waiting_on_count: 1,
      waiting_on: [{ task_id: "prerequisite" }] } });
  composerOptions.onFiled({ task: created }, {});
  await vi.waitFor(async () => expect((await cache.readCached(summaries.summaryAddress(scope, created.id)))?.value?.task?.title)
    .toBe(created.title));
  const projected = (await cache.readCached(summaries.summaryAddress(scope, created.id))).value.task;
  expect(projected.attachment_count).toBe(1);
  expect(projected).not.toHaveProperty("body");
  expect(projected).not.toHaveProperty("attachments");
  expect(projected.dependencies).toEqual({ blocked: true, waiting_on_count: 1 });
  const prior = await summaries.readSummaryQuery(scope, {});
  expect(prior.ids).toEqual(["move-task"]);
  expect(prior.stale).toBe(true);
  await vi.waitFor(() => expect(fence.taskMutationsPending(scope.deviceId)).toBe(0));
});
