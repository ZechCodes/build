import { describe, it, expect } from "vitest";
import { creationDeviceId, homeDeviceId, onlineStickyDeviceId } from "../src/core/devicePolicy.js";

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

// The home device is where creation goes and what the App.* aliases point at:
// the sticky choice while it can answer, and otherwise whichever device can.
describe("homeDeviceId", () => {
  it("returns the sticky device when it is online", () => {
    const devices = [
      { id: "dev-a", status: "online" },
      { id: "dev-b", status: "online" },
    ];
    expect(homeDeviceId(devices, "dev-b")).toBe("dev-b");
  });

  it("falls back to the first online device in list order", () => {
    const devices = [
      { id: "dev-a", status: "offline" },
      { id: "dev-b", status: "online" },
      { id: "dev-c", status: "online" },
    ];
    expect(homeDeviceId(devices, "dev-a")).toBe("dev-b");
    expect(homeDeviceId(devices, null)).toBe("dev-b");
    expect(homeDeviceId(devices, "dev-z")).toBe("dev-b");
  });

  it("answers null when nothing is online", () => {
    expect(homeDeviceId([{ id: "dev-a", status: "offline" }], "dev-a")).toBeNull();
    expect(homeDeviceId([], "dev-a")).toBeNull();
  });
});

// Where creation goes, said so a surface can name it: the machine that would
// take the work right now, and — while nothing can take it — the machine the
// user picked, so the composer can say whose return it is waiting for.
describe("creationDeviceId", () => {
  it("is the device home policy names while something is online", () => {
    const devices = [
      { id: "dev-a", status: "online" },
      { id: "dev-b", status: "online" },
    ];
    expect(creationDeviceId(devices, "dev-b")).toBe("dev-b");
    expect(creationDeviceId(devices, null)).toBe("dev-a");
  });

  it("keeps naming the picked device while nothing is online", () => {
    const devices = [{ id: "dev-a", status: "offline" }];
    expect(creationDeviceId(devices, "dev-a")).toBe("dev-a");
  });

  it("names nobody when the account has neither a pick nor an online device", () => {
    expect(creationDeviceId([], null)).toBeNull();
  });
});
