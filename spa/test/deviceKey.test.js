import { describe, it, expect } from "vitest";
import { deviceKey, routeProjectKey, splitDeviceKey } from "../src/core/deviceKey.js";

// A project's identity across the account is the pair (deviceId, projectId):
// every device's first project is `proj-1`, so the bare bridge id names nothing
// on its own. This module is the only place the two are joined or taken apart.

describe("the device key", () => {
  it("mints `${deviceId}/${projectId}` and splits it back", () => {
    const key = deviceKey("6f3c-uuid", "proj-1");

    expect(key).toBe("6f3c-uuid/proj-1");
    expect(splitDeviceKey(key)).toEqual({ deviceId: "6f3c-uuid", projectId: "proj-1" });
  });

  it("splits on the first `/` — a project id containing `/` cannot exist", () => {
    // The bridge mints `proj-<n>` from a per-machine counter, and api device ids
    // are uuids: neither half ever holds a slash, so the first one is the seam.
    expect(splitDeviceKey("dev-a/proj-1/extra")).toEqual({ deviceId: "dev-a", projectId: "proj-1/extra" });
  });

  it("answers null when the key is not a string or either half is empty", () => {
    expect(splitDeviceKey(null)).toBe(null);
    expect(splitDeviceKey(42)).toBe(null);
    expect(splitDeviceKey("proj-1")).toBe(null);
    expect(splitDeviceKey("/proj-1")).toBe(null);
    expect(splitDeviceKey("dev-a/")).toBe(null);
    expect(splitDeviceKey("")).toBe(null);
  });
});

// A route carries the bare id one bridge minted and the machine it minted it
// on, so the project it stands in is named from the pair — in one place, for
// the toolbar and the rail alike.
describe("the project a route stands in", () => {
  it("names it by the route's device and project together", () => {
    expect(routeProjectKey({ name: "branch", deviceId: "dev-a", projectId: "proj-1" })).toBe("dev-a/proj-1");
  });

  it("names nothing when the route names no machine, no project, or nothing at all", () => {
    expect(routeProjectKey({ name: "branch", projectId: "proj-1" })).toBe(null);
    expect(routeProjectKey({ name: "capture", deviceId: "dev-a" })).toBe(null);
    expect(routeProjectKey(null)).toBe(null);
  });
});
