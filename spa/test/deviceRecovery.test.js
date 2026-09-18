import { beforeEach, describe, expect, it, vi } from "vitest";

import { createDeviceRecoverySupervisor } from "../src/core/deviceRecovery.js";

describe("device recovery supervisor", () => {
  beforeEach(() => vi.useFakeTimers());

  const setup = () => {
    const attempts = [];
    const cancelled = [];
    const recovery = createDeviceRecoverySupervisor({
      attempt: (deviceId) => attempts.push(deviceId),
      cancelAttempt: (deviceId) => cancelled.push(deviceId),
      random: () => 0.5,
    });
    return { recovery, attempts, cancelled };
  };

  it("tries a lost online device immediately and stays silent until that fresh attempt fails", async () => {
    const { recovery, attempts } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);

    recovery.recoverNow("dev-a");
    expect(recovery.snapshot("dev-a")).toEqual({
      deviceId: "dev-a", status: "attempting", failedAttempts: 0, nextAttemptAt: null,
    });
    expect(attempts).toEqual([]);

    await vi.advanceTimersByTimeAsync(0);
    expect(attempts).toEqual(["dev-a"]);
    recovery.failed("dev-a");
    expect(recovery.snapshot("dev-a")).toEqual({
      deviceId: "dev-a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 2000,
    });
  });

  it("backs off at two, four, eight, sixteen and thirty seconds without duplicate presence timers", async () => {
    const { recovery, attempts } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);
    recovery.recoverNow("dev-a");
    await vi.advanceTimersByTimeAsync(0);

    for (const [failure, delay] of [
      [1, 2000], [2, 4000], [3, 8000], [4, 16000], [5, 30000], [6, 30000],
    ]) {
      recovery.failed("dev-a");
      const before = recovery.snapshot("dev-a");
      expect(before.failedAttempts).toBe(failure);
      expect(before.nextAttemptAt - Date.now()).toBe(delay);
      recovery.syncPresence([{ id: "dev-a", status: "online" }]);
      expect(recovery.snapshot("dev-a").nextAttemptAt).toBe(before.nextAttemptAt);
      await vi.advanceTimersByTimeAsync(delay);
      await vi.advanceTimersByTimeAsync(0);
    }
    expect(attempts).toHaveLength(7);
  });

  it("cancels timers and in-flight authority when a device goes offline or is removed", async () => {
    const { recovery, attempts, cancelled } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }, { id: "dev-b", status: "online" }]);
    recovery.recoverNow("dev-a");
    recovery.recoverNow("dev-b");
    await vi.advanceTimersByTimeAsync(0);
    recovery.failed("dev-a");

    recovery.syncPresence([{ id: "dev-a", status: "offline" }]);

    expect(recovery.snapshot()).toEqual([]);
    expect(cancelled).toEqual(expect.arrayContaining(["dev-a", "dev-b"]));
    await vi.advanceTimersByTimeAsync(60000);
    expect(attempts).toEqual(["dev-a", "dev-b"]);
  });

  it("invalidates late outcomes and clears failure history on success and manual retry", async () => {
    const { recovery, attempts } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);
    recovery.recoverNow("dev-a");
    await vi.advanceTimersByTimeAsync(0);
    recovery.failed("dev-a");
    const staleEpoch = recovery.epoch("dev-a");

    recovery.beginAttempt("dev-a", { resetFailures: true });
    expect(recovery.snapshot("dev-a").failedAttempts).toBe(0);
    recovery.failed("dev-a", { epoch: staleEpoch });
    expect(recovery.snapshot("dev-a").status).toBe("attempting");
    expect(attempts).toEqual(["dev-a"]);

    recovery.connected("dev-a");
    expect(recovery.snapshot()).toEqual([]);
  });

  it("does not schedule fatal failures and reset cancels every device", async () => {
    const { recovery, attempts, cancelled } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }, { id: "dev-b", status: "online" }]);
    recovery.recoverNow("dev-a");
    recovery.recoverNow("dev-b");
    await vi.advanceTimersByTimeAsync(0);

    recovery.failed("dev-a", { retryable: false });
    expect(recovery.snapshot("dev-a")).toBe(null);
    recovery.reset();
    expect(recovery.snapshot()).toEqual([]);
    expect(cancelled).toEqual(expect.arrayContaining(["dev-a", "dev-b"]));
    expect(attempts).toEqual(["dev-a", "dev-b"]);
  });

  it("cannot be re-armed synchronously by resources cancelled on offline, removal, stop, or reset", () => {
    let recovery;
    const attempted = vi.fn();
    const cancelled = vi.fn((deviceId) => recovery.recoverNow(deviceId));
    recovery = createDeviceRecoverySupervisor({ attempt: attempted, cancelAttempt: cancelled, random: () => 0.5 });

    recovery.syncPresence([{ id: "dev-a", status: "online" }, { id: "dev-b", status: "online" }]);
    recovery.recoverNow("dev-a");
    recovery.recoverNow("dev-b");
    recovery.syncPresence([{ id: "dev-b", status: "online" }]);
    expect(recovery.snapshot("dev-a")).toBe(null);

    recovery.stop("dev-b");
    expect(recovery.snapshot("dev-b")).toBe(null);

    recovery.syncPresence([{ id: "dev-c", status: "online" }]);
    recovery.recoverNow("dev-c");
    recovery.reset();
    expect(recovery.snapshot()).toEqual([]);
    vi.runAllTimers();
    expect(attempted).not.toHaveBeenCalled();
  });
});
