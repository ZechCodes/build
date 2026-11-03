// @vitest-environment jsdom
import { webcrypto } from "node:crypto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createTaskChecklist } from "../src/core/taskChecklist.js";
import { resetTaskMutationFences, subscribeTaskMutations, taskMutationEpoch, taskMutationsPending,
  wrapTaskMutationCall } from "../src/core/taskMutationFence.js";

const mock = vi.hoisted(() => ({ merge: vi.fn(), support: vi.fn() }));
vi.mock("../src/core/taskContentAuthority.js", () => ({
  mergeTaskContentRecordsTogether: (...args) => mock.merge(...args),
  retireTaskContent: (task) => task,
}));
vi.mock("../src/core/taskChecklistSupport.js", () => ({ readTaskChecklistSupport: (...args) => mock.support(...args) }));
vi.mock("../src/core/trackerCache.js", () => ({
  readTaskRecord: async () => null,
  taskAddress: (deviceId, projectId, taskId) => ({ deviceId, entityId: projectId, kind: "tracker-task", sub: taskId }),
}));
const deviceId = "checklist-intent-900", source = "- [ ] Verify";
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const checklist = (taskId, call = vi.fn().mockResolvedValue({}), extra = {}) => createTaskChecklist({
  deviceId, projectId: "proj-1", taskId, call, refresh: vi.fn(), onPending: vi.fn(), onFailure: vi.fn(), ...extra,
});

beforeEach(() => {
  resetTaskMutationFences(); mock.merge.mockReset().mockResolvedValue(true); mock.support.mockReset().mockResolvedValue(true);
  vi.stubGlobal("crypto", webcrypto);
});
afterEach(() => vi.unstubAllGlobals());

it("starts pending before waiting for the write lock and before any optimistic cache write", async () => {
  const lock = deferred(), cacheWrite = deferred(), call = vi.fn().mockResolvedValue({});
  vi.stubGlobal("navigator", { locks: { request: (_key, work) => lock.promise.then(work) } });
  mock.merge.mockImplementation(() => cacheWrite.promise);
  const editor = checklist("lock", call), before = taskMutationEpoch(deviceId);
  const action = editor.press(0, true, source);
  expect(taskMutationEpoch(deviceId)).not.toBe(before); expect(taskMutationsPending(deviceId)).toBe(1);
  expect(mock.merge).not.toHaveBeenCalled(); expect(call).not.toHaveBeenCalled();
  lock.resolve(); await vi.waitFor(() => expect(mock.merge).toHaveBeenCalledTimes(1));
  expect(taskMutationsPending(deviceId)).toBe(1); expect(call).not.toHaveBeenCalled();
  cacheWrite.resolve(true); expect(await action).toBe(true);
  expect(taskMutationsPending(deviceId)).toBe(0); editor.dispose();
});

it("holds pending after rejected RPC until the optimistic body rollback finishes", async () => {
  const rpc = deferred(), rollback = deferred(), onFailure = vi.fn(), refreshed = vi.fn();
  mock.merge.mockResolvedValueOnce(true).mockImplementationOnce(() => rollback.promise);
  const call = vi.fn(() => wrapTaskMutationCall(deviceId, "tasks.update", () => rpc.promise));
  subscribeTaskMutations(deviceId, (event) => { if (!event.pending) refreshed(); });
  const editor = checklist("rollback", call, { onFailure }), action = editor.press(0, true, source);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  expect(taskMutationsPending(deviceId)).toBe(2);
  rpc.reject(new Error("stale body")); await vi.waitFor(() => expect(mock.merge).toHaveBeenCalledTimes(2));
  expect(taskMutationsPending(deviceId)).toBe(1); expect(refreshed).not.toHaveBeenCalled();
  rollback.resolve(true); expect(await action).toBe(false);
  expect(taskMutationsPending(deviceId)).toBe(0); expect(refreshed).toHaveBeenCalledTimes(1);
  expect(onFailure).toHaveBeenCalledWith(expect.objectContaining({ message: "stale body" })); editor.dispose();
});

it("keeps overlapping task checklists fenced until all saves finish", async () => {
  const one = deferred(), two = deferred();
  const first = checklist("overlap-a", () => one.promise), second = checklist("overlap-b", () => two.promise);
  const a = first.press(0, true, source), b = second.press(0, true, source);
  expect(taskMutationsPending(deviceId)).toBe(2);
  await vi.waitFor(() => expect(mock.merge).toHaveBeenCalledTimes(2));
  one.resolve({}); await a; expect(taskMutationsPending(deviceId)).toBe(1);
  two.resolve({}); await b; expect(taskMutationsPending(deviceId)).toBe(0);
  first.dispose(); second.dispose();
});

it("settles a refused write lock without dispatch or optimistic cache changes", async () => {
  const failure = new Error("lock refused"), call = vi.fn(), onFailure = vi.fn();
  vi.stubGlobal("navigator", { locks: { request: () => Promise.reject(failure) } });
  const editor = checklist("lock-refused", call, { onFailure }), action = editor.press(0, true, source);
  expect(taskMutationsPending(deviceId)).toBe(1);
  expect(await action).toBe(false); expect(taskMutationsPending(deviceId)).toBe(0);
  expect(mock.merge).not.toHaveBeenCalled(); expect(call).not.toHaveBeenCalled();
  expect(onFailure).toHaveBeenCalledWith(failure); editor.dispose();
});
