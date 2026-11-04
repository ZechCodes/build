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
  first.subscribe(vi.fn());
  const stop = second.register(task("follower"));
  expect(first.tracked("d").map(({ scope }) => scope.task_id)).toEqual(["follower"]);
  stop(); expect(first.tracked("d")).toHaveLength(0);
});
it("answers a new holder's presence request and expires vanished tab registrations", async () => {
  vi.useFakeTimers(); const channels = [];
  const channel = () => { const c = { onmessage: null, postMessage(data) { channels.filter((other) => other !== c)
    .forEach((other) => other.onmessage?.({ data: structuredClone(data) })); }, close() { channels.splice(channels.indexOf(c), 1); } }; channels.push(c); return c; };
  const first = registry({ channel: channel() }); first.register(task("existing"));
  const second = registry({ channel: channel() }); second.subscribe(vi.fn()); expect(second.tracked("d")).toHaveLength(1);
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


it("uses independent local mount ids without cursor-authority tokens", () => {
  const scopes = registry(); const closeFirst = scopes.register(task("a"));
  const first = scopes.tracked("d")[0];
  expect(first).not.toHaveProperty("registration");
  expect(first).not.toHaveProperty("registrationOpened");
  const closeSecond = scopes.register(task("a")); closeFirst();
  const second = scopes.tracked("d")[0];
  expect(second.id).not.toBe(first.id);
  expect(second.key).toBe(first.key);
  closeSecond(); scopes.register(task("a"));
  expect(scopes.tracked("d")[0].id).not.toBe(second.id);
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

it("opens channel factories only for active scopes or observers and closes them when idle", () => {
  vi.useFakeTimers();
  const channels = [], createChannel = vi.fn(() => {
    const channel = { onmessage: null, postMessage: vi.fn(), close: vi.fn() };
    channels.push(channel); return channel;
  });
  const scopes = registry({ createChannel });
  expect(createChannel).not.toHaveBeenCalled();
  const stop = scopes.subscribe(vi.fn());
  expect(createChannel).toHaveBeenCalledOnce();
  expect(channels[0].postMessage).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ type: "request" }));
  stop(); expect(channels[0].close).toHaveBeenCalledOnce(); expect(channels[0].onmessage).toBeNull();
  const release = scopes.register(task("reopened"));
  expect(createChannel).toHaveBeenCalledTimes(2);
  expect(channels[1].onmessage).toEqual(expect.any(Function));
  release(); expect(channels[1].close).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it("reopens a lazy holder channel and rediscovers remote presence before forwarding wakes", () => {
  const channels = [];
  const createChannel = () => {
    const channel = { onmessage: null,
      postMessage(data) { channels.filter((other) => other !== channel).forEach((other) => other.onmessage?.({ data: structuredClone(data) })); },
      close() { channels.splice(channels.indexOf(channel), 1); } };
    channels.push(channel); return channel;
  };
  const holder = registry({ createChannel }), follower = registry({ createChannel });
  const wake = vi.fn(), stop = holder.subscribeWake(wake);
  const release = follower.register(task("remote"));
  expect(holder.tracked("d").map(({ scope }) => scope.task_id)).toEqual(["remote"]);
  stop(); expect(channels).toHaveLength(1);
  const releaseSecond = follower.register(task("while-holder-idle"));
  const stopAgain = holder.subscribeWake(wake);
  expect(holder.tracked("d").map(({ scope }) => scope.task_id)).toEqual(["remote", "while-holder-idle"]);
  follower.wake("d", [taskSyncScope("remote")]);
  expect(wake).toHaveBeenCalledExactlyOnceWith({ deviceId: "d", scopes: [taskSyncScope("remote")] });
  release(); releaseSecond();
  expect(holder.tracked("d")).toHaveLength(0);
  stopAgain(); expect(channels).toHaveLength(0);
});

function asynchronousChannels() {
  const channels = new Set(), pending = [], sent = [];
  const createChannel = () => {
    const channel = { onmessage: null,
      postMessage(data) {
        sent.push(structuredClone(data));
        for (const peer of channels) if (peer !== channel) {
          const copy = structuredClone(data);
          pending.push(() => { if (channels.has(peer)) peer.onmessage?.({ data: copy }); });
        }
      },
      close() { channels.delete(channel); },
    };
    channels.add(channel); return channel;
  };
  const flush = async () => {
    let delivered = 0;
    while (pending.length) {
      if (++delivered > 100) throw new Error("Presence messages did not converge");
      await Promise.resolve(); pending.shift()();
    }
  };
  const inject = (data) => {
    for (const channel of channels) {
      const copy = structuredClone(data);
      pending.push(() => { if (channels.has(channel)) channel.onmessage?.({ data: copy }); });
    }
  };
  return { createChannel, flush, sent, inject };
}

it("treats delayed asynchronous legacy presence as scheduling metadata without adopting or rebroadcasting its token", async () => {
  vi.useFakeTimers();
  const bus = asynchronousChannels(); let clock = 100;
  const first = registry({ createChannel: bus.createChannel, now: () => clock });
  const second = registry({ createChannel: bus.createChannel, now: () => clock });
  const closeFirst = first.register(task("shared"));
  const original = first.tracked("d")[0];
  clock = 200;
  const closeSecond = second.register(task("shared"));
  const secondMount = second.tracked("d")[0];
  await bus.flush();
  const beforeLegacy = bus.sent.length;
  bus.inject({ type: "presence", owner: original.owner,
    scopes: [{ ...original, registration: "legacy-resume-token", registrationOpened: -100 }] });
  await bus.flush();
  expect(bus.sent).toHaveLength(beforeLegacy);
  for (const entry of [...first.tracked("d"), ...second.tracked("d")]) {
    expect(entry).not.toHaveProperty("registration");
    expect(entry).not.toHaveProperty("registrationOpened");
  }
  closeFirst(); await bus.flush();
  expect(second.tracked("d")[0]).toMatchObject({ id: secondMount.id, key: secondMount.key, opened: 200 });
  closeSecond(); await bus.flush();
  const closeReopened = second.register(task("shared"));
  expect(second.tracked("d")[0].id).not.toBe(secondMount.id);
  expect(second.tracked("d")[0]).not.toHaveProperty("registration");
  closeReopened();
});

it("prunes expired legacy presence before local lookup and does not reuse its authority when it arrives late", () => {
  vi.useFakeTimers();
  let clock = 0;
  const channel = { onmessage: null, postMessage: vi.fn(), close: vi.fn() };
  const scopes = registry({ channel, now: () => clock });
  scopes.subscribe(vi.fn());
  const legacy = { type: "presence", owner: "legacy-tab", scopes: [{ ...task("shared"),
    id: "legacy-tab:1", owner: "legacy-tab", key: JSON.stringify(["d", taskSyncScope("shared")]),
    opened: -1, ordinal: 1, registration: "expired-token", registrationOpened: -1 }] };
  channel.onmessage({ data: legacy });
  expect(scopes.tracked("d")).toHaveLength(1);
  expect(scopes.tracked("d")[0]).not.toHaveProperty("registration");
  expect(scopes.tracked("d")[0]).not.toHaveProperty("registrationOpened");
  clock = 16_000;
  expect(scopes.tracked("d")).toEqual([]);
  scopes.register(task("shared"));
  const local = scopes.tracked("d")[0], sentBefore = channel.postMessage.mock.calls.length;
  channel.onmessage({ data: legacy });
  expect(channel.postMessage).toHaveBeenCalledTimes(sentBefore);
  expect(scopes.tracked("d")[0]).toMatchObject({ id: local.id, key: local.key, opened: 16_000 });
  expect(scopes.tracked("d")[0]).not.toHaveProperty("registration");
  expect(scopes.tracked("d")[0]).not.toHaveProperty("registrationOpened");
});

it("keeps backwards-clock mount metadata independent from asynchronous remote cursor tokens", async () => {
  vi.useFakeTimers();
  const bus = asynchronousChannels(); let clock = 100;
  const first = registry({ createChannel: bus.createChannel, now: () => clock });
  const second = registry({ createChannel: bus.createChannel, now: () => clock });
  const closeFirst = first.register(task("shared"));
  const original = first.tracked("d")[0];
  clock = -100;
  const closeSecond = second.register(task("shared"));
  const local = second.tracked("d")[0];
  await bus.flush();
  bus.inject({ type: "presence", owner: original.owner,
    scopes: [{ ...original, registration: "old-clock-token", registrationOpened: -1000 }] });
  await bus.flush();
  closeFirst(); await bus.flush();
  expect(second.tracked("d")[0]).toMatchObject({ id: local.id, key: local.key, opened: -100 });
  expect(second.tracked("d")[0]).not.toHaveProperty("registration");
  expect(second.tracked("d")[0]).not.toHaveProperty("registrationOpened");
  for (const { scopes } of bus.sent.filter(({ type }) => type === "presence")) {
    for (const entry of scopes) {
      expect(entry).not.toHaveProperty("registration"); expect(entry).not.toHaveProperty("registrationOpened");
    }
  }
  closeSecond(); await bus.flush();
});
