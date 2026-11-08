// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { beforeEach, expect, it, vi } from "vitest";
import * as fence from "../src/core/taskMutationFence.js";
import { createTaskAggregateReader } from "../src/core/taskAggregateReads.js";
import { adoptBridgeSelection, adoptDeviceSession, resetDeviceContexts, retireDeviceContext } from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";
import { readTaskCountsSupport, taskCountsSupportAddress } from "../src/core/taskCountsSupport.js";
import { readCached } from "../src/core/localCache.js";

const deviceId = "mutation-900";
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const call = (method, dispatch, device = deviceId) => fence.wrapTaskMutationCall(device, method, dispatch);

beforeEach(() => {
  resetDeviceContexts();
  fence.resetTaskMutationFences();
});

it("changes opaque epoch and pending count synchronously before dispatch and again at acknowledgment", async () => {
  const before = fence.taskMutationEpoch(deviceId), pending = deferred(), listener = vi.fn();
  const unsubscribe = fence.subscribeTaskMutations(deviceId, listener);
  const dispatch = vi.fn(() => {
    expect(fence.taskMutationEpoch(deviceId)).not.toBe(before);
    expect(fence.taskMutationsPending(deviceId)).toBe(1);
    return pending.promise;
  });
  const result = call("tasks.read_through", dispatch), issued = fence.taskMutationEpoch(deviceId);
  expect(result).toBe(pending.promise);
  expect(listener.mock.calls[0][0]).toMatchObject({ phase: "pending", pending: 1, epoch: issued });
  const acknowledgment = { task: { watched: true } };
  pending.resolve(acknowledgment);
  expect(await result).toBe(acknowledgment);
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
  expect(fence.taskMutationEpoch(deviceId)).not.toBe(issued);
  expect(listener.mock.calls[1][0]).toMatchObject({ phase: "settled", pending: 0 });
  unsubscribe();
  call("tasks.watch", () => ({}));
  expect(listener).toHaveBeenCalledTimes(2);
});

it("holds overlapping mutations pending until every mutation settles", async () => {
  const one = deferred(), two = deferred(), listener = vi.fn();
  fence.subscribeTaskMutations(deviceId, listener);
  const first = call("tasks.watch", () => one.promise), second = call("tasks.assign", () => two.promise);
  expect(fence.taskMutationsPending(deviceId)).toBe(2);
  one.resolve({}); await first;
  expect(fence.taskMutationsPending(deviceId)).toBe(1);
  const afterFirst = fence.taskMutationEpoch(deviceId);
  two.reject(new Error("uncertain")); await expect(second).rejects.toThrow("uncertain");
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
  expect(fence.taskMutationEpoch(deviceId)).not.toBe(afterFirst);
  expect(listener.mock.calls.map(([event]) => event.pending)).toEqual([1, 2, 1, 0]);
});

it("preserves synchronous return and throw behavior while fencing both outcomes", () => {
  const before = fence.taskMutationEpoch(deviceId), answer = {};
  expect(call("tasks.update", () => answer)).toBe(answer);
  const first = fence.taskMutationEpoch(deviceId), error = new Error("sync dispatch failed");
  expect(first).not.toBe(before);
  expect(() => call("tasks.create", () => { throw error; })).toThrow(error);
  expect(fence.taskMutationEpoch(deviceId)).not.toBe(first);
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
});

it("does not let a failing listener change dispatch, acknowledgment or rejection semantics", async () => {
  fence.subscribeTaskMutations(deviceId, () => { throw new Error("listener failed"); });
  const answer = {}, failure = new Error("wire failure");
  expect(call("tasks.update", () => answer)).toBe(answer);
  await expect(call("tasks.watch", () => Promise.reject(failure))).rejects.toBe(failure);
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
});

