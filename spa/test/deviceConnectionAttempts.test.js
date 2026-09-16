import { describe, expect, it, vi } from "vitest";

import { createDeviceConnectionAttempts } from "../src/core/deviceConnectionAttempts.js";

const deferred = () => {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
};

describe("a device connection-attempt controller", () => {
  it("shares the exact promise while one attempt is connecting", async () => {
    const pending = deferred();
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const first = owner.connect(() => pending.promise);
    const second = owner.connect(vi.fn());

    expect(second).toBe(first);
    expect(owner.state).toBe("connecting");
    pending.resolve("ready");
    await expect(first).resolves.toBe("ready");
    expect(owner.state).toBe("succeeded");
  });

  it("publishes the attempt before synchronously invoking it", async () => {
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    let reentered;

    const first = owner.connect(() => {
      reentered = owner.connect(vi.fn());
      return "ready";
    });

    expect(reentered).toBe(first);
    await expect(first).resolves.toBe("ready");
  });

  it("starts a fresh attempt immediately after the prior promise settles", async () => {
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const first = owner.connect(() => "first");
    await expect(first).resolves.toBe("first");

    const second = owner.connect(() => "second");

    expect(second).not.toBe(first);
    await expect(second).resolves.toBe("second");
  });

  it("cancels provisional resources in ownership order and rejects late success", async () => {
    const pending = deferred();
    const closed = [];
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const connection = owner.connect(async (attempt) => {
      attempt.own("session", () => closed.push("session"));
      attempt.own("peer", () => closed.push("peer"));
      await pending.promise;
      return "late";
    });

    owner.cancel();
    expect(owner.state).toBe("idle");
    expect(closed).toEqual(["session", "peer"]);
    await expect(connection).rejects.toThrow(/cancelled/);
    pending.resolve();
    expect(owner.state).toBe("idle");
  });

  it("closes a resource handed to a stale token without touching a new attempt", async () => {
    const oldPending = deferred();
    const closeLate = vi.fn();
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const oldConnection = owner.connect(async (attempt) => {
      await oldPending.promise;
      attempt.own("late-session", closeLate);
    });
    owner.cancel();

    const fresh = owner.connect(() => "fresh");
    await expect(fresh).resolves.toBe("fresh");
    await expect(oldConnection).rejects.toThrow(/cancelled/);
    oldPending.resolve();
    await Promise.resolve();

    expect(closeLate).toHaveBeenCalledTimes(1);
    expect(owner.state).toBe("succeeded");
  });

  it("records an authoritative failure but ignores a rejection after cancellation", async () => {
    const currentError = new Error("current failure");
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    await expect(owner.connect(() => Promise.reject(currentError))).rejects.toBe(currentError);
    expect(owner.state).toBe("failed");

    const pending = deferred();
    const stale = owner.connect(() => pending.promise);
    owner.cancel();
    await expect(stale).rejects.toThrow(/cancelled/);
    pending.reject(new Error("late failure"));
    await Promise.resolve();
    expect(owner.state).toBe("idle");
  });

  it("retires captured owners and permits a new owner for the same device", async () => {
    const pending = deferred();
    const attempts = createDeviceConnectionAttempts();
    const oldOwner = attempts.forDevice("dev-a");
    const oldConnection = oldOwner.connect(() => pending.promise);

    attempts.retire("dev-a");
    expect(oldOwner.state).toBe("retired");
    expect(() => oldOwner.connect(vi.fn())).toThrow(/retired/);
    const replacement = attempts.forDevice("dev-a");
    await expect(replacement.connect(() => "fresh")).resolves.toBe("fresh");

    await expect(oldConnection).rejects.toThrow(/retired/);
    pending.resolve("late");
    expect(replacement.state).toBe("succeeded");
  });

  it("detaches cleanup before callbacks and continues past a throwing disposer", async () => {
    const pending = deferred();
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const laterClose = vi.fn();
    let reentered;
    const connection = owner.connect((attempt) => {
      attempt.own("session", () => {
        reentered = owner.connect(() => "replacement");
        throw new Error("close failed");
      });
      attempt.own("peer", laterClose);
      return pending.promise;
    });

    owner.cancel();

    await expect(connection).rejects.toThrow(/cancelled/);
    await expect(reentered).resolves.toBe("replacement");
    expect(laterClose).toHaveBeenCalledTimes(1);
    expect(owner.state).toBe("succeeded");
  });

  it("does not publish an old failure over a replacement started by cleanup", async () => {
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const onFailure = vi.fn();
    let replacement;
    const failed = owner.connect((attempt) => {
      attempt.own("session", () => { replacement = owner.connect(() => "fresh"); });
      throw new Error("old failure");
    }, { onFailure });

    await expect(failed).rejects.toThrow("old failure");
    await expect(replacement).resolves.toBe("fresh");
    expect(onFailure).not.toHaveBeenCalled();
    expect(owner.state).toBe("succeeded");
  });

  it("keeps retirement authoritative when failure cleanup retires the owner", async () => {
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const onFailure = vi.fn();
    const failed = owner.connect((attempt) => {
      attempt.own("session", () => attempts.retire("dev-a"));
      throw new Error("old failure");
    }, { onFailure });

    await expect(failed).rejects.toThrow("old failure");
    expect(owner.state).toBe("retired");
    expect(onFailure).not.toHaveBeenCalled();
  });

  it("preserves the connection error when failure reporting throws", async () => {
    const attempts = createDeviceConnectionAttempts();
    const owner = attempts.forDevice("dev-a");
    const original = new Error("wire failed");

    const failed = owner.connect(() => Promise.reject(original), {
      onFailure: () => { throw new Error("report failed"); },
    });

    await expect(failed).rejects.toBe(original);
    expect(owner.state).toBe("failed");
  });

  it("clear retires every captured owner", () => {
    const attempts = createDeviceConnectionAttempts();
    const first = attempts.forDevice("dev-a");
    const second = attempts.forDevice("dev-b");

    attempts.clear();

    expect(first.state).toBe("retired");
    expect(second.state).toBe("retired");
    expect(attempts.forDevice("dev-a")).not.toBe(first);
  });
});
