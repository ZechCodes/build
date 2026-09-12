import { describe, expect, it, vi } from "vitest";
import { openFirstReachableDevice } from "../src/core/deviceBootstrap.js";

describe("device bootstrap", () => {
  it("tries every API-online device when the first advertised device cannot accept", async () => {
    const open = vi.fn(async ({ preferDeviceId }) => {
      if (preferDeviceId === "dev-a") throw new Error("device did not accept the session");
      return { deviceId: preferDeviceId };
    });
    const devices = ["dev-a", "dev-b", "dev-c"].map((id) => ({ id, status: "online" }));

    await expect(openFirstReachableDevice({ devices, selectedDeviceId: null, open })).resolves.toEqual({
      deviceId: "dev-b",
    });
    expect(open).toHaveBeenNthCalledWith(1, { preferDeviceId: "dev-a" });
    expect(open).toHaveBeenNthCalledWith(2, { preferDeviceId: "dev-b" });
  });

  it("tries a sticky online device first, then rotates without waiting for an offline event", async () => {
    const open = vi.fn(async ({ preferDeviceId }) => {
      if (preferDeviceId === "dev-c") throw new Error("no device online");
      return { deviceId: preferDeviceId };
    });
    const devices = ["dev-a", "dev-b", "dev-c"].map((id) => ({ id, status: "online" }));

    await openFirstReachableDevice({ devices, selectedDeviceId: "dev-c", open });

    expect(open.mock.calls.map(([options]) => options.preferDeviceId)).toEqual(["dev-c", "dev-a"]);
  });

  it("eventually tries devices whose API status is stale offline", async () => {
    const open = vi.fn(async ({ preferDeviceId }) => {
      if (preferDeviceId === "dev-a") throw new Error("device did not accept the session");
      return { deviceId: preferDeviceId };
    });

    await expect(openFirstReachableDevice({
      devices: [{ id: "dev-a", status: "online" }, { id: "dev-b", status: "offline" }],
      selectedDeviceId: null,
      open,
    })).resolves.toEqual({ deviceId: "dev-b" });
  });

  it("keeps key-pin failures fatal instead of trying another device", async () => {
    const failure = Object.assign(new Error("relay-supplied device key does not match"), { securityCritical: true });
    const open = vi.fn().mockRejectedValue(failure);

    await expect(openFirstReachableDevice({
      devices: [{ id: "dev-a", status: "online" }, { id: "dev-b", status: "online" }],
      selectedDeviceId: null,
      open,
    })).rejects.toBe(failure);
    expect(open).toHaveBeenCalledTimes(1);
  });
});
