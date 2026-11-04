import { afterEach, expect, it, vi } from "vitest";
import { createScopedSyncRegistry, projectSyncScope, taskSyncScope, MAX_TRACKED_SYNC_SCOPES } from "../src/core/scopedSyncRegistry.js";
const registries = [];
function registry(options) { const value = createScopedSyncRegistry(options); registries.push(value); return value; }
afterEach(() => { registries.splice(0).forEach((value) => value.dispose()); vi.useRealTimers(); });
const task = (taskId, deviceId = "d") => ({ deviceId, scope: taskSyncScope(taskId), projectId: "p" });
it("canonicalizes normalized filters, order and exact task identities", () => {
  expect(projectSyncScope("p", { state: "open", labels: ["UI", "ui"], order: "number" }))
    .toEqual(projectSyncScope("p", { states: ["open"], label: "ui" }));
  expect(projectSyncScope("p", { order: "priority" })).toMatchObject({ filters: {}, order: "priority" });
  expect(taskSyncScope("TaskCase")).toEqual({ kind: "task", task_id: "TaskCase" });
});
it("retains duplicate mounts until both close, with independent devices and definitions", () => {
  const scopes = registry(); const one = scopes.register(task("a")), two = scopes.register(task("a"));
  scopes.register(task("a", "other")); scopes.register(task("b"));
  expect(scopes.tracked("d")).toHaveLength(2); one(); expect(scopes.tracked("d")).toHaveLength(2);
  two(); expect(scopes.tracked("d").map(({ scope }) => scope.task_id)).toEqual(["b"]);
  expect(scopes.tracked("other")).toHaveLength(1);
});
it("stops tracking least recently opened scopes at the hard per-device cap", () => {
  const scopes = registry();
  for (let index = 0; index <= MAX_TRACKED_SYNC_SCOPES; index += 1) scopes.register(task(String(index)));
  expect(scopes.tracked("d")).toHaveLength(MAX_TRACKED_SYNC_SCOPES);
  expect(scopes.tracked("d").some(({ scope }) => scope.task_id === "0")).toBe(false);
  expect(scopes.tracked("d").at(-1).scope.task_id).toBe(String(MAX_TRACKED_SYNC_SCOPES));
});
it("shares mounted scopes with the lock holder in another tab and releases them on close", () => {
  const channels = [];
  const channel = () => { const c = { onmessage: null, postMessage(data) { channels.filter((other) => other !== c)
    .forEach((other) => other.onmessage?.({ data: structuredClone(data) })); }, close() {} }; channels.push(c); return c; };
  const first = registry({ channel: channel() }), second = registry({ channel: channel() });
  const stop = second.register(task("follower"));
  expect(first.tracked("d").map(({ scope }) => scope.task_id)).toEqual(["follower"]);
  stop(); expect(first.tracked("d")).toHaveLength(0);
});
it("answers a new holder's presence request and expires vanished tab registrations", async () => {
  vi.useFakeTimers(); const channels = [];
  const channel = () => { const c = { onmessage: null, postMessage(data) { channels.filter((other) => other !== c)
    .forEach((other) => other.onmessage?.({ data: structuredClone(data) })); }, close() { channels.splice(channels.indexOf(c), 1); } }; channels.push(c); return c; };
  const first = registry({ channel: channel() }); first.register(task("existing"));
  const second = registry({ channel: channel() }); expect(second.tracked("d")).toHaveLength(1);
  // A crashed tab cannot announce release or refresh its lease.
  const owner = channels[0]; owner.postMessage = () => {};
  await vi.advanceTimersByTimeAsync(20_000); expect(second.tracked("d")).toHaveLength(0);
});
it("notifies only membership changes, not heartbeats", async () => {
  vi.useFakeTimers(); const scopes = registry(); const changed = vi.fn(); const stop = scopes.subscribe(changed);
  const close = scopes.register(task("a")); expect(changed).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(5_000); expect(changed).toHaveBeenCalledTimes(1);
  close(); expect(changed).toHaveBeenCalledTimes(2); stop();
});
it("forwards scope wake hints to another tab without records or rebroadcast loops", () => {
  const channels = [], sent = [];
  const channel = () => { const c = { onmessage: null, postMessage(data) { sent.push(data);
    channels.filter((other) => other !== c).forEach((other) => other.onmessage?.({ data: structuredClone(data) })); }, close() {} };
    channels.push(c); return c; };
  const holder = registry({ channel: channel() }), follower = registry({ channel: channel() });
  follower.register(task("follower"));
  const notified = vi.fn(), followerNotified = vi.fn();
  holder.subscribeWake(notified); follower.subscribeWake(followerNotified);
  sent.length = 0;
  follower.wake("d", [taskSyncScope("follower")]);
  expect(notified).toHaveBeenCalledExactlyOnceWith({ deviceId: "d", scopes: [taskSyncScope("follower")] });
  expect(followerNotified).not.toHaveBeenCalled();
  expect(sent.map((item) => item.type)).toEqual(["presence", "wake"]);
  expect(sent[1]).toEqual({ type: "wake", owner: expect.any(String), deviceId: "d", scopes: [taskSyncScope("follower")] });
});
it("renews a frozen follower's expired presence before delivering its foreground wake", async () => {
  vi.useFakeTimers();
  const channels = [];
  const channel = () => { const c = { onmessage: null, postMessage(data) {
    channels.filter((other) => other !== c).forEach((other) => other.onmessage?.({ data: structuredClone(data) })); }, close() {} };
    channels.push(c); return c; };
  const holder = registry({ channel: channel() }), follower = registry({ channel: channel() });
  follower.register(task("resumed"));
  const send = channels[1].postMessage;
  channels[1].postMessage = () => {};
  await vi.advanceTimersByTimeAsync(20_000);
  expect(holder.tracked("d")).toHaveLength(0);
  channels[1].postMessage = send;
  const seen = [];
  holder.subscribeWake(() => seen.push(holder.tracked("d").map(({ scope }) => scope.task_id)));
  follower.wake("d", [taskSyncScope("resumed")]);
  expect(seen).toEqual([["resumed"]]);
});