it("matches the bridge's supported task write methods and excludes every task read", () => {
  const effects = readFileSync("../bridge/src/api/effects.rs", "utf8");
  const writes = effects.match(/const WRITE_METHODS:.*?= &\[([\s\S]*?)\];/)[1];
  const methods = [...effects.matchAll(/"(tasks\.[^"]+)"/g)].map((match) => match[1]);
  for (const method of new Set(methods)) expect(fence.isTaskMutationMethod(method)).toBe(writes.includes(`"${method}"`));
  for (const method of ["entity.seen", "entity.dismiss", "entity.mute", "project.delete"]) expect(fence.isTaskMutationMethod(method)).toBe(true);
  for (const method of ["project.rename", "environment.rename", "environment.add_source", "environment.remove_source", "environment.update_source", "environment.sync_source", "environment.set_isolation", "environment.create", "environment.discard_unused", "environment.create_project", "project.discard_unused", "tasks.delete", "tasks.track", "tasks.dismiss", "tasks.unknown", "board.list", "thread.post", "secrets.create", "secrets.list", "secrets.revoke", "secrets.audit"]) expect(fence.isTaskMutationMethod(method)).toBe(false);
  for (const method of ["sync.head", "sync.snapshot", "sync.changes"]) expect(fence.isTaskMutationMethod(method)).toBe(false);
});

it.each(["tasks.list", "tasks.lookup", "tasks.counts", "tasks.get", "tasks.filterChoices", "tasks.waiting_on.blocks", "tasks.review.get", "tasks.review.diff", "sync.head", "sync.snapshot", "sync.changes"])(
  "does not change epoch or result semantics for read %s", async (method) => {
    const before = fence.taskMutationEpoch(deviceId), pending = deferred();
    expect(call(method, () => pending.promise)).toBe(pending.promise);
    expect(fence.taskMutationEpoch(deviceId)).toBe(before);
    expect(fence.taskMutationsPending(deviceId)).toBe(0);
    pending.resolve({}); await pending.promise;
    expect(fence.taskMutationEpoch(deviceId)).toBe(before);
  },
);

it("isolates devices and retires old settlement authority and subscriptions", async () => {
  const pending = deferred(), listener = vi.fn(), otherBefore = fence.taskMutationEpoch("other");
  fence.subscribeTaskMutations(deviceId, listener);
  const request = call("tasks.update", () => pending.promise), old = fence.taskMutationEpoch(deviceId);
  fence.retireTaskMutationFence(deviceId);
  const replacement = fence.taskMutationEpoch(deviceId);
  expect(replacement).not.toBe(old);
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
  expect(listener.mock.calls.at(-1)[0].phase).toBe("retired");
  const announcements = listener.mock.calls.length;
  pending.resolve({}); await request;
  expect(fence.taskMutationEpoch(deviceId)).toBe(replacement);
  expect(fence.taskMutationEpoch("other")).toBe(otherBefore);
  call("tasks.update", () => ({}));
  expect(listener).toHaveBeenCalledTimes(announcements);
});

it("fences real context RPCs before capability waits and preserves existing refusal", async () => {
  const session = fakeSession(deviceId), context = adoptDeviceSession(session);
  const before = fence.taskMutationEpoch(deviceId);
  const request = context.rpc("tasks.update", { task_id: "task-1", expected_body_hash: "hash" });
  expect(fence.taskMutationEpoch(deviceId)).not.toBe(before);
  expect(fence.taskMutationsPending(deviceId)).toBe(1);
  expect(session.call).not.toHaveBeenCalled();
  adoptBridgeSelection(context, { version: "3.27.0" }, { capabilities: { tasks: {} } });
  await expect(request).rejects.toThrow("Update this device's bridge to tick task checklists.");
  expect(session.call).not.toHaveBeenCalled();
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
});

it("retargets session epoch without losing overlapping pending writes, then retires context authority", async () => {
  const old = deferred(), first = fakeSession(deviceId);
  first.call = vi.fn(() => old.promise);
  const context = adoptDeviceSession(first), request = context.rpc("tasks.update", { task_id: "task-1" });
  const issued = fence.taskMutationEpoch(deviceId);
  const replacement = fakeSession(deviceId);
  expect(adoptDeviceSession(replacement)).toBe(context);
  expect(fence.taskMutationEpoch(deviceId)).not.toBe(issued);
  expect(fence.taskMutationsPending(deviceId)).toBe(1);
  await context.rpc("tasks.watch", { task_id: "task-2" });
  expect(fence.taskMutationsPending(deviceId)).toBe(1);
  retireDeviceContext(deviceId);
  const retired = fence.taskMutationEpoch(deviceId);
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
  old.resolve({}); await request;
  expect(fence.taskMutationEpoch(deviceId)).toBe(retired);
  await expect(context.rpc("tasks.update", {})).rejects.toThrow();
  expect(fence.taskMutationEpoch(deviceId)).toBe(retired);
});

