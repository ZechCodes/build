import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchDevices } from "../src/api.js";

afterEach(() => vi.unstubAllGlobals());

describe("device presence api failures", () => {
  it("rejects request failures instead of turning them into an empty account", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("network down")));
    await expect(fetchDevices()).rejects.toThrow("network down");
  });

  it("rejects non-ok and malformed responses", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false }));
    await expect(fetchDevices()).rejects.toThrow("could not read devices");

    fetch.mockResolvedValue({ ok: true, json: async () => ({ devices: null }) });
    await expect(fetchDevices()).rejects.toThrow("invalid devices response");
  });

  it("preserves a successful empty device list for onboarding", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, json: async () => ({ devices: [] }) }));
    await expect(fetchDevices()).resolves.toEqual([]);
  });
});