it("keeps a continuous registration across duplicate mounts and renews it after last close", () => {
  const scopes = registry(); const closeFirst = scopes.register(task("a"));
  const registration = scopes.tracked("d")[0].registration;
  expect(registration).toBeTruthy();
  const closeSecond = scopes.register(task("a")); closeFirst();
  expect(scopes.tracked("d")[0].registration).toBe(registration);
  closeSecond(); scopes.register(task("a"));
  expect(scopes.tracked("d")[0].registration).not.toBe(registration);
});

it("starts presence on the first local mount and stops it on the final release", () => {
  vi.useFakeTimers();
  const scopes = registry();
  expect(vi.getTimerCount()).toBe(0);
  const first = scopes.register(task("a")), second = scopes.register(task("b"));
  expect(vi.getTimerCount()).toBe(1);
  first(); expect(vi.getTimerCount()).toBe(1);
  second(); expect(vi.getTimerCount()).toBe(0);
});

it("keeps remote lease expiry running for observers without a local mount", async () => {
  vi.useFakeTimers();
  const channel = { onmessage: null, postMessage: vi.fn(), close: vi.fn() };
  const scopes = registry({ channel });
  const changed = vi.fn(), stop = scopes.subscribe(changed);
  expect(vi.getTimerCount()).toBe(1);
  channel.onmessage({ data: { type: "presence", owner: "remote", scopes: [{ ...task("remote"),
    id: "remote:1", key: JSON.stringify(["d", taskSyncScope("remote")]), opened: Date.now(), ordinal: 1 }] } });
  expect(scopes.tracked("d")).toHaveLength(1);
  changed.mockClear();
  await vi.advanceTimersByTimeAsync(15_000);
  expect(scopes.tracked("d")).toHaveLength(0);
  expect(changed).toHaveBeenCalledOnce();
  stop(); expect(vi.getTimerCount()).toBe(0);
});
