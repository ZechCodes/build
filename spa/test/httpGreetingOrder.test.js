// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { greetBridge, resetChangeEvents, watchChanges } from "../src/core/changeEvents.js";

const greeting = { api_version: "3.24.0", push_events: true, capabilities: ["changes.subscriptions"] };
const flush = async () => { for (let i = 0; i < 20; i += 1) await Promise.resolve(); };
beforeEach(() => { resetChangeEvents(); });
afterEach(() => { resetChangeEvents(); });

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

  it("fails closed on subscription refusal", async () => {
    const refresh = vi.fn(); const ready = vi.fn();
    const dispose = watchChanges({ deviceId: "device", scope: "all", kinds: ["state"], refresh });
    await expect(greetBridge(async (method) => {
      if (method === "session.hello") return greeting;
      throw Object.assign(Error("no capacity"), { error_code: "busy" });
    }, { deviceId: "device", beforeRefresh: ready })).rejects.toThrow();
    expect(ready).not.toHaveBeenCalled(); expect(refresh).not.toHaveBeenCalled();
    dispose.dispose();
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
});
