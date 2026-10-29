// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { columns, comment, task } from "./trackerWireFixture.js";
import { wipeCache } from "../src/core/localCache.js";
import { taskMutationEpoch, taskMutationsPending, resetTaskMutationFences } from "../src/core/taskMutationFence.js";
import { writeTaskRecord, writeTasksRecord } from "../src/core/trackerCache.js";
import { mountTaskPage } from "../src/core/trackerTaskPage.js";

const observation = vi.hoisted(() => ({ carries: false, advanceRead: null }));
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] } }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/trackerWatch.js", async (original) => ({ ...await original(), carriesWatching: () => observation.carries }));
vi.mock("../src/core/trackerCache.js", async (original) => {
  const cache = await original();
  return { ...cache, advanceTaskReadThrough: (...args) => observation.advanceRead
    ? observation.advanceRead(...args) : cache.advanceTaskReadThrough(...args) };
});
vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));

const deviceId = "task-page-intent-900", projectId = "proj-1", taskId = "task-intent";
const wire = { task: task({ id: taskId, number: 12, watched: false, trackers: [] }),
  timeline: [comment({ id: "tc-01", task_id: taskId, created_at: "2026-09-21T10:01:00Z", body: "Newest comment" })] };
let page, host;
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const mount = async (callRpc) => {
  page = mountTaskPage(host, { deviceId, projectId, taskId, projectKey: `${deviceId}|${projectId}`,
    callRpc, catalog: () => ({ providers: [] }), feed: () => ({ items: [], projects: [], workspaces: [] }) });
  await vi.waitFor(() => expect(host.querySelector(".rail-watch")).not.toBeNull());
};

beforeEach(async () => {
  await wipeCache(); resetTaskMutationFences();
  observation.carries = false; observation.advanceRead = null;
  document.body.innerHTML = '<div id="task-intent-host"></div>';
  host = document.querySelector("#task-intent-host");
  await writeTaskRecord(deviceId, projectId, taskId, wire);
  await writeTasksRecord(deviceId, projectId, { tasks: [wire.task], columns: columns() });
});
afterEach(() => { page?.dispose(); vi.restoreAllMocks(); });

it("fences before optimistic watch painting and retains pending through refusal rollback", async () => {
  const pending = deferred(), call = vi.fn((method) => method === "tasks.get" ? Promise.resolve(wire) : pending.promise);
  await mount(call);
  const button = host.querySelector(".rail-watch"), before = taskMutationEpoch(deviceId), painted = [];
  const original = button.setAttribute;
  vi.spyOn(button, "setAttribute").mockImplementation(function (name, value) {
    if (name === "aria-pressed") painted.push({ value: String(value), pending: taskMutationsPending(deviceId) });
    return original.call(this, name, value);
  });
  button.click();
  expect(taskMutationEpoch(deviceId)).not.toBe(before); expect(taskMutationsPending(deviceId)).toBe(1);
  expect(painted[0]).toEqual({ value: "true", pending: 1 });
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.watch", { task_id: taskId }));
  pending.reject(new Error("watch refused"));
  await vi.waitFor(() => expect(taskMutationsPending(deviceId)).toBe(0));
  expect(painted).toContainEqual({ value: "false", pending: 1 });
});

it("retains optimistic read intent through acknowledgment and guarded cache advancement", async () => {
  observation.carries = true;
  const pending = deferred(), cacheUpdate = deferred(), before = taskMutationEpoch(deviceId);
  observation.advanceRead = vi.fn(() => cacheUpdate.promise);
  const call = vi.fn((method) => method === "tasks.get" ? Promise.resolve(wire) : pending.promise);
  await mount(call);
  await vi.waitFor(() => expect(call.mock.calls.some(([method]) => method === "tasks.read_through")).toBe(true));
  expect(taskMutationEpoch(deviceId)).not.toBe(before); expect(taskMutationsPending(deviceId)).toBe(1);
  pending.resolve({}); await vi.waitFor(() => expect(observation.advanceRead).toHaveBeenCalledTimes(1));
  expect(taskMutationsPending(deviceId)).toBe(1);
  cacheUpdate.resolve(); await vi.waitFor(() => expect(taskMutationsPending(deviceId)).toBe(0));
});
