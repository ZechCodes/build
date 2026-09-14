import { beforeEach, describe, expect, it } from "vitest";

import {
  adoptCacheScope,
  cacheDeviceId,
  clearCacheScope,
  currentCacheScope,
  releaseScope,
  scopeFor,
} from "../src/core/cacheScope.js";

describe("the application cache scope", () => {
  beforeEach(() => clearCacheScope());

  it("keeps the same captured scope when the same device reconnects", () => {
    const first = adoptCacheScope("device-a");
    const resumed = adoptCacheScope("device-a");

    expect(resumed).toBe(first);
    expect(first.active()).toBe(true);
    expect(cacheDeviceId()).toBe("device-a");
  });

  it("leaves the first scope live when another device is adopted; releaseScope retires it", () => {
    // Every paired device keeps its own scope: a second device joining must not
    // invalidate the first device's in-flight reads. Only retiring it does.
    const first = adoptCacheScope("device-a");
    const second = adoptCacheScope("device-b");

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
    expect(currentCacheScope()).toBe(second);

    releaseScope("device-a");

    expect(first.active()).toBe(false);
    expect(first.address({ entityId: "run-a", kind: "thread" })).toBe(null);
    expect(second.active()).toBe(true);
    expect(currentCacheScope()).toBe(second);
  });

  it("scopeFor answers the same object until it is released", () => {
    const scope = scopeFor("device-a");

    expect(scopeFor("device-a")).toBe(scope);

    releaseScope("device-a");

    expect(scope.active()).toBe(false);
    expect(scopeFor("device-a")).not.toBe(scope);
    expect(scopeFor(null)).toBe(null);
  });

  it("clears the ambient compatibility accessor on application disposal", () => {
    const captured = adoptCacheScope("device-a");
    const other = scopeFor("device-b");

    clearCacheScope();

    expect(captured.active()).toBe(false);
    expect(other.active()).toBe(false);
    expect(currentCacheScope()).toBe(null);
    expect(cacheDeviceId()).toBe(null);
  });
});
