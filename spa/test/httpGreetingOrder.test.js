// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchChangeEvent, greetBridge, resetChangeEvents, subscriptionsSettled, watchChanges } from "../src/core/changeEvents.js";
import { clearConnectionDiagnosticHistory, connectionDiagnosticHistory } from "../src/core/connectionDiagnostics.js";

const greeting = { api_version: "3.24.0", push_events: true, capabilities: ["changes.subscriptions"] };
const flush = async () => { for (let i = 0; i < 20; i += 1) await Promise.resolve(); };
beforeEach(() => { resetChangeEvents(); clearConnectionDiagnosticHistory(); });
afterEach(() => { resetChangeEvents(); vi.useRealTimers(); });

describe("HTTP greeting subscribes before refresh", () => {
  it("holds ordinary refresh until logical subscriptions settle", async () => {
    let accept;
    const refresh = vi.fn(); const ready = vi.fn();
    const dispose = watchChanges({ deviceId: "device", scope: "all", kinds: ["state"], refresh });
    const call = vi.fn((method) => method === "session.hello" ? Promise.resolve(greeting) : new Promise((resolve) => { accept = resolve; }));
    const pending = greetBridge(call, { deviceId: "device", beforeRefresh: ready });
    await flush();
    expect(call).toHaveBeenCalledWith("changes.subscribe", expect.anything());
    expect(refresh).not.toHaveBeenCalled();
    expect(ready).not.toHaveBeenCalled();
    accept({}); await pending;
    expect(ready).toHaveBeenCalledOnce();
    expect(refresh).toHaveBeenCalledOnce();
    dispose.dispose();
  });

  it("fails closed on a permanent subscription refusal", async () => {
    const refresh = vi.fn(); const ready = vi.fn();
    const dispose = watchChanges({ deviceId: "device", scope: "all", kinds: ["state"], refresh });
    await expect(greetBridge(async (method) => {
      if (method === "session.hello") return greeting;
      throw Object.assign(Error("unknown kind"), { error_code: "invalid_params" });
    }, { deviceId: "device", beforeRefresh: ready })).rejects.toThrow();
    expect(ready).not.toHaveBeenCalled(); expect(refresh).not.toHaveBeenCalled();
    dispose.dispose();
  });

  it("holds a slot for the mounted thread on a bridge with the old eight-slot limit", async () => {
    const held = new Map();
    const call = vi.fn(async (method, spec) => {
      if (method === "session.hello") return greeting;
      if (method === "changes.unsubscribe") return held.delete(spec.subscription_id);
      if (!held.has(spec.subscription_id) && held.size === 8) throw Object.assign(Error("no capacity"), { error_code: "busy" });
      held.set(spec.subscription_id, spec);
    });
    for (let index = 0; index < 10; index += 1) {
      watchChanges({ id: `source-${index}`, deviceId: "device", scope: "all", kinds: ["terminals"], refresh() {} });
    }
    await greetBridge(call, { deviceId: "device", beforeRefresh() {} });
    expect(held.size).toBe(7);
    const thread = (entity) => watchChanges({ id: "s-thread", deviceId: "device", entity, kinds: ["thread"], refresh() {} });
    const first = thread("first");
    await subscriptionsSettled();
    expect(held.get("s-thread")?.scope).toEqual({ kind: "entity", id: "first" });
    first.dispose();
    thread("second");
    await subscriptionsSettled();
    expect(held.get("s-thread")?.scope).toEqual({ kind: "entity", id: "second" });
    expect(held.size).toBe(8);
    expect(call.mock.calls).not.toContainEqual(["changes.unsubscribe", { subscription_id: "s-thread" }]);
  });

  it("retains a busy HTTP subscription and catches up after a later push triggers retry", async () => {
    vi.useFakeTimers();
    const ready = vi.fn();
    const catchUp = vi.fn();
    let blocked = true;
    const call = vi.fn(async (method, spec) => {
      if (method === "session.hello") return greeting;
      if (spec.subscription_id === "s-thread" && blocked) throw Object.assign(Error("no capacity"), { error_code: "busy" });
    });
    watchChanges({ id: "inbox", deviceId: "device", scope: "all", kinds: ["state"], refresh() {} });
    watchChanges({ id: "s-thread", deviceId: "device", entity: "thread", kinds: ["thread"], refresh() {}, catchUp });
    await greetBridge(call, { deviceId: "device", beforeRefresh: ready });
    expect(ready).toHaveBeenCalledOnce();
    expect(catchUp).not.toHaveBeenCalled();
    expect(connectionDiagnosticHistory()).toContainEqual(expect.objectContaining({
      event: "subscription", state: "refused", subscription: "s-thread", code: "busy",
    }));
    blocked = false;
    dispatchChangeEvent({ type: "changes", subscription_id: "inbox", items: [{ entity_id: "thread", state: {} }] }, "device");
    await subscriptionsSettled();
    expect(catchUp).toHaveBeenCalledOnce();
  });

  it("retries after a slot is released and gives an otherwise quiet refusal only one delayed retry", async () => {
    vi.useFakeTimers();
    const held = new Set();
    const freed = vi.fn();
    const waiting = watchChanges({ id: "waiting", deviceId: "device", scope: "all", kinds: ["state"], refresh() {} });
    watchChanges({ id: "contender", deviceId: "device", scope: "all", kinds: ["state"], refresh() {}, catchUp: freed });
    const call = vi.fn(async (method, spec) => {
      if (method === "session.hello") return greeting;
      if (method === "changes.unsubscribe") return held.delete(spec.subscription_id);
      if (held.size && !held.has(spec.subscription_id)) throw Object.assign(Error("no capacity"), { error_code: "busy" });
      held.add(spec.subscription_id);
    });
    await greetBridge(call, { deviceId: "device", beforeRefresh() {} });
    const attempts = () => call.mock.calls.filter(([method, p]) => method === "changes.subscribe" && p.subscription_id === "contender");
    expect(attempts()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(5_000);
    const beforeRelease = attempts().length;
    expect(beforeRelease).toBeGreaterThan(1);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(attempts()).toHaveLength(beforeRelease);
    waiting.dispose();
    await subscriptionsSettled();
    expect(held).toEqual(new Set(["contender"]));
    expect(attempts()).toHaveLength(beforeRelease + 1);
    expect(freed).toHaveBeenCalledOnce();
  });

  it("does not release or refresh after its opening is retired", async () => {
    let accept; let active = true;
    const refresh = vi.fn(); const ready = vi.fn();
    const dispose = watchChanges({ deviceId: "device", scope: "all", kinds: ["state"], refresh });
    const pending = greetBridge((method) => method === "session.hello" ? Promise.resolve(greeting) : new Promise((resolve) => { accept = resolve; }), {
      deviceId: "device", isCurrent: () => active, beforeRefresh: ready,
    });
    await flush(); active = false; accept({}); await pending;
    expect(ready).not.toHaveBeenCalled(); expect(refresh).not.toHaveBeenCalled();
    dispose.dispose();
  });

  it("retains the existing WebRTC default refresh timing", async () => {
    const refresh = vi.fn(); let accept;
    const dispose = watchChanges({ deviceId: "device", scope: "all", kinds: ["state"], refresh });
    await greetBridge((method) => method === "session.hello" ? Promise.resolve(greeting) : new Promise((resolve) => { accept = resolve; }), { deviceId: "device" });
    expect(refresh).toHaveBeenCalledOnce();
    await flush(); accept({}); dispose.dispose();
  });

  it("cannot release a replacement through an old opening's subscription completion", async () => {
    let finishOld;
    let oldCurrent = true;
    const oldReady = vi.fn(); const nextReady = vi.fn(); const refresh = vi.fn();
    const watcher = watchChanges({ deviceId: "device", scope: "all", kinds: ["state"], refresh });
    const oldGreeting = greetBridge((method) => method === "session.hello" ? Promise.resolve(greeting) : new Promise((resolve) => { finishOld = resolve; }), {
      deviceId: "device", isCurrent: () => oldCurrent, beforeRefresh: oldReady,
    });
    await flush(); oldCurrent = false;
    const nextGreeting = greetBridge(async (method) => method === "session.hello" ? greeting : {}, { deviceId: "device", beforeRefresh: nextReady });
    await flush(); finishOld({});
    await Promise.all([oldGreeting, nextGreeting]);
    expect(oldReady).not.toHaveBeenCalled();
    expect(nextReady).toHaveBeenCalledOnce();
    expect(refresh).toHaveBeenCalledOnce();
    watcher.dispose();
  });
});