it("preserves actual RPC synchronous exceptions and fences generic task Inbox mutations", async () => {
  const session = fakeSession(deviceId), failure = new Error("dispatch threw");
  session.call = vi.fn(() => { throw failure; });
  const context = adoptDeviceSession(session), before = fence.taskMutationEpoch(deviceId);
  expect(() => context.rpc("entity.dismiss", { entity_id: "task-1" })).toThrow(failure);
  expect(fence.taskMutationEpoch(deviceId)).not.toBe(before);
  expect(fence.taskMutationsPending(deviceId)).toBe(0);
});

it("blocks aggregate dispatch while actual writes are pending and rejects late reads before and after acknowledgment", async () => {
  const pendingRead = deferred(), pendingWrite = deferred(), session = fakeSession(deviceId);
  session.call = vi.fn((method) => method === "tasks.counts" ? pendingRead.promise : pendingWrite.promise);
  const context = adoptDeviceSession(session);
  const reader = createTaskAggregateReader({ scope: { deviceId, projectId: "proj-1" }, callRpc: context.rpc,
    capabilities: () => ({ counts: true }), sessionOf: () => context.session,
    active: () => context.active() && !fence.taskMutationsPending(deviceId), mutationEpoch: () => fence.taskMutationEpoch(deviceId) });
  const read = reader.refresh(); await vi.waitFor(() => expect(session.call).toHaveBeenCalledTimes(1));
  const write = context.rpc("tasks.watch", { task_id: "task-1" });
  expect(await reader.refresh()).toBe(false);
  const during = fence.taskMutationEpoch(deviceId);
  pendingWrite.resolve({}); await write;
  expect(fence.taskMutationEpoch(deviceId)).not.toBe(during);
  pendingRead.resolve({ project_id: "proj-1", unfinished_open_count: 1, watched_unread_total: 3, agents: [], basis: "opaque" });
  expect(await read).toBe(false);
  expect(await reader.read()).toBeNull();
  expect(session.call).toHaveBeenCalledTimes(2);
});

it("persists current greeting count support for cache rendering across downgrade and retirement", async () => {
  const selected = `${deviceId}-support`, context = adoptDeviceSession(fakeSession(selected));
  adoptBridgeSelection(context, { version: "3.27.0" }, { capabilities: { tasks: { counts: true } } });
  await vi.waitFor(async () => expect(await readTaskCountsSupport(selected)).toBe(true));
  adoptDeviceSession(fakeSession(selected));
  adoptBridgeSelection(context, { version: "3.26.0" }, { capabilities: { tasks: {} } });
  await vi.waitFor(async () => expect((await readCached(taskCountsSupportAddress(selected)))?.value).toEqual({ counts: false, everCounts: true }));
  retireDeviceContext(selected);
  expect(await readTaskCountsSupport(selected)).toBe(true);
});

it("cannot publish support from a greeting replaced before its cache transaction", async () => {
  const selected = `${deviceId}-superseded-support`, context = adoptDeviceSession(fakeSession(selected));
  adoptBridgeSelection(context, { version: "3.27.0" }, { capabilities: { tasks: { counts: true } } });
  adoptDeviceSession(fakeSession(selected));
  adoptBridgeSelection(context, { version: "3.26.0" }, { capabilities: { tasks: {} } });
  await vi.waitFor(async () => expect((await readCached(taskCountsSupportAddress(selected)))?.value).toEqual({ counts: false, everCounts: false }));
  expect(await readTaskCountsSupport(selected)).toBe(false);
});

// Vault and conversation metadata cannot invalidate task projections.
it.each(["secrets.create", "secrets.list", "secrets.revoke", "secrets.audit"])(
  "leaves task authority untouched for %s", async (method) => {
    const before = fence.taskMutationEpoch(deviceId), pending = deferred();
    expect(call(method, () => pending.promise)).toBe(pending.promise);
    expect(fence.taskMutationEpoch(deviceId)).toBe(before);
    expect(fence.taskMutationsPending(deviceId)).toBe(0);
    pending.resolve({}); await pending.promise;
    expect(fence.taskMutationEpoch(deviceId)).toBe(before);
  },
);
