import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchIceServers } from "../src/api.js";

afterEach(() => vi.unstubAllGlobals());

describe("ICE server API diagnostics", () => {
  it("names the HTTP refusal without retaining the response or credentials", async () => {
    const json = vi.fn();
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 401, json })));
    await expect(fetchIceServers()).rejects.toMatchObject({ message: "could not mint ICE servers", status: 401 });
    expect(json).not.toHaveBeenCalled();
  });

  it("returns the ICE server array on success", async () => {
    const iceServers = [{ urls: ["stun:test.invalid"] }];
    const fetch = vi.fn(async () => ({ ok: true, json: async () => ({ iceServers }) }));
    vi.stubGlobal("fetch", fetch);
    await expect(fetchIceServers()).resolves.toEqual(iceServers);
    expect(fetch).toHaveBeenCalledWith("/api/rtc/ice-servers", { method: "POST" });
  });
});
