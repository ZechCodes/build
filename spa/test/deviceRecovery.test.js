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

  it("backs off at two, four, eight and ten seconds without duplicate presence timers", async () => {
    const { recovery, attempts } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);
    recovery.recoverNow("dev-a");
    await vi.advanceTimersByTimeAsync(0);

    for (const [failure, delay] of [
      // Ten seconds is the ceiling, not thirty: the failures this ladder sees are
      // usually a sleeping phone rather than an unreachable machine (#60).
      [1, 2000], [2, 4000], [3, 8000], [4, 10000], [5, 10000], [6, 10000],
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

  // #60: Zech's phone had climbed the ladder across two earlier failures and was
  // still sitting on the top step when he picked it up, so waking the screen bought
  // a half-minute of nothing. A wake is not evidence about the machine — it is
  // evidence that the reason the last dial failed has probably gone.
  it("drops a waiting device back to the floor and dials at once when the app wakes", async () => {
    const { recovery, attempts } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);
    recovery.recoverNow("dev-a");
    await vi.advanceTimersByTimeAsync(0);

    // Climb to the ceiling the way two failures and a sleep would.
    for (const delay of [2000, 4000, 8000]) {
      recovery.failed("dev-a");
      await vi.advanceTimersByTimeAsync(delay);
      await vi.advanceTimersByTimeAsync(0);
    }
    recovery.failed("dev-a");
    expect(recovery.snapshot("dev-a")).toMatchObject({ status: "waiting", failedAttempts: 4 });
    expect(recovery.snapshot("dev-a").nextAttemptAt - Date.now()).toBe(10000);
    const before = attempts.length;

    const woken = recovery.wake("visible");

    expect(woken).toEqual({ reason: "visible", woke: ["dev-a"] });
    // Dialling now, and the count is dropped as well as the timer: a ladder built
    // out of the phone's own absence is not a ladder worth keeping.
    expect(recovery.snapshot("dev-a")).toMatchObject({ status: "attempting", failedAttempts: 0 });
    await vi.advanceTimersByTimeAsync(0);
    expect(attempts.length).toBe(before + 1);

    // And the next failure waits the floor, not the ceiling.
    recovery.failed("dev-a");
    expect(recovery.snapshot("dev-a").nextAttemptAt - Date.now()).toBe(2000);
  });

  it("leaves a device that is already dialling alone, rather than dialling it twice", async () => {
    const { recovery, attempts } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);
    recovery.recoverNow("dev-a");
    await vi.advanceTimersByTimeAsync(0);
    expect(attempts).toEqual(["dev-a"]);

    // Mid-attempt: a second dial here is the double mint this issue also reports.
    expect(recovery.wake("online")).toEqual({ reason: "online", woke: [] });
    await vi.advanceTimersByTimeAsync(0);

    expect(attempts).toEqual(["dev-a"]);
  });

  it("wakes every waiting device, and says nothing about ones it is not tracking", async () => {
    const { recovery } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }, { id: "dev-b", status: "online" }]);
    for (const id of ["dev-a", "dev-b"]) {
      recovery.recoverNow(id);
    }
    await vi.advanceTimersByTimeAsync(0);
    recovery.failed("dev-a");
    recovery.failed("dev-b");

    expect(recovery.wake("network-change").woke.sort()).toEqual(["dev-a", "dev-b"]);
  });

  it("is a no-op when nothing is waiting", async () => {
    const { recovery, attempts } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);

    expect(recovery.wake("visible")).toEqual({ reason: "visible", woke: [] });
    await vi.advanceTimersByTimeAsync(0);

    expect(attempts).toEqual([]);
  });

  // The fourth reset the issue asks for was already there, by a different route:
  // a connected session forgets the record, so the count starts from zero next
  // time. Asserted so it cannot regress silently.
  it("forgets the failure count once a session reaches connected", async () => {
    const { recovery } = setup();
    recovery.syncPresence([{ id: "dev-a", status: "online" }]);
    recovery.recoverNow("dev-a");
    await vi.advanceTimersByTimeAsync(0);
    recovery.failed("dev-a");
    recovery.failed("dev-a");

    recovery.connected("dev-a", { epoch: recovery.epoch("dev-a") });
    expect(recovery.snapshot("dev-a")).toBe(null);

    recovery.recoverNow("dev-a");
    await vi.advanceTimersByTimeAsync(0);
    recovery.failed("dev-a");

    expect(recovery.snapshot("dev-a").nextAttemptAt - Date.now()).toBe(2000);
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
