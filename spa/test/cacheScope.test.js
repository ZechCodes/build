import { beforeEach, describe, expect, it } from "vitest";

import {
  adoptCacheScope,
  cacheDeviceId,
  clearCacheScope,
  currentCacheScope,
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

  it("retires the old scope before adopting another device", () => {
    const first = adoptCacheScope("device-a");
    const second = adoptCacheScope("device-b");

    expect(second).not.toBe(first);
    expect(first.active()).toBe(false);
    expect(first.address({ entityId: "run-a", kind: "thread" })).toBe(null);
    expect(second.address({ entityId: "run-b", kind: "thread" })).toEqual({
      deviceId: "device-b",
      entityId: "run-b",
      kind: "thread",
    });
    expect(currentCacheScope()).toBe(second);
  });

  it("clears the ambient compatibility accessor on application disposal", () => {
    const captured = adoptCacheScope("device-a");

    clearCacheScope();

    expect(captured.active()).toBe(false);
    expect(currentCacheScope()).toBe(null);
    expect(cacheDeviceId()).toBe(null);
  });
});
