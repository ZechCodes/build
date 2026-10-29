// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import * as inbox from "../src/core/inboxView.js";
import { resetTaskMutationFences, subscribeTaskMutations, taskMutationEpoch, taskMutationsPending,
  wrapTaskMutationCall } from "../src/core/taskMutationFence.js";

const notify = vi.hoisted(() => vi.fn());
vi.mock("../src/core/notify.js", async (original) => ({ ...await original(), notifyError: notify }));
const deviceId = "inbox-intent-900";
const entry = (key) => ({ key, deviceId });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

beforeEach(() => { resetTaskMutationFences(); notify.mockClear(); });

it("fences before a blocked optimistic cache write and keeps pending through rejected RPC rollback", async () => {
  const cacheWrite = deferred(), rpc = deferred(), rollback = deferred(), countRefresh = vi.fn();
  const undo = vi.fn(() => rollback.promise), call = vi.fn(() => wrapTaskMutationCall(deviceId, "tasks.unwatch", () => rpc.promise));
  const before = taskMutationEpoch(deviceId), events = [];
  subscribeTaskMutations(deviceId, (event) => { events.push(event.pending); if (!event.pending) countRefresh(); });
  const action = inbox.optimisticVerb(entry("blocked-unwatch"), { write: () => cacheWrite.promise, call, failureSummary: "Unwatch failed" });
  expect(taskMutationEpoch(deviceId)).not.toBe(before);
  expect(taskMutationsPending(deviceId)).toBe(1);
  expect(call).not.toHaveBeenCalled();
  cacheWrite.resolve(undo);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  expect(taskMutationsPending(deviceId)).toBe(2);
  rpc.reject(new Error("offline"));
  await vi.waitFor(() => expect(undo).toHaveBeenCalledTimes(1));
  expect(taskMutationsPending(deviceId)).toBe(1);
  expect(countRefresh).not.toHaveBeenCalled();
  rollback.resolve(); await action;
  expect(taskMutationsPending(deviceId)).toBe(0);
  expect(countRefresh).toHaveBeenCalledTimes(1);
  expect(events).toEqual([1, 2, 1, 0]);
  expect(notify).toHaveBeenCalledWith("Unwatch failed", "offline");
});

it("holds independent overlapping row actions until both optimistic intents settle", async () => {
  const first = deferred(), second = deferred(), refreshed = vi.fn();
  subscribeTaskMutations(deviceId, (event) => { if (!event.pending) refreshed(); });
  const one = inbox.optimisticVerb(entry("overlap-one"), { write: async () => vi.fn(), call: () => first.promise });
  const two = inbox.optimisticVerb(entry("overlap-two"), { write: async () => vi.fn(), call: () => second.promise });
  expect(taskMutationsPending(deviceId)).toBe(2);
  first.resolve({}); await one;
  expect(taskMutationsPending(deviceId)).toBe(1); expect(refreshed).not.toHaveBeenCalled();
  second.resolve({}); await two;
  expect(taskMutationsPending(deviceId)).toBe(0); expect(refreshed).toHaveBeenCalledTimes(1);
});

it("settles failed cache writes and allows the same row to be acted on again", async () => {
  const selected = entry("failed-cache"), failure = new Error("cache refused"), call = vi.fn();
  await expect(inbox.optimisticVerb(selected, { write: () => Promise.reject(failure), call })).rejects.toBe(failure);
  expect(taskMutationsPending(deviceId)).toBe(0); expect(call).not.toHaveBeenCalled();
  await inbox.optimisticVerb(selected, { write: async () => vi.fn(), call });
  expect(call).toHaveBeenCalledTimes(1);
});

it("retains intent authority through rollback after a synchronous call exception", async () => {
  const rollback = deferred(), undo = vi.fn(() => rollback.promise);
  const action = inbox.optimisticVerb(entry("sync-call"), { write: async () => undo,
    call: () => { throw new Error("call refused"); }, failureSummary: "Action failed" });
  await vi.waitFor(() => expect(undo).toHaveBeenCalledTimes(1));
  expect(taskMutationsPending(deviceId)).toBe(1);
  rollback.resolve(); await action;
  expect(taskMutationsPending(deviceId)).toBe(0);
});
