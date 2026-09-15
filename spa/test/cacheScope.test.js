import { beforeEach, describe, expect, it } from "vitest";

import { clearCacheScope, releaseScope, scopeFor } from "../src/core/cacheScope.js";

describe("the application cache scope", () => {
  beforeEach(() => clearCacheScope());

  it("keeps the same captured scope when the same device reconnects", () => {
    const first = scopeFor("device-a");
    const resumed = scopeFor("device-a");

    expect(resumed).toBe(first);
    expect(first.active()).toBe(true);
    expect(first.deviceId).toBe("device-a");
  });

  it("leaves the first scope live when another device is adopted; releaseScope retires it", () => {
    // Every paired device keeps its own scope: a second device joining must not
    // invalidate the first device's in-flight reads. Only retiring it does.
    const first = scopeFor("device-a");
    const second = scopeFor("device-b");

    expect(second).not.toBe(first);
    expect(first.active()).toBe(true);
    expect(first.address({ entityId: "run-a", kind: "thread" })).toEqual({
      deviceId: "device-a",
      entityId: "run-a",
      kind: "thread",
    });
    expect(second.address({ entityId: "run-b", kind: "thread" })).toEqual({
      deviceId: "device-b",
      entityId: "run-b",
      kind: "thread",
    });

    releaseScope("device-a");

    expect(first.active()).toBe(false);
    expect(first.address({ entityId: "run-a", kind: "thread" })).toBe(null);
    expect(second.active()).toBe(true);
  });

  it("scopeFor answers the same object until it is released", () => {
    const scope = scopeFor("device-a");

    expect(scopeFor("device-a")).toBe(scope);

    releaseScope("device-a");

    expect(scope.active()).toBe(false);
    expect(scopeFor("device-a")).not.toBe(scope);
    expect(scopeFor(null)).toBe(null);
  });

  it("clearCacheScope retires every device's scope", () => {
    const first = scopeFor("device-a");
    const second = scopeFor("device-b");

    clearCacheScope();

    expect(first.active()).toBe(false);
    expect(second.active()).toBe(false);
    expect(scopeFor("device-a")).not.toBe(first);
  });
});
