import { afterEach, beforeEach, expect, it, vi } from "vitest";

let runtime, release;
beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
});
afterEach(() => {
  runtime?.stopScopedSyncRuntime();
  release?.();
  runtime = null; release = null;
  vi.useRealTimers(); vi.unstubAllGlobals();
});

it("imports the scoped sync runtime without scheduling presence or recovery timers", async () => {
  expect(vi.getTimerCount()).toBe(0);
  runtime = await import("../src/core/scopedSyncRuntime.js");
  expect(vi.getTimerCount()).toBe(0);
});

it("observes remote scope leases while the runtime is active with no local mounted scope", async () => {
  runtime = await import("../src/core/scopedSyncRuntime.js");
  expect(vi.getTimerCount()).toBe(0);
  runtime.startScopedSyncRuntime({ ownsLock: () => true, request: vi.fn() });
  expect(vi.getTimerCount()).toBe(1);
  runtime.stopScopedSyncRuntime();
  expect(vi.getTimerCount()).toBe(0);
});

it("keeps local presence alive after the runtime stops until the final mounted scope closes", async () => {
  runtime = await import("../src/core/scopedSyncRuntime.js");
  const { registerScopedSync, taskSyncScope } = await import("../src/core/scopedSyncRegistry.js");
  release = registerScopedSync({ deviceId: "import-lifecycle-device", projectId: "p", scope: taskSyncScope("t") });
  expect(vi.getTimerCount()).toBe(1);
  runtime.startScopedSyncRuntime({ ownsLock: () => false, request: vi.fn() });
  expect(vi.getTimerCount()).toBe(1);
  runtime.stopScopedSyncRuntime();
  expect(vi.getTimerCount()).toBe(1);
  release(); release = null;
  expect(vi.getTimerCount()).toBe(0);
});
