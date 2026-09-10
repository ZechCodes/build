import { describe, it, expect } from "vitest";
import { onlineStickyDeviceId } from "../src/core/devicePolicy.js";

// The sticky device choice is only honored when that device is actually online.
// Anything else must fall back to "any device" — pinning a resume to an offline
// sticky device would ignore the working device's return forever.

describe("onlineStickyDeviceId", () => {
  const devices = [
    { id: "dev-a", status: "online" },
    { id: "dev-b", status: "offline" },
  ];

  it("returns the sticky device when it is online", () => {
    expect(onlineStickyDeviceId(devices, "dev-a")).toBe("dev-a");
  });

  it("falls back to any device when the sticky device is offline", () => {
    expect(onlineStickyDeviceId(devices, "dev-b")).toBeNull();
  });

  it("falls back when the sticky device is unknown or unset", () => {
    expect(onlineStickyDeviceId(devices, "dev-z")).toBeNull();
    expect(onlineStickyDeviceId(devices, null)).toBeNull();
    expect(onlineStickyDeviceId([], "dev-a")).toBeNull();
  });
});
