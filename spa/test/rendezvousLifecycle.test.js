import { describe, expect, it, vi } from "vitest";

import { createRendezvousLifecycle } from "../src/core/rendezvousLifecycle.js";

function fixture() {
  const rendezvousByDevice = new Map();
  const lifecycle = createRendezvousLifecycle((deviceId) => {
    const rendezvous = { deviceId, close: vi.fn() };
    rendezvousByDevice.set(deviceId, rendezvous);
    return rendezvous;
  });
  return { lifecycle, rendezvousByDevice };
}

describe("a device rendezvous owner", () => {
  it("closes only when its last overlapping lease is released", () => {
    const { lifecycle, rendezvousByDevice } = fixture();
    const owner = lifecycle.forDevice("dev-a");
    const first = owner.acquire();
    const second = owner.acquire();

    expect(owner.state).toBe("negotiating");
    expect(first.rendezvous).toBe(second.rendezvous);
    first.release();
    first.release();
    expect(rendezvousByDevice.get("dev-a").close).not.toHaveBeenCalled();

    second.release();
    expect(owner.state).toBe("idle");
    expect(rendezvousByDevice.get("dev-a").close).toHaveBeenCalledTimes(1);

    const next = owner.acquire();
    first.release();
    expect(owner.state).toBe("negotiating");
    next.release();
    expect(rendezvousByDevice.get("dev-a").close).toHaveBeenCalledTimes(2);
  });

  it("invalidates stale releases on force close and permits a fresh retry", () => {
    const { lifecycle, rendezvousByDevice } = fixture();
    const owner = lifecycle.forDevice("dev-a");
    const stale = owner.acquire();
    const rendezvous = stale.rendezvous;

    owner.forceClose();
    expect(owner.state).toBe("idle");
    expect(rendezvous.close).toHaveBeenCalledTimes(1);

    const retry = owner.acquire();
    stale.release();
    expect(rendezvous.close).toHaveBeenCalledTimes(1);
    retry.release();
    expect(rendezvous.close).toHaveBeenCalledTimes(2);
  });

  it("lets a released lease reacquire only while its owner generation is current", () => {
    const { lifecycle } = fixture();
    const owner = lifecycle.forDevice("dev-a");
    const session = owner.acquire();
    session.release();

    const restart = session.reacquire();
    expect(restart?.rendezvous).toBe(session.rendezvous);
    owner.forceClose();
    expect(session.reacquire()).toBe(null);
    expect(restart.reacquire()).toBe(null);
  });

  it("retires captured owners while a new owner for the same device works", () => {
    const { lifecycle } = fixture();
    const oldOwner = lifecycle.forDevice("dev-a");
    const stale = oldOwner.acquire();

    lifecycle.retire("dev-a");
    expect(oldOwner.state).toBe("retired");
    expect(stale.reacquire()).toBe(null);
    expect(() => oldOwner.acquire()).toThrow(/retired/);

    const replacement = lifecycle.forDevice("dev-a");
    expect(replacement).not.toBe(oldOwner);
    expect(replacement.acquire().rendezvous).toBeDefined();
  });

  it("clears and retires every captured owner", () => {
    const { lifecycle } = fixture();
    const first = lifecycle.forDevice("dev-a");
    const second = lifecycle.forDevice("dev-b");
    first.acquire();
    second.acquire();

    lifecycle.clear();

    expect(first.state).toBe("retired");
    expect(second.state).toBe("retired");
    expect(lifecycle.forDevice("dev-a")).not.toBe(first);
  });

  it("commits state before close side effects and keeps close events idempotent", () => {
    let stateSeenDuringClose;
    const rendezvous = { close: vi.fn(() => { stateSeenDuringClose = owner.state; }) };
    const lifecycle = createRendezvousLifecycle(() => rendezvous);
    const owner = lifecycle.forDevice("dev-a");

    owner.forceClose();
    expect(rendezvous.close).not.toHaveBeenCalled();
    const lease = owner.acquire();
    lease.release();
    expect(stateSeenDuringClose).toBe("idle");

    owner.retire();
    owner.retire();
    expect(owner.state).toBe("retired");
    expect(rendezvous.close).toHaveBeenCalledTimes(1);
  });
});
