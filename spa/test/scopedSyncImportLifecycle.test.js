import { afterEach, beforeEach, expect, it, vi } from "vitest";

let runtime, release, channelConstructor, messages, channels;
beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
  channels = []; messages = [];
  channelConstructor = vi.fn(function(name) {
    this.name = name; this.onmessage = null;
    this.postMessage = vi.fn((data) => messages.push({ name, data }));
    this.close = vi.fn(); channels.push(this);
  });
  vi.stubGlobal("BroadcastChannel", channelConstructor);
});
afterEach(() => {
  runtime?.stopScopedSyncRuntime();
  release?.();
  runtime = null; release = null;
  vi.useRealTimers(); vi.unstubAllGlobals();
});

const presenceChannels = () => channels.filter((channel) => channel.name === "build-scoped-sync-presence");
const presenceMessages = () => messages.filter((message) => message.name === "build-scoped-sync-presence").map(({ data }) => data);

it("imports the scoped sync runtime without constructing scoped-sync channels, sending presence messages or scheduling timers", async () => {
  expect(vi.getTimerCount()).toBe(0);
  // Load the registry through its browser branch, then load the full runtime
  // graph without inventing a partial DOM for unrelated browser modules.
  vi.stubGlobal("window", {});
  await import("../src/core/scopedSyncRegistry.js");
  vi.stubGlobal("window", undefined);
  runtime = await import("../src/core/scopedSyncRuntime.js");
  expect(channelConstructor.mock.calls.filter(([name]) => name === "build-scoped-sync-presence")).toEqual([]);
  expect(presenceMessages()).toEqual([]);
  expect(vi.getTimerCount()).toBe(0);
});

it("observes remote scope leases while the runtime is active with no local mounted scope", async () => {
  runtime = await import("../src/core/scopedSyncRuntime.js");
  vi.stubGlobal("window", {});
  expect(vi.getTimerCount()).toBe(0);
  runtime.startScopedSyncRuntime({ ownsLock: () => true, request: vi.fn() });
  expect(presenceChannels()).toHaveLength(1);
  expect(presenceMessages()).toEqual([expect.objectContaining({ type: "request" })]);
  expect(vi.getTimerCount()).toBe(1);
  runtime.stopScopedSyncRuntime();
  expect(presenceChannels()[0].close).toHaveBeenCalledOnce();
  expect(presenceChannels()[0].onmessage).toBeNull();
  expect(vi.getTimerCount()).toBe(0);
});

it("keeps local presence alive after the runtime stops until the final mounted scope closes", async () => {
  runtime = await import("../src/core/scopedSyncRuntime.js");
  vi.stubGlobal("window", {});
  const { registerScopedSync, taskSyncScope } = await import("../src/core/scopedSyncRegistry.js");
  release = registerScopedSync({ deviceId: "import-lifecycle-device", projectId: "p", scope: taskSyncScope("t") });
  expect(vi.getTimerCount()).toBe(1);
  runtime.startScopedSyncRuntime({ ownsLock: () => false, request: vi.fn() });
  expect(vi.getTimerCount()).toBe(1);
  runtime.stopScopedSyncRuntime();
  expect(presenceChannels()[0].close).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(1);
  release(); release = null;
  expect(presenceChannels()[0].close).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  release = registerScopedSync({ deviceId: "import-lifecycle-device", projectId: "p", scope: taskSyncScope("reopened") });
  expect(presenceChannels()).toHaveLength(2);
  expect(presenceChannels()[1].onmessage).toEqual(expect.any(Function));
  release(); release = null;
  expect(presenceChannels()[1].close).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});
