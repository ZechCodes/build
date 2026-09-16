import { describe, expect, it, vi } from "vitest";
import { createDeviceLifecycles } from "../src/core/deviceLifecycle.js";

const resources = (events, options = {}) => ({
  session: {
    call: vi.fn(),
    fail: vi.fn(() => {
      events.push("fail");
      if (options.throwFail) throw new Error("broken fail");
    }),
    close: vi.fn(() => {
      events.push("session");
      options.onSessionClose?.();
      if (options.throwSession) throw new Error("broken session");
    }),
  },
  peerLink: {
    close: vi.fn(() => {
      events.push("peer");
      if (options.throwPeer) throw new Error("broken peer");
    }),
  },
  onDetached: vi.fn(() => events.push("detach")),
});

describe("established device lifecycle ownership", () => {
  it("publishes one bundle and projects its availability", () => {
    const owner = createDeviceLifecycles().forDevice("dev-a");
    const held = resources([]);
    const lifetime = owner.adopt(held);

    expect(owner.snapshot()).toMatchObject({
      state: "available",
      session: held.session,
      peerLink: held.peerLink,
      offline: false,
      blocked: null,
    });
    expect(lifetime.current()).toBe(true);
  });

  it("detaches state before reentrant cleanup and closes session before peer", () => {
    const registry = createDeviceLifecycles();
    const owner = registry.forDevice("dev-a");
    const events = [];
    const first = resources(events, {
      onSessionClose: () => {
        expect(owner.snapshot()).toMatchObject({ session: null, peerLink: null, offline: true });
        owner.adopt(resources(events));
      },
      throwFail: true,
      throwSession: true,
    });
    const lifetime = owner.adopt(first);

    expect(owner.block(lifetime, "failed")).toBe(true);
    expect(events.slice(0, 4)).toEqual(["fail", "session", "detach", "peer"]);
    expect(owner.snapshot().offline).toBe(false);
    expect(lifetime.current()).toBe(false);
  });

  it("ignores stale lifetime loss after replacement", () => {
    const owner = createDeviceLifecycles().forDevice("dev-a");
    const first = owner.adopt(resources([]));
    const second = owner.adopt(resources([]));

    expect(owner.lose(first)).toBe(false);
    expect(owner.snapshot().offline).toBe(false);
    expect(second.current()).toBe(true);
  });

  it("keeps security refusal sticky across retry and presence", () => {
    const owner = createDeviceLifecycles().forDevice("dev-a");
    owner.refuse("wrong key");

    owner.retry();
    owner.presenceAway();

    expect(owner.snapshot()).toMatchObject({ offline: true, blocked: "refused", securityStop: "wrong key" });
  });

  it("retires captured ownership without affecting a same-id replacement", () => {
    const registry = createDeviceLifecycles();
    const oldOwner = registry.forDevice("dev-a");
    const held = resources([]);
    const lifetime = oldOwner.adopt(held);

    registry.retire("dev-a");
    const replacement = registry.forDevice("dev-a");
    replacement.adopt(resources([]));

    expect(oldOwner.snapshot().state).toBe("retired");
    expect(lifetime.current()).toBe(false);
    expect(replacement.snapshot().offline).toBe(false);
    expect(held.session.close).toHaveBeenCalledTimes(1);
    expect(held.peerLink.close).toHaveBeenCalledTimes(1);
  });

  it("returns a stale lifetime when outgoing cleanup reentrantly retires the owner", () => {
    const registry = createDeviceLifecycles();
    const owner = registry.forDevice("dev-a");
    owner.adopt(resources([], { onSessionClose: () => registry.retire("dev-a") }));

    const incoming = owner.adopt(resources([]));

    expect(incoming.current()).toBe(false);
    expect(owner.snapshot().state).toBe("retired");
  });

  it("ignores every state-changing event after retirement", () => {
    const owner = createDeviceLifecycles().forDevice("dev-a");
    owner.retire();

    owner.refuse("late refusal");
    owner.presenceAway();
    owner.blockCurrent("failed");
    owner.setAvailability({ offline: false });

    expect(owner.snapshot()).toMatchObject({ state: "retired", securityStop: null, session: null });
  });
});
